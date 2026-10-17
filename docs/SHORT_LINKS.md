# Short patch links

*The design from #370, 2026-10-17. Not built yet: #376 builds it.*

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

The spike is `packaging/shortlinks/`: the Worker, its schema, and a test that
runs against `npx wrangler dev`. It has only run locally, and nothing is deployed.
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

## Desktop

Share posts with `ureq` (already a dependency for updates) on a thread, with a
3-second timeout, so the frame never waits. While it's out, the note says
"Making a link…". If it fails or times out, the long link is copied instead. Opening a
`soba.chaps.dev/s/<id>` pasted onto the desktop canvas fetches it the same way.

## Durability

If the service ever shuts down, the short links die with it unless they're carried over:
- A scheduled GitHub Action runs `wrangler d1 export` into a **private** archive
  repo every week.
- If the service has to go, the archive is written out as static files,
  `soba.chaps.dev/s/<id>.txt`, and the app tries that same-origin path
  when the API doesn't answer. Every link keeps working on GitHub Pages alone.
  (The public repo would list the IDs then. That's acceptable as a last resort, but not before.)
- The long link stays authoritative: `p=1.` is the format, and a short link is
  only a name for one.

## Later: Patch Garden

The API is under `/v1/` and the table can grow. A gallery would add a
`published` flag, a title and a preview render (R2) to rows the sharer
*chooses* to publish, with `GET /v1/garden` listing only those. Sharing is
never publishing. Link previews in chats, which the 404 page can't give,
would need the Worker in front of `soba.chaps.dev/s/*`. That's when moving
chaps.dev's DNS to Cloudflare earns its keep.
