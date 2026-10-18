# Patch Garden — Proposal

Oct 17, 2026 · @Chris

## Summary

Patch Garden is a public page at **soba.chaps.dev/garden** where people plant Soba patches and others wander through and listen. Each patch is drawn as its own cable diagram, like a pressed flower. Hover over one and it plays live, right there, before you open it. Open it, change it and plant it again, and the new patch grows from the old one as a visible branch.

Now is the moment because most of the plumbing landed this week. Patch links (#303) made the link itself the patch. Short links (#376) put a Cloudflare Worker and a D1 database behind `soba.chaps.dev/s/<id>`. D1 was picked over KV partly *because* a gallery would want queries. The garden is a thin layer over those: one table, two endpoints and one static page.

It also gives Soba something it doesn't have yet: a reason to make a patch for someone else.

## What's already built

The garden needs almost no new infrastructure.

| Piece | Where it came from | What the garden uses it for |
| --- | --- | --- |
| Patch links: `#p=1.<data>`, deflated JSON in the URL fragment | #303 | The patch format the garden stores and serves, unchanged |
| Short links: `soba.chaps.dev/s/<id>`, an 8-character content-addressed ID | #370 design, #376 build | Every plant *is* a short link; the garden adds metadata beside it |
| Worker `soba-links.chaps.workers.dev` + D1, 10 POSTs/min/IP, 410 takedowns | #376 | Home for the `plants` table and the two new endpoints |
| soba.chaps.dev on GitHub Pages, deployed after CI | #371 | Hosts the static garden page |
| Recipe embeds of the web build, with a Play button | Soba docs | The live "listen before you open" player |
| `example:` sample keys that the browser fetches | #303 | Patches that use built-in samples can be planted; ones with private samples can't |

Sources: the vault notes \[\[Soba Patch Links - 2026-10-16\]\], \[\[Soba Short Patch Links Design (#370) - 2026-10-17\]\] and \[\[Soba Moves to soba.chaps.dev (#371) - 2026-10-17\]\].

## The experience

**Wandering.** The garden is a loose grid of cards on a dark page. Each card is the patch's own graph, drawn from its JSON: modules as small coloured shapes in their real positions, and cables as curves in Soba's signal colours (audio, CV, gate, MIDI). A drone looks sparse and a drum machine looks dense, so the drawing is a fair hint of the sound. Under it are a title, an optional name, the date planted, and the module count.

**Listening.** Hover, or tap on a phone, and the card plays live through one shared Soba embed in the page. No audio file is stored anywhere. Moving to another card crossfades to the next patch. A thin ring around the card shows the signal level, so it's clear which one is sounding.

**Opening.** **Open in Soba** goes to `soba.chaps.dev/s/<id>`, the short link that already works today. The patch opens stopped, with nothing unsaved, exactly as a shared link does now.

**Planting.** In Soba, **Share → Plant in the garden** asks for a title (40 characters at most) and an optional name, then plants the patch. If the patch was opened from the garden, its parent is recorded automatically.

**Branches.** A card with descendants shows a small stem count. Opening it shows the lineage: the original, then each remix, as a short vertical vine of cards. This is the part that makes it a garden rather than a gallery.

## Design principles

1. **The link is still the patch.** The garden stores only what a short link already stores, plus a title and a parent. Any plant can be exported to static files, and its link keeps working if the service goes away.
2. **Sound first, words second.** Listening takes one hover. Text is limited to a title and a name: no descriptions, comments, likes or counts. That keeps it calm and leaves almost nothing to moderate.
3. **No accounts.** Planting needs no sign-in. Rate limits and manual takedowns (410), which already exist, are the safety net. A name is just text you type.
4. **Show what a patch is, not a screenshot.** Cards are drawn from the patch data, so they are always true to it and cost nothing to store.
5. **Remixing is the point.** The parent link is recorded automatically, never asked for, and lineage is the only ranking the garden has.

## Architecture

A plant is a short link with a title and an optional parent. The only new parts are the garden page, a `plants` table and two Worker endpoints.

&#91;embedded content: Patch Garden architecture · one new page, one new table\]

- **Table.** `plants(id TEXT PRIMARY KEY REFERENCES links, title TEXT, name TEXT, parent TEXT NULL, planted_at INTEGER, hidden INTEGER DEFAULT 0)`. `id` is the existing 8-character content hash, so planting the same patch twice is a no-op.
- **Endpoints.** `POST /plant` takes `{payload, title, name?, parent?}`, stores the link if it's new, and inserts the plant. `GET /garden?before=<t>` returns pages of 48 plants with their payloads, cached at the edge for 60 s.
- **Drawing cards.** The page inflates each payload with `DecompressionStream("deflate-raw")`, which is already proven against Soba's output in the Worker, then draws modules and cables as SVG. No Rust is needed for the drawing.
- **Listening.** One hidden embed of the web build loads a patch when a card is hovered. That reuses the recipe-embed path, so the garden page carries a single wasm instance, not one per card.

## Milestones

v1 ends at M3. Each milestone is about one Soba issue's worth of work.

1. **M1 · Seed bed.** A D1 `plants` table and `POST /plant` and `GET /garden` on the existing Worker, under the same rate limit. Done when a plant from curl shows up in the list and its short link opens in Soba.
2. **M2 · The page.** A static `soba.chaps.dev/garden/` page: cards drawn from the patch JSON and live listening through one shared embed. Seed it with Soba's own examples, so the garden is never empty on day one. Done when hover-to-listen works in Chrome and on a phone.
3. **M3 · Planting from Soba.** A **Plant in the garden** item in the share flow, on desktop and web, which records the parent when the patch came from the garden. **Ship v1 here.**
4. **M4 · Vines.** Lineage view, stem counts, and "grown from" on each card.
5. **Later.** A "plays itself" check before planting, seasonal rows (the garden grouped by month), a static export of the whole garden as an archive, and a chaps.dev page featuring a few favourites.

## Risks and open questions

| Risk | Why it matters | Fallback |
| --- | --- | --- |
| Silent patches | Many patches only sound when you play keys or MIDI, so hovering gives silence | Store a short note phrase alongside the plant (recorded while planting), or mark the card "needs playing" |
| Heavy patches in the browser | Canopy, Clair de Satellite and Orrery already drop buffers on the web (#349, follow-up #387) | Show a CPU-weight badge from the module count; heavy cards play only on click, not on hover |
| Abuse in titles and names | Public, unauthenticated text | 40-character limit, a word filter in the Worker, 410 takedowns. Add GitHub sign-in only if it's ever needed |
| Safari and Firefox audio | Autoplay rules may block hover playback until a first click | A one-time "tap to let the garden play" gate, which is honest and calm |
| Free-tier limits | D1 and Workers free tiers | Reads are light (one list query per visit, cached at the edge); writes are capped by the existing rate limit |

Open questions:

- [ ] Is the garden curated (you approve plants) or open from day one? Starting curated is a softer launch.
- [ ] Should desktop Soba get a **Garden** browser of its own, or does it just open the web page?
- [ ] What is the right card size on a phone: one patch per row, or two?
- [ ] Does the parent need to be in the garden, or can any short link be a parent?
