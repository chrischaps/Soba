// Short patch links (#370) and the Patch Garden's seed bed (#388). Keeps
// the payload of a patch link (`1.<data>`, what follows `#p=`) under a short
// ID, and hands it back. A plant is a short link with a title.
//
//   POST /v1/patches        body: 1.<data>                       ->  { id, url }
//   GET  /v1/patches/<id>                                        ->  1.<data>
//   POST /v1/plants         body: {payload, title, name?, parent?} ->  { id, url }
//   GET  /v1/garden?before=<planted_at>                          ->  { plants, before }
//
// The ID is content-addressed: the SHA-256 of the patch's JSON, in base62,
// cut to 8 characters (longer only if two patches ever share a prefix). So
// sharing the same patch twice gives the same link and stores it once, and
// what's stored never changes, which lets every GET be cached for good.
// The answer is 200 either way, so it doesn't say which.
//
// The short link itself is soba.chaps.dev/s/<id>, a static page on GitHub
// Pages that the app resolves through this API. The API's address is never
// shown to anyone, so it can move without breaking a link.

const ALLOWED_ORIGINS = ["https://soba.chaps.dev", "http://127.0.0.1:8080", "http://localhost:8080"];
const LINK_BASE = "https://soba.chaps.dev/s/";
// The largest example (Canopy) deflates to about 25 KB, 34 K characters
const MAX_BODY = 96 * 1024;
// What a patch may inflate to. The largest example is under 300 KB
const MAX_JSON = 2 * 1024 * 1024;
const ID_LENGTH = 8;
const ID = /^[0-9A-Za-z]{6,16}$/;
const TOO_LARGE = "that patch is too large for a short link";
// A plant's JSON is its payload and a few short words
const MAX_PLANT_BODY = MAX_BODY + 1024;
const TITLE_LENGTH = 40;
const NAME_LENGTH = 24;
const GARDEN_PAGE = 48;
const GARDEN_CACHE_SECONDS = 60;

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const cors = corsHeaders(request);
    if (request.method === "OPTIONS") return new Response(null, { status: 204, headers: cors });
    try {
      if (url.pathname === "/v1/patches" && request.method === "POST") return await create(request, env, cors);
      const match = url.pathname.match(/^\/v1\/patches\/([^/]+)$/);
      if (match && request.method === "GET") return await fetchPatch(match[1], env, cors);
      if (url.pathname === "/v1/plants" && request.method === "POST") return await plant(request, env, cors);
      if (url.pathname === "/v1/garden" && request.method === "GET") return await garden(url, env, cors);
      return text("not found", 404, cors);
    } catch (e) {
      return text("the link service failed", 500, cors);
    }
  },
};

async function create(request, env, cors) {
  if (await rateLimited(request, env)) return text(RATE_LIMITED, 429, cors);
  const body = await readBody(request, MAX_BODY);
  if (body === null) return text(TOO_LARGE, 413, cors);
  const stored = await store(body.trim(), env);
  if (stored.refused) return text(stored.refused, stored.status, cors);
  return created(stored.id, cors);
}

const RATE_LIMITED = "too many links at once: try again in a minute";

/// Whether this address has made its ten links this minute. Plants count too.
async function rateLimited(request, env) {
  if (!env.CREATE_LIMIT) return false;
  const ip = request.headers.get("CF-Connecting-IP") ?? "local";
  const { success } = await env.CREATE_LIMIT.limit({ key: ip });
  return !success;
}

/// The body as text, or null if it's over `cap`.
async function readBody(request, cap) {
  const declared = Number(request.headers.get("Content-Length") ?? 0);
  if (declared > cap) return null;
  const body = await request.text();
  return body.length > cap ? null : body;
}

/// Keeps a payload under its short ID: `{ id, removed }`, or
/// `{ refused, status }` saying why it can't.
async function store(body, env) {
  if (body.length > MAX_BODY) return { refused: TOO_LARGE, status: 413 };
  const json = await inflatePayload(body);
  if (json === TOO_LARGE) return { refused: json, status: 413 };
  if (typeof json === "string") return { refused: json, status: 400 };
  const why = notAPatch(json.text);
  if (why) return { refused: why, status: 400 };

  const hash = base62(await sha256(json.bytes));
  const known = await env.DB.prepare("SELECT id, removed FROM patches WHERE hash = ?").bind(hash).first();
  if (known) return { id: known.id, removed: !!known.removed };

  // Eight characters, unless another patch already holds them
  for (let length = ID_LENGTH; length <= hash.length; length++) {
    const id = hash.slice(0, length);
    const result = await env.DB.prepare(
      "INSERT INTO patches (id, hash, payload, bytes, created_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(id) DO NOTHING",
    ).bind(id, hash, body, body.length, Date.now()).run();
    if (result.meta.changes === 1) return { id, removed: false };
  }
  throw new Error("no free ID");
}

