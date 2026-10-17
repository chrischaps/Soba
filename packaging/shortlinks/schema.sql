-- One row per distinct patch. `payload` is exactly what follows `#p=` in
-- the long link, so a short link always turns back into a long one.
CREATE TABLE IF NOT EXISTS patches (
  id         TEXT PRIMARY KEY,           -- the short ID: a prefix of `hash`
  hash       TEXT NOT NULL UNIQUE,       -- SHA-256 of the patch's JSON, base62
  payload    TEXT NOT NULL,              -- 1.<data>
  bytes      INTEGER NOT NULL,
  created_at INTEGER NOT NULL,           -- milliseconds since 1970
  removed    INTEGER NOT NULL DEFAULT 0  -- taken down: answers 410, never reused
);
