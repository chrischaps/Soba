// The garden's curation, against the deployed D1 (--local for wrangler dev's):
//   node garden.mjs pending        the plants waiting, with links to listen to
//   node garden.mjs review         each waiting plant in turn: show, hide or leave it
//   node garden.mjs show <id> …    list them in the garden
//   node garden.mjs hide <id> …    take them out again (their short links still work)
// `npm run garden:pending` and the rest wrap these. A hidden plant can be
// shown again. To take a patch down altogether, set `patches.removed`.
import { execFileSync } from "node:child_process";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const [command, ...rest] = process.argv.slice(2);
const where = rest.includes("--local") ? "--local" : "--remote";
const ids = rest.filter((arg) => !arg.startsWith("--"));
const ID = /^[0-9A-Za-z]{6,16}$/;

const PENDING = `SELECT plants.id, title, name, parent, datetime(planted_at / 1000, 'unixepoch') AS planted,
              'https://soba.chaps.dev/s/' || plants.id AS listen
            FROM plants JOIN patches ON patches.id = plants.id
            WHERE status = 'pending' AND removed = 0 ORDER BY planted_at`;
const sql = {
  pending: PENDING,
  review: PENDING,
  show: setStatus("shown", ids),
  hide: setStatus("hidden", ids),
}[command];
const takesIds = command === "show" || command === "hide";
if (!sql || (takesIds && (ids.length === 0 || !ids.every((id) => ID.test(id))))) {
  console.error("usage: node garden.mjs pending | review | show <id> … | hide <id> …  [--local]");
  process.exit(2);
}
if (command === "review") await review();
else wrangler(sql);

/// Runs `sql` through wrangler: its table on the terminal, or with `json`,
/// the first statement's rows. Wrangler runs through node rather than a
/// shell, which would mangle the quotes
function wrangler(sql, json = false) {
  const bin = fileURLToPath(new URL("./node_modules/wrangler/bin/wrangler.js", import.meta.url));
  const args = [bin, "d1", "execute", "soba-links", where, "--command", sql.replace(/\s+/g, " ")];
  if (!json) return execFileSync(process.execPath, args, { stdio: "inherit" });
  const out = execFileSync(process.execPath, [...args, "--json"], { stdio: ["inherit", "pipe", "inherit"] });
  return JSON.parse(out)[0].results;
}

/// Sets each plant in `list` to `status`, and lists them as they are now.
/// The IDs are checked against `ID` first, so they're safe inside the SQL
function setStatus(status, list) {
  const quoted = list.map((id) => `'${id}'`).join(", ");
  return `UPDATE plants SET status = '${status}' WHERE id IN (${quoted}); SELECT id, title, status FROM plants WHERE id IN (${quoted})`;
}

/// Walks the waiting plants oldest first, asking of each: show, hide, or
/// leave it waiting. The answers go in together at the end (or at `q`): one
/// UPDATE for those shown and one for those hidden, however many there were
async function review() {
  // The worker makes every ID, but they go into SQL, so they're checked as `show`'s are
  const plants = wrangler(PENDING, true).filter((p) => ID.test(p.id));
  if (plants.length === 0) return console.log("Nothing waits for the garden.");
  const titles = new Map(plants.map((p) => [p.id, p.title]));
  const chosen = { shown: [], hidden: [] };
  // Read line by line rather than with `question`, which drops lines piped in
  // before it asks. The input ending stops, as `q` does
  const lines = createInterface({ input: process.stdin })[Symbol.asyncIterator]();
  const ask = async () => {
    process.stdout.write("     s/h/enter/q? ");
    const { value, done } = await lines.next();
    if (!process.stdin.isTTY) console.log(done ? "" : value);
    return done ? "q" : value.trim().toLowerCase();
  };
  console.log(`${plants.length} waiting. For each: s show, h hide, enter leaves it waiting, q stops here.\n`);
  for (const [i, p] of plants.entries()) {
    const from = !p.parent ? "" : titles.has(p.parent) ? `, grown from "${titles.get(p.parent)}" (waiting too)`
      : `, grown from https://soba.chaps.dev/s/${p.parent}`;
    console.log(`${i + 1}/${plants.length}  "${p.title}"${p.name ? ` by ${p.name}` : ""}, planted ${p.planted} UTC${from}`);
    console.log(`     listen: ${p.listen}`);
    let answer;
    while (!["s", "h", "", "q"].includes((answer = await ask())));
    if (answer === "q") break;
    if (answer === "s") chosen.shown.push(p.id);
    if (answer === "h") chosen.hidden.push(p.id);
  }
  lines.return();
  for (const status of ["shown", "hidden"]) {
    if (chosen[status].length === 0) continue;
    console.log(`\n${status}: ${chosen[status].length}`);
    wrangler(setStatus(status, chosen[status]));
  }
  const left = plants.length - chosen.shown.length - chosen.hidden.length;
  console.log(`\n${left === 0 ? "Nothing" : left} left waiting.`);
}