async function fetchPatch(id, env, cors) {
  if (!ID.test(id)) return text("not found", 404, cors);
  const row = await env.DB.prepare("SELECT payload, removed FROM patches WHERE id = ?").bind(id).first();
  if (!row) return text("no patch has that link", 404, cors);
  if (row.removed) return text("this patch was taken down", 410, cors);
  return new Response(row.payload, {
    headers: { ...cors, "Content-Type": "text/plain; charset=utf-8", "Cache-Control": "public, max-age=31536000, immutable" },
  });
}

/// Plants a patch: stores it if it's new, and gives it a title, a name and
/// the short link it grew from. A new plant waits, `pending`, until it's
/// shown (`npm run garden:show`): the garden is curated. Planting the same
/// patch again answers the same way and changes nothing.
async function plant(request, env, cors) {
  if (await rateLimited(request, env)) return text(RATE_LIMITED, 429, cors);
  const body = await readBody(request, MAX_PLANT_BODY);
  if (body === null) return text(TOO_LARGE, 413, cors);
  let fields;
  try {
    fields = JSON.parse(body);
  } catch {
    return text("a plant is JSON: {payload, title, name?, parent?}", 400, cors);
  }
  if (!fields || typeof fields !== "object" || typeof fields.payload !== "string") {
    return text("a plant is JSON: {payload, title, name?, parent?}", 400, cors);
  }
  const title = words(fields.title);
  if (title === null || title.length === 0) return text("a plant needs a title", 400, cors);
  if ([...title].length > TITLE_LENGTH) return text(`a title is ${TITLE_LENGTH} characters at most`, 400, cors);
  const name = words(fields.name ?? "");
  if (name === null) return text("a name is text", 400, cors);
  if ([...name].length > NAME_LENGTH) return text(`a name is ${NAME_LENGTH} characters at most`, 400, cors);
  if (unkind(title) || unkind(name)) return text("please choose other words", 400, cors);

  const parent = fields.parent ?? null;
  if (parent !== null) {
    const known = typeof parent === "string" && ID.test(parent)
      && await env.DB.prepare("SELECT 1 FROM patches WHERE id = ?").bind(parent).first();
    if (!known) return text("the patch it grew from isn't one this service knows", 400, cors);
  }

  const stored = await store(fields.payload.trim(), env);
  if (stored.refused) return text(stored.refused, stored.status, cors);
  if (stored.removed) return text("this patch was taken down", 410, cors);
  if (parent === stored.id) return text("a patch can't grow from itself", 400, cors);

  // Each plant a moment after the last, so no two share a `planted_at`
  // and paging by `before` never skips one
  await env.DB.prepare(
    `INSERT INTO plants (id, title, name, parent, planted_at)
     VALUES (?, ?, ?, ?, MAX(?, COALESCE((SELECT MAX(planted_at) FROM plants), 0) + 1))
     ON CONFLICT(id) DO NOTHING`,
  ).bind(stored.id, title, name || null, parent, Date.now()).run();
  return created(stored.id, cors);
}

/// The newest shown plants planted before `before`, with their payloads, so
/// the garden page can draw and play them without asking again. `before` in
/// the answer is where the next page starts, or null at the oldest.
async function garden(url, env, cors) {
  const asked = url.searchParams.get("before");
  if (asked !== null && !/^[0-9]{1,16}$/.test(asked)) return text("before is a planted_at", 400, cors);
  const before = asked === null ? Number.MAX_SAFE_INTEGER : Number(asked);

  // Cached without CORS headers, which are added for whoever asks
  const key = new Request(`${url.origin}/v1/garden?before=${asked ?? ""}`);
  const cache = typeof caches === "undefined" ? null : caches.default;
  let answer = cache && await cache.match(key);
  if (!answer) {
    const { results } = await env.DB.prepare(
      `SELECT plants.id, title, name, planted_at, bytes, parent, payload
       FROM plants JOIN patches ON patches.id = plants.id
       WHERE status = 'shown' AND removed = 0 AND planted_at < ?
       ORDER BY planted_at DESC LIMIT ?`,
    ).bind(before, GARDEN_PAGE).all();
    const next = results.length === GARDEN_PAGE ? results[results.length - 1].planted_at : null;
    answer = new Response(JSON.stringify({ plants: results, before: next }), {
      headers: { "Content-Type": "application/json", "Cache-Control": `public, max-age=${GARDEN_CACHE_SECONDS}` },
    });
    if (cache) await cache.put(key, answer.clone());
  }
  const headers = new Headers(answer.headers);
  for (const [name, value] of Object.entries(cors)) headers.set(name, value);
  return new Response(answer.body, { headers });
}

