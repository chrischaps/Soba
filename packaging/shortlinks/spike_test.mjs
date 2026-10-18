// Drives a running `npx wrangler dev` (http://127.0.0.1:8787) through the
// short-link API: every example in, back out byte for byte, and the refusals.
// Then the garden: plants, curation, the caps and paging.
//   node spike_test.mjs ../../patches
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { deflateRawSync } from "node:zlib";

const API = process.env.API ?? "http://127.0.0.1:8787";
const dir = process.argv[2] ?? "../../patches";
// A fresh block of addresses each run, so the last run's minute doesn't count
const run = Math.floor(Math.random() * 250);
let failures = 0;
const check = (ok, what) => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) failures++;
};
const payloadOf = (json) => "1." + deflateRawSync(Buffer.from(json), { level: 9 }).toString("base64url");
// Each example from its own address, so the rate limit doesn't stop the sweep
const post = (body, ip = "10.0.0.1") =>
  fetch(`${API}/v1/patches`, { method: "POST", body, headers: { "Content-Type": "text/plain", "CF-Connecting-IP": ip, Origin: "https://soba.chaps.dev" } });

let largest = 0;
const files = readdirSync(dir).filter((f) => f.endsWith(".json"));
for (const [i, file] of files.entries()) {
  const payload = payloadOf(JSON.stringify(JSON.parse(readFileSync(join(dir, file), "utf8"))));
  largest = Math.max(largest, payload.length);
  const first = await post(payload, `10.1.${run}.${i}`);
  const { id, url } = await first.json();
  const again = await post(payload, `10.2.${run}.${i}`);
  const back = await fetch(`${API}/v1/patches/${id}`);
  const same = (await back.text()) === payload;
  check(first.status === 200 && again.status === 200 && (await again.json()).id === id && same,
    `${file}: ${payload.length} chars -> ${url}`);
  check(back.headers.get("cache-control")?.includes("immutable"), `${file}: cached for good`);
}
console.log(`largest payload: ${largest} chars`);

const refused = async (body, status, what) => {
  const response = await post(body, `10.3.${run}.1`);
  check(response.status === status, `${what}: ${response.status} ${await response.text()}`);
};
await refused("hello", 400, "no version");
await refused("2.AAAA", 400, "a newer version");
await refused("1.!!!!", 400, "not base64url");
await refused("1.AAAA", 400, "not deflate");
await refused(payloadOf('{"not":"a patch"}'), 400, "JSON that isn't a patch");
await refused(payloadOf("x".repeat(3 * 1024 * 1024)), 413, "a small payload that inflates to 3 MB");
await refused("1." + "A".repeat(200 * 1024), 413, "a body over the cap");

const missing = await fetch(`${API}/v1/patches/zzzzzzzz`);
check(missing.status === 404, "an ID nobody made: 404");
const listing = await fetch(`${API}/v1/patches`);
check(listing.status === 404, "no listing of IDs");
const foreign = await fetch(`${API}/v1/patches/zzzzzzzz`, { headers: { Origin: "https://evil.example" } });
check(!foreign.headers.get("access-control-allow-origin"), "another site can't read answers");

// The eleventh link in a minute from one address
const small = (n) => payloadOf(JSON.stringify({ name: `n${n}`, version: 1, nodes: [], connections: [] }));
const statuses = [];
for (let n = 0; n < 11; n++) statuses.push((await post(small(n), `10.4.${run}.1`)).status);
check(statuses.slice(0, 10).every((s) => s === 200) && statuses[10] === 429, `rate limit: ${statuses.join(" ")}`);

// The garden. Curation runs against wrangler dev's local D1, as Chris's
// scripts do against the deployed one
const sql = (command) => execFileSync(process.execPath,
  ["node_modules/wrangler/bin/wrangler.js", "d1", "execute", "soba-links", "--local", "--command", command],
  { stdio: "pipe" });
const curate = (verb, id) => execFileSync(process.execPath, ["garden.mjs", verb, id, "--local"], { stdio: "pipe" });
const plantFrom = (fields, ip) =>
  fetch(`${API}/v1/plants`, { method: "POST", body: JSON.stringify(fields), headers: { "Content-Type": "text/plain", "CF-Connecting-IP": ip, Origin: "https://soba.chaps.dev" } });
