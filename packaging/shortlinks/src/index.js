// Short patch links (#370): a spike. Keeps the payload of a patch link
// (`1.<data>`, what follows `#p=`) under a short ID, and hands it back.
//
//   POST /v1/patches        body: 1.<data>  ->  { id, url }
//   GET  /v1/patches/<id>                   ->  1.<data>
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

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const cors = corsHeaders(request);
    if (request.method === "OPTIONS") return new Response(null, { status: 204, headers: cors });
    try {
      if (url.pathname === "/v1/patches" && request.method === "POST") return await create(request, env, cors);
      const match = url.pathname.match(/^\/v1\/patches\/([^/]+)$/);
      if (match && request.method === "GET") return await fetchPatch(match[1], env, cors);
      return text("not found", 404, cors);
    } catch (e) {
      return text("the link service failed", 500, cors);
    }
  },
};

async function create(request, env, cors) {
  const ip = request.headers.get("CF-Connecting-IP") ?? "local";
  if (env.CREATE_LIMIT) {
    const { success } = await env.CREATE_LIMIT.limit({ key: ip });
    if (!success) return text("too many links at once: try again in a minute", 429, cors);
  }
  const declared = Number(request.headers.get("Content-Length") ?? 0);
  if (declared > MAX_BODY) return text(TOO_LARGE, 413, cors);
  const body = (await request.text()).trim();
  if (body.length > MAX_BODY) return text(TOO_LARGE, 413, cors);

  const json = await inflatePayload(body);
  if (json === TOO_LARGE) return text(json, 413, cors);
  if (typeof json === "string") return text(json, 400, cors);
  const why = notAPatch(json.text);
  if (why) return text(why, 400, cors);

  const hash = base62(await sha256(json.bytes));
  const known = await env.DB.prepare("SELECT id FROM patches WHERE hash = ?").bind(hash).first();
  if (known) return created(known.id, cors);

  // Eight characters, unless another patch already holds them
  for (let length = ID_LENGTH; length <= hash.length; length++) {
    const id = hash.slice(0, length);
    const result = await env.DB.prepare(
      "INSERT INTO patches (id, hash, payload, bytes, created_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(id) DO NOTHING",
    ).bind(id, hash, body, body.length, Date.now()).run();
    if (result.meta.changes === 1) return created(id, cors);
  }
  return text("the link service failed", 500, cors);
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
