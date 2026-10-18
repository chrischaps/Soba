// The garden's curation, against the deployed D1 (--local for wrangler dev's):
//   node garden.mjs pending        the plants waiting, with links to listen to
//   node garden.mjs show <id> …    list them in the garden
//   node garden.mjs hide <id> …    take them out again (their short links still work)
// `npm run garden:pending` and the rest wrap these. A hidden plant can be
// shown again. To take a patch down altogether, set `patches.removed`.
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const [command, ...rest] = process.argv.slice(2);
const where = rest.includes("--local") ? "--local" : "--remote";
const ids = rest.filter((arg) => !arg.startsWith("--"));
const ID = /^[0-9A-Za-z]{6,16}$/;

const sql = {
  pending: `SELECT plants.id, title, name, parent, datetime(planted_at / 1000, 'unixepoch') AS planted,
              'https://soba.chaps.dev/s/' || plants.id AS listen
            FROM plants JOIN patches ON patches.id = plants.id
            WHERE status = 'pending' AND removed = 0 ORDER BY planted_at`,
  show: setStatus("shown"),
  hide: setStatus("hidden"),
}[command];
if (!sql || (command !== "pending" && (ids.length === 0 || !ids.every((id) => ID.test(id))))) {
  console.error("usage: node garden.mjs pending | show <id> … | hide <id> …  [--local]");
  process.exit(2);
}
// The IDs are checked above, so they're safe inside the SQL. Wrangler runs
// through node rather than a shell, which would mangle the quotes
const wrangler = fileURLToPath(new URL("./node_modules/wrangler/bin/wrangler.js", import.meta.url));
execFileSync(process.execPath, [wrangler, "d1", "execute", "soba-links", where, "--command", sql.replace(/\s+/g, " ")], {
  stdio: "inherit",
});

/// Sets each plant in `ids` to `status`, and lists them as they are now.
function setStatus(status) {
  const list = ids.map((id) => `'${id}'`).join(", ");
  return `UPDATE plants SET status = '${status}' WHERE id IN (${list}); SELECT id, title, status FROM plants WHERE id IN (${list})`;
}
