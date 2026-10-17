// Drives a running `npx wrangler dev` (http://127.0.0.1:8787) through the
// short-link API: every example in, back out byte for byte, and the refusals.
//   node spike_test.mjs ../../patches
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

console.log(failures ? `${failures} failed` : "all passed");
process.exit(failures ? 1 : 0);