// A patch no earlier run has made, so its plant is new
const fresh = (n) => payloadOf(JSON.stringify({ name: `garden ${run} ${n} ${Date.now()}`, version: 1, nodes: [], connections: [] }));
// Every query a URL of its own, so the 60 s edge cache never answers with an old page
let asked = 0;
const gardenBefore = async (before = Number.MAX_SAFE_INTEGER - run * 1000 - ++asked) => {
  const response = await fetch(`${API}/v1/garden?before=${before}`, { headers: { Origin: "https://soba.chaps.dev" } });
  return { response, page: response.ok ? await response.json() : null };
};
const listed = (page, id) => page.plants.some((p) => p.id === id);

const payload = fresh("round trip");
const planted = await plantFrom({ payload, title: "  First\u0007 light ‮ ", name: "Chris" }, `10.5.${run}.1`);
const { id: plantId, url: plantUrl } = await planted.json();
check(planted.status === 200 && plantUrl.endsWith(`/s/${plantId}`), `a plant: ${planted.status} ${plantUrl}`);
const asLink = await fetch(`${API}/v1/patches/${plantId}`);
check((await asLink.text()) === payload, "a plant is a short link, byte for byte");
const twice = await plantFrom({ payload, title: "A second title" }, `10.5.${run}.2`);
check(twice.status === 200 && (await twice.json()).id === plantId, "planting it again answers the same way");
check(!listed((await gardenBefore()).page, plantId), "a pending plant isn't listed");

curate("show", plantId);
const shown = await gardenBefore();
const entry = shown.page.plants.find((p) => p.id === plantId);
check(shown.response.headers.get("cache-control")?.includes("max-age=60"), "the garden is cached for a minute");
check(shown.response.headers.get("access-control-allow-origin") === "https://soba.chaps.dev", "the garden page may read it");
check(entry && entry.title === "First light" && entry.name === "Chris" && entry.payload === payload
  && entry.bytes === payload.length && entry.parent === null && Number.isInteger(entry.planted_at),
  `a shown plant is listed, its first title kept: ${JSON.stringify({ ...entry, payload: undefined })}`);
const cachedAgain = await fetch(`${API}/v1/garden?before=${Number.MAX_SAFE_INTEGER - run * 1000 - asked}`, { headers: { Origin: "https://evil.example" } });
check(cachedAgain.ok && !cachedAgain.headers.get("access-control-allow-origin"), "a cached page still keeps other sites out");
curate("hide", plantId);
check(!listed((await gardenBefore()).page, plantId), "a hidden plant isn't listed");

const child = await plantFrom({ payload: fresh("child"), title: "Second light", parent: plantId }, `10.5.${run}.3`);
const { id: childId } = await child.json();
curate("show", childId);
check(child.status === 200 && (await gardenBefore()).page.plants.find((p) => p.id === childId)?.parent === plantId,
  "a plant grows from its parent");

const plantRefused = async (fields, status, what) => {
  const response = await plantFrom(fields, `10.6.${run}.${asked++ % 250}`);
  check(response.status === status, `${what}: ${response.status} ${await response.text()}`);
};
await plantRefused({ payload: fresh("t"), title: "" }, 400, "no title");
await plantRefused({ payload: fresh("t"), title: " \u0000​ " }, 400, "a title of nothing but control characters");
await plantRefused({ payload: fresh("t"), title: "x".repeat(41) }, 400, "a 41-character title");
await plantRefused({ payload: fresh("t"), title: "ok", name: "y".repeat(25) }, 400, "a 25-character name");
await plantRefused({ payload: fresh("t"), title: "N 4 Z I beats" }, 400, "one of the worst words");
await plantRefused({ payload: fresh("t"), title: "ok", parent: "zzzzzzzz" }, 400, "an unknown parent");
await plantRefused({ payload: "1.AAAA", title: "ok" }, 400, "a damaged payload");
await plantRefused("not an object", 400, "JSON that isn't a plant");
const longest = await plantFrom({ payload: fresh("caps"), title: "é".repeat(40), name: "ü".repeat(24) }, `10.6.${run}.200`);
check(longest.status === 200, `a 40-character title and a 24-character name: ${longest.status}`);
const therapist = await plantFrom({ payload: fresh("words"), title: "The therapist's grapes" }, `10.6.${run}.201`);
check(therapist.status === 200, `a kind word that holds an unkind one: ${therapist.status}`);

// Taken down: never listed, and it can't be planted again
const doomed = await (await plantFrom({ payload: fresh("removed"), title: "Gone soon" }, `10.7.${run}.1`)).json();
curate("show", doomed.id);
sql(`UPDATE patches SET removed = 1 WHERE id = '${doomed.id}'`);
check(!listed((await gardenBefore()).page, doomed.id), "a removed patch is never listed");