/// A title or name as it's kept: trimmed, with control and formatting
/// characters (bidi overrides, zero-width joiners) taken out and runs of
/// spaces made one. Null if it isn't text.
function words(value) {
  if (typeof value !== "string") return null;
  return value.replace(/[\p{Cc}\p{Cf}]/gu, "").replace(/\s+/gu, " ").trim();
}

// The worst words, which no title or name in the garden may hold. The
// garden is curated anyway: this only spares the curator
const UNKIND = [
  "nigger", "nigga", "faggot", "fag", "kike", "spic", "chink", "gook", "wetback",
  "tranny", "retard", "cunt", "rape", "rapist", "nazi", "hitler", "kkk", "heil",
];
const LOOKALIKES = { 0: "o", 1: "i", 3: "e", 4: "a", 5: "s", 7: "t", "@": "a", $: "s", "!": "i" };

/// Whether `value` holds one of the worst words: as a word, plural, or
/// spelled out a letter at a time ("n a z i"). Never inside another word,
/// so "therapist" and "grape" are fine.
function unkind(value) {
  const plain = value.toLowerCase().normalize("NFKD").replace(/\p{M}/gu, "")
    .replace(/[0-9@$!]/g, (c) => LOOKALIKES[c] ?? c);
  const tokens = [];
  let spelled = "";
  for (const token of plain.split(/[^a-z]+/).filter(Boolean)) {
    if (token.length === 1) {
      spelled += token;
      continue;
    }
    if (spelled) tokens.push(spelled);
    spelled = "";
    tokens.push(token);
  }
  if (spelled) tokens.push(spelled);
  return tokens.some((token) => UNKIND.includes(token) || (token.endsWith("s") && UNKIND.includes(token.slice(0, -1))));
}

/// The JSON a link's payload holds, as bytes and text, or why it holds none.
/// The same steps as `patch_link::patch_from_fragment`.
async function inflatePayload(body) {
  const dot = body.indexOf(".");
  if (dot < 0) return "the link is cut short";
  if (body.slice(0, dot) !== "1") return "the link isn't one this service knows";
  let deflated;
  try {
    deflated = base64url(body.slice(dot + 1));
  } catch {
    return "the link is damaged";
  }
  const reader = new Blob([deflated]).stream().pipeThrough(new DecompressionStream("deflate-raw")).getReader();
  const chunks = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      // A small payload that inflates hugely is refused before it's all read
      if (size > MAX_JSON) {
        await reader.cancel();
        return TOO_LARGE;
      }
      chunks.push(value);
    }
  } catch {
    return "the link is damaged";
  }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.length;
  }
  return { bytes, text: new TextDecoder("utf-8", { fatal: true }).decode(bytes) };
}

/// Why `text` isn't a Soba patch, or nothing if it looks like one. Only
/// its shape: the app checks the rest when it opens it.
function notAPatch(text) {
  let patch;
  try {
    patch = JSON.parse(text);
  } catch {
    return "that isn't a Soba patch";
  }
  const shaped = patch && typeof patch === "object" && !Array.isArray(patch)
    && typeof patch.name === "string" && Number.isInteger(patch.version)
    && Array.isArray(patch.nodes) && Array.isArray(patch.connections);
  return shaped ? null : "that isn't a Soba patch";
}

/// The same answer whether the patch is new or was shared before, so a POST
/// can't be used to ask whether someone shared a given patch.
function created(id, cors) {
  return new Response(JSON.stringify({ id, url: LINK_BASE + id }), {
    headers: { ...cors, "Content-Type": "application/json" },
  });
}

function text(message, status, cors) {
  return new Response(message, { status, headers: { ...cors, "Content-Type": "text/plain; charset=utf-8" } });
}

/// Only the app's own pages may read the answers from a browser.
function corsHeaders(request) {
  const origin = request.headers.get("Origin");
  if (!origin || !ALLOWED_ORIGINS.includes(origin)) return {};
  return {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": "GET, POST",
    "Access-Control-Allow-Headers": "Content-Type",
    "Access-Control-Max-Age": "86400",
    Vary: "Origin",
  };
}

async function sha256(bytes) {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
}

const DIGITS = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The first 16 bytes of `bytes` in base62: 22 characters, least significant
/// first, so a prefix is as random as the hash (the top digit only runs to 7).
function base62(bytes) {
  let n = 0n;
  for (const byte of bytes.slice(0, 16)) n = (n << 8n) | BigInt(byte);
  let out = "";
  for (let i = 0; i < 22; i++) {
    out += DIGITS[Number(n % 62n)];
    n /= 62n;
  }
  return out;
}

function base64url(data) {
  const plain = data.replace(/\s/g, "").replace(/=+$/, "").replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(plain + "=".repeat((4 - (plain.length % 4)) % 4));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}
