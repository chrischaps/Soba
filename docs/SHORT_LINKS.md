# Short patch links

*The design from #370, 2026-10-17, built in #376. WEB.md says how it runs.*

A patch link (#303) holds the whole patch in the address's fragment:
`soba.chaps.dev/#p=1.<data>`. That runs from 1.1 K characters (First Sound)
to 33 K (Canopy). The long ones look like noise in a chat, some apps cut them
off, and they don't fit on a slide or in a QR code. A short link looks like this:

```
https://soba.chaps.dev/s/BxdWNBJJ        33 characters, a 29×29 QR code
```

## The shape

```
 Share ──POST 1.<data>──▶  Worker (soba-links)  ──▶  D1: id → 1.<data>
   ◀── { id, url } ──────
 soba.chaps.dev/s/<id>   GitHub Pages' 404.html: location.replace("/#s=<id>")
 soba.chaps.dev/#s=<id>  the app: GET /v1/patches/<id> → 1.<data>
                         → patch_link::patch_from_fragment("p=" + it)
```

- **The visible link is static, on our domain.** `soba.chaps.dev/s/<id>`
  matches no file, so GitHub Pages serves `404.html`. That page turns the
  address into `/#s=<id>` and the app takes over. It works the way `#p=` does today. The
  app fetches the payload, opens it as it would open a long link, and takes the fragment out of the
  address. The short link stays short all the way, and the patch never shows
  in the address bar.
- **The store is a Cloudflare Worker with D1**, at `soba-links.<account>.workers.dev`.
  Nobody ever sees that address. Only the app calls it, so it can move (to
  `api.soba.chaps.dev`, say, if chaps.dev's DNS ever moves from Porkbun to
  Cloudflare) without breaking a single link.
- **What's stored is the long link's payload**, `1.<data>`, byte for byte.
  So the app needs no new decoder, the version rule carries over unchanged, and
  any short link can always be turned back into a long one.

The service is `packaging/shortlinks/`: the Worker, its schema, and a test that
runs against `npx wrangler dev`. It's deployed at `soba-links.chaps.workers.dev`.
Every example goes in and comes back out byte for byte. Sharing the same patch a
second time gives the same ID, and each refusal below answers as it should.

## Decisions

**Hosting: Cloudflare Workers + D1.** Pricing as of writing (check it before deploying):

| | At zero traffic | Cold start | What there is to run |
|---|---|---|---|
| **Workers + D1** | Free: 100 K requests/day, 100 K row writes/day, 5 GB | None worth measuring (isolates) | ~200 lines of JS, no server, no OS to patch |
| Workers + KV | Free, but only 1 K writes/day | None | The same, but KV can't grow into a gallery's queries |
| Fly.io | Pay-as-you-go, cents | 1–3 s from zero | A container and its image |
| Render free | Free | ~1 minute after 15 idle minutes | A service |
| Supabase free | Free | Pauses after a week idle | Dead links after a quiet week rule it out |

D1 wins over KV because a gallery ("Patch Garden") needs queries, and KV's write
cap is low enough for one person to use up.

**Serve, not redirect.** A 302 to the long link would put the 33 K
characters back in the address bar, and it would need the Worker on
soba.chaps.dev itself, which means moving DNS. Fetching the patch keeps the
link short and costs one CORS rule. The POST sends `text/plain`, which makes
it a simple request with no preflight. Answers are readable only from
soba.chaps.dev and `trunk serve`'s localhost.

**IDs are content-addressed.** The ID is the SHA-256 of the patch's JSON, in base62,
cut to 8 characters (about 48 bits). In the rare case two patches share those 8, it runs to 9 and on up.
Sharing a patch twice gives the same link and stores it once. Since a stored patch
never changes, the GET is cached as `immutable` for a year. Hashing the
*inflated* JSON means a change of deflate settings doesn't change the link.

Anyone holding the patch can work out its link. That's fine: having the patch
means having more than the link gives, and 48 bits can't be guessed through a rate
limit. The one leak is that someone holding an exact patch can test
whether it was ever shared. If that ever matters, an HMAC with a server
secret closes it and keeps the deduplication.

**Limits and abuse:**
- The body is capped at 96 KB, about three times Canopy's payload. The JSON may inflate
  to 2 MB at most, checked while it streams, so a zip bomb is refused before it's read.
- The payload must decode as JSON shaped like a patch (`name`, `version`,
  `nodes`, `connections`). The app checks everything else when it opens a patch,
  as it already does for any hostile long link.
- Rate limit: 10 new links a minute per IP (the Workers rate-limiting binding).
- No listing. A missing ID gets 404, and there's no endpoint that names IDs.
- **Every failure ends in the long link.** A refusal, a 429, a used-up
  daily quota or an outage all give the person the long link with a quiet
  word, never an error in their way. So the worst abuse can do is make links long again.

**No expiry, no self-deletion.** Links end up in docs, slides and printed QR
codes, so they last. With content addressing, one person's delete would
break another person's identical link, and with no accounts there's no
owner to ask anyway. Takedown is manual: Chris sets `removed`, the ID answers
410 ("this patch was taken down") and is never reused. Someone who wants
nothing stored uses the long link.

## Privacy, where the person shares

Share copies the short link. The note says what that means and always keeps the
long link on offer:

> **Link copied**
> Paste it anywhere. Whoever opens it hears this patch in their browser, and can play with it, with nothing to install.
> *To keep the link short, the patch is stored on soba.chaps.dev, where anyone with the link can open it.*
> **[Copy the long link instead]**: *it carries the whole patch, so nothing is stored, and it works offline.*

If the service didn't answer:

> **Link copied**
> … *This is the long link, which holds the whole patch: the short-link service didn't answer.*

The manual's privacy line and WEB.md get the same two sentences. Patches that
play the user's own files still can't go by link of either kind.

Planting (#396) says the other half before anything is sent: sharing stores a
patch behind a link, and planting publishes it, for anyone to hear and remix
once it's approved.

## Desktop

Share posts with `ureq` (already a dependency for updates) on a thread, with a
3-second timeout, so the frame never waits. While it's out, the note says
"Making a link…". If it fails or times out, the long link is copied instead. Opening a
`soba.chaps.dev/s/<id>` pasted onto the desktop canvas fetches it the same way.

## Durability

If the service ever shuts down, the short links die with it unless they're carried over:
- **Every Sunday the whole database goes to a private repo**,
  [`chrischaps/soba-links-archive`](https://github.com/chrischaps/soba-links-archive),
  as one SQL file, `soba-links.sql`: the `patches` table, the garden's `plants`
  and whatever comes later. The workflow is `.github/workflows/links-archive.yml`
  (#389). It runs `wrangler d1 export soba-links --remote` and commits the dump only when it
  changed, so the archive's history is a week-by-week record. The dump goes straight into the
  archive's checkout and the log shows only its size and row counts. It never reaches this
  public repo. **Run workflow** on the Actions tab takes a backup at any time.
- **What it needs** (secrets on this repo):
  - `LINKS_ARCHIVE_DEPLOY_KEY`: the private half of an SSH key whose public half is a
    deploy key, with write access, on the archive repo. Create the repo with a README, so it has a
    branch to check out.
  - `CLOUDFLARE_API_TOKEN`: a token for the account holding `soba-links`, with
    **Account › D1 › Edit**. The export API takes a POST, and a read-only D1 token may be refused.
    A D1-only token can't list accounts, so `wrangler.toml` names the account (`account_id`).

  Until both exist, the run leaves a warning and stops. With only one of them it fails.
- **Restoring** into a new database, from `packaging/shortlinks/`:
  ```
  npx wrangler d1 create soba-links        # then put its database_id in wrangler.toml
  npx wrangler d1 execute soba-links --remote --file path/to/soba-links-archive/soba-links.sql
  npx wrangler deploy
  ```
  The dump holds its own `CREATE TABLE`s, so it goes into an empty database. Every row keeps
  its ID, so every link answers as it did.
- If the service has to go, the archive is written out as static files,
  `soba.chaps.dev/s/<id>.txt`, and the app tries that same-origin path
  when the API doesn't answer. Every link keeps working on GitHub Pages alone.
  (The public repo would list the IDs then. That's acceptable as a last resort, but not before.)
- The long link stays authoritative: `p=1.` is the format, and a short link is
  only a name for one.

## Patch Garden

*The seed bed, #388, the page, #393, listening, #394, and vines, #397. Planting
from the app comes later (`docs/Patch Garden — Proposal.md`).*

**The page** is `packaging/web/garden/`, published at `soba.chaps.dev/garden/`
by `docs.yml`. It pages through `GET /v1/garden`, inflates each payload and
draws the patch as an SVG cable diagram from `modules.json` (#390), skipping any
it can't read. A card says what it was grown from and how many remixes grew
from it, and either opens its vine (`vine.js`). To try it against `wrangler dev`, serve `packaging/web` at
`127.0.0.1:8080` and open `/garden/?api=http://127.0.0.1:8787` (`?api=` works
only on a local page).

**Listening** (#394, `listen.js`): resting the pointer on a card for a moment
plays it through one hidden listener, `/?listen` (#391, `WEB.md`); moving to
another card fades across, and leaving the cards fades out. A tap plays and
stops a card on a phone, and a click does for heavy cards everywhere. A ring
round the sounding card follows its level, and if the browser holds the
sound back, "Tap to let the garden play" asks once a visit. Locally, add
`&app=http://127.0.0.1:8081` with a `trunk serve --port 8081` to listen with.

A plant is a short link with a title. **Planting publishes a patch; sharing
still never does.** Only `POST /v1/plants` puts a patch in the garden.

```sql
plants(id      TEXT PRIMARY KEY REFERENCES patches(id),
       title   TEXT NOT NULL,                  -- 1–40 characters
       name    TEXT,                           -- 0–24, optional
       parent  TEXT REFERENCES patches(id),    -- the short link it grew from
       planted_at INTEGER NOT NULL,            -- ms, one plant to each
       status  TEXT NOT NULL DEFAULT 'pending')  -- pending, shown, hidden
```

- **`POST /v1/plants`**: a JSON body, `{payload, title, name?, parent?}`, sent as
  `text/plain` so there's no preflight. It goes through every check a short link
  does (the 96 KB body, the 2 MB inflate, the shape, the same ID) and shares the
  10-a-minute rate limit. Titles and names are trimmed, with control and
  formatting characters taken out, and a small word list refuses the worst. A
  `parent` must be a short link the service knows, planted or not. It answers
  `{id, url}`, and planting the same patch again answers the same way and
  changes nothing: the first title stays. A patch that was taken down can't
  be planted (410).
- **`GET /v1/garden?before=<planted_at>`**: the 48 newest shown plants planted
  before `before` (or the newest of all), each
  `{id, title, name, planted_at, bytes, parent, payload, stems, parent_title}`,
  and `before` for the
  next page (null at the end). Pages are cached at the edge for 60 s. A patch
  that was taken down is never listed. Each plant is planted at least a
  millisecond after the last, so paging never skips one. Anything that seeds
  the table by hand should keep `planted_at` distinct too. `stems` counts the
  shown plants grown from it, and from those, but never past a hidden one.
  `parent_title` is the parent's title if the parent is a shown plant, and
  null if it's a shared link that isn't (or a plant that isn't shown).
- **`GET /v1/plants/<id>/lineage`**: a shown plant's vine, `{id, root, cut}`.
  `root` is the oldest shown plant it grew from, and each node is a garden row
  with its `children`, oldest first: down the line to the plant, then
  everything grown from it. Only shown plants, so a hidden one ends its branch.
  It reaches 32 generations up or down and holds 48 remixes (`cut` says there
  were more). A loop, which two patches planted from each other can make, ends
  where it comes round. 404 for anything that isn't a shown plant. Cached at
  the edge for 60 s.

**Curated at launch.** A new plant is `pending` and isn't listed until Chris
shows it, from `packaging/shortlinks/` against the deployed database:

```
npm run garden:pending       # the waiting plants, each with its short link to listen to
npm run garden:show <id> …   # into the garden
npm run garden:hide <id> …   # out of it again; its short link still works
```

(`node garden.mjs ... --local` does the same against `wrangler dev`'s
database.) A whole takedown is still `patches.removed`, which also takes the
plant out of the garden.

**Seeded with the examples** (#392), so the garden is never empty:

```
cargo run --release --bin garden_seed            # SOBA_LINKS_URL=http://127.0.0.1:8787 for wrangler dev
cargo run --release --bin garden_seed -- --dry-run
```

It plants every example under its own name, by "Soba", opened in the app
(with no audio device) and encoded as Share encodes it, so an example shared
from the app later is the same plant, not a twin. Encoding the example files
themselves would not do: the editor writes parameters a file leaves out, so
the JSON differs. Nor would a seed in Node, whose JSON differs again. For the
same reason a link numbers its modules from 1 in order
(`patch_link::renumbered`): the editor's IDs count up across a session, so
without it an example opened second made a different link from one opened
first. One plant every
twelve seconds keeps it inside the rate limit (about five minutes for all
of them), and it plants the last example first, so the garden, newest
first, reads in the Examples menu's order. It ends by printing the
`npm run garden:show -- <id> …` that lists them all.
