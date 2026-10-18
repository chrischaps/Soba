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

-- The Patch Garden (#388): a plant is a short link someone chose to
-- publish, with a title. Planting the same patch twice changes nothing.
-- A new plant is `pending` until Chris shows it (`npm run garden:show`).
CREATE TABLE IF NOT EXISTS plants (
  id         TEXT PRIMARY KEY REFERENCES patches(id),
  title      TEXT NOT NULL,                   -- 1-40 characters
  name       TEXT,                            -- who planted it, if they said: 0-24
  parent     TEXT REFERENCES patches(id),     -- the short link it grew from
  planted_at INTEGER NOT NULL,                -- milliseconds since 1970, one plant to each
  status     TEXT NOT NULL DEFAULT 'pending'  -- pending, shown or hidden
);
CREATE INDEX IF NOT EXISTS plants_by_status ON plants (status, planted_at);
CREATE INDEX IF NOT EXISTS plants_by_parent ON plants (parent);