// Paging: three plants, the newest first, and `before` starts after one
const three = [];
for (let n = 0; n < 3; n++) {
  const { id } = await (await plantFrom({ payload: fresh(`page ${n}`), title: `Row ${n}` }, `10.8.${run}.${n}`)).json();
  curate("show", id);
  three.push(id);
}
const top = (await gardenBefore()).page.plants;
check(top.slice(0, 3).map((p) => p.id).join() === [...three].reverse().join(), "the newest plants first");
const after = (await gardenBefore(top[0].planted_at)).page.plants;
check(after[0]?.id === three[1] && !listed({ plants: after }, three[2]), "before= pages on from a plant");
check(top.length <= 48 && new Set(top.map((p) => p.planted_at)).size === top.length, "no two plants share a planted_at");
check((await gardenBefore("soon")).response.status === 400, "before must be a time");

// Vines: a plant grown from a shared patch, two remixes of it, a remix of
// one of those, and a hidden remix whose own remix it hides too
const vineFrom = async (title, parent, n, show = true) => {
  const { id } = await (await plantFrom({ payload: fresh(`vine ${n}`), title, parent }, `10.9.${run}.${n}`)).json();
  if (show) curate("show", id);
  return id;
};
const { id: shared } = await (await post(fresh("shared"), `10.9.${run}.100`)).json();
const vineA = await vineFrom("Vine root", shared, 1);
const vineB = await vineFrom("Vine first remix", vineA, 2);
const vineC = await vineFrom("Vine remix of a remix", vineB, 3);
const vineD = await vineFrom("Vine second remix", vineA, 4);
const vineE = await vineFrom("Vine hidden remix", vineD, 5, false);
await vineFrom("Vine under the hidden", vineE, 6);
const vinePage = (await gardenBefore()).page.plants;
const vineRow = (id) => vinePage.find((p) => p.id === id);
check(vineRow(vineA)?.stems === 3 && vineRow(vineD)?.stems === 0 && vineRow(vineC)?.stems === 0,
  `stems count shown remixes, not past a hidden one: ${[vineA, vineB, vineC, vineD].map((id) => vineRow(id)?.stems)}`);
check(vineRow(vineB)?.parent_title === "Vine root" && vineRow(vineA)?.parent === shared && vineRow(vineA)?.parent_title === null,
  "a remix names the plant it grew from, and a shared patch has no title");
const lineageOf = async (id) => {
  const response = await fetch(`${API}/v1/plants/${id}/lineage`, { headers: { Origin: "https://soba.chaps.dev" } });
  return { response, vine: response.ok ? await response.json() : null };
};
const shape = (node) => node.children.length ? `${node.title}(${node.children.map(shape).join(",")})` : node.title;
const whole = await lineageOf(vineA);
check(whole.response.headers.get("access-control-allow-origin") === "https://soba.chaps.dev" && whole.vine.root.payload
  && shape(whole.vine.root) === "Vine root(Vine first remix(Vine remix of a remix),Vine second remix)" && !whole.vine.cut,
  `a root's lineage is everything grown from it: ${whole.vine && shape(whole.vine.root)}`);
const leaf = await lineageOf(vineC);
check(leaf.vine?.id === vineC && shape(leaf.vine.root) === "Vine root(Vine first remix(Vine remix of a remix))",
  `a remix's lineage is its line back to the root: ${leaf.vine && shape(leaf.vine.root)}`);
check((await lineageOf(vineE)).response.status === 404, "a hidden plant has no lineage");
check((await lineageOf(shared)).response.status === 404, "a short link that isn't a plant has no lineage");
// A loop: X planted from the link Y, then Y planted from X
const loopY = fresh("loop y");
const { id: loopYId } = await (await post(loopY, `10.9.${run}.101`)).json();
const loopX = await vineFrom("Loop X", loopYId, 7);
const { id: loopYPlant } = await (await plantFrom({ payload: loopY, title: "Loop Y", parent: loopX }, `10.9.${run}.8`)).json();
curate("show", loopYPlant);
const loop = await lineageOf(loopX);
check(loop.response.ok && shape(loop.vine.root) === "Loop Y(Loop X)", `a loop ends where it comes round: ${loop.vine && shape(loop.vine.root)}`);

console.log(failures ? `${failures} failed` : "all passed");
process.exit(failures ? 1 : 0);
