# Documentation images

Screenshots for the manual. This file isn't part of the book; it's a note for whoever updates them.

## What's here

| File | Shows | Used on |
|------|-------|---------|
| `interface-overview.png` | The whole window, Lush Pad playing a chord | Introduction, Interface Overview |
| `interface-context-menu.png` | The right-click add menu | Interface Overview |
| `interface-quick-add.png` | The quick-add palette | Interface Overview |
| `interface-frames-and-notes.png` | First Sound playing, with its Shape frame's menu open | Interface Overview |
| `groups-outside.png` | First Sound with its oscillator, envelope and VCA grouped as Voice, playing, Wave and four knobs pinned | Groups |
| `groups-inside.png` | Inside Voice: the trail, Inputs and Outputs, re-laid so no cable crosses a module, zoomed to 0.78 | Groups |
| `tutorial-complete-patch.png` | The finished First Patch (`tools/showcase/patches/first-patch.json`), playing a note | Your First Patch |
| `module-<page>.png` | One module, playing, cropped with a margin of grid | Each module page (`module-svf-filter.png` for `svf-filter.md`) |
| `recipe-live-looper.png` | The whole Live Looper patch as it opens, frames and notes included | Live Looper |
| `recipe-from-one-sine-harmony.png`, `recipe-from-one-sine-line.png` | One frame of From One Sine mid-song: the Chord Sequencer playing Everything, and the motif's Step Sequencer on pattern C | From One Sine |
| `recipe-held-vowels-faces.png` | The Vocoder's bands frozen on each of the four vowels, side by side | Held Vowels |

A recipe with a playable embed (`<iframe class="patch-embed">`) has no picture of its whole patch: the embed shows the patch, fitted, and plays it. What a still would have said about the layout goes in the italic line under the embed. A recipe keeps a picture only for what the embed can't show: a patch with no embed (Live Looper needs a live input), a moment minutes into a song, or a face at several moments at once.

## Guidelines

- Capture while the patch is playing, so cables glow and displays show signal.
- Keep module shots at one scale (1.5 pixels per point) so modules compare at a glance.
- Crop modules tightly, leaving a little of the dark grid around the edge. Don't let cables cross a module's face.
- Keep each PNG under about 400 KB, and full-window shots no wider than 1600 px.
- Recapture a module's shot whenever its ports, knobs or display change. A screenshot that disagrees with the table under it is worse than none.

## Re-shooting recipe images

`python tools/showcase/recipe_images.py` films every recipe picture above and writes it again (name pictures to do only those, or a new example to picture it whole). For a whole patch, listed in the script's `STILLS`, it plays the patch for two seconds, frames it with the capture's `fit` cue, crops to its modules, frames and notes with a little grid around, and saves it 1594 px wide in a 256-colour palette. `--check` writes nothing and lists the pictures that no longer match the build. Build into `target/capture` first (see `tools/showcase/shots.py`).

`recipe-from-one-sine-harmony.png` and `recipe-from-one-sine-line.png` show the song mid-performance. They're listed in the script's `MOMENTS` table, and it re-shoots and checks them with the rest. Each one plays the song up to its moment, aims the view at one frame at the scale the picture has always had, and crops to that frame without resizing it. Playing three minutes of song frame by frame takes a few minutes.

`recipe-held-vowels-faces.png` is a close-up, listed in the script's `CLOSEUPS` table: the patch is filmed once for each vowel, with the view aimed at the Vocoder, and the same crop of its face from each take is set on a grid.

## Re-shooting the interface pictures

`python tools/showcase/manual_images.py` re-shoots the four `interface-*.png` pictures and `tutorial-complete-patch.png` the same way: each patch is played and posed by a short capture script (a chord held, a menu opened, a key down), then cropped to the window, the patch or a fixed box, and saved as the recipe images are. Name pictures to do only those, or pass `--check` to list the stale ones. A picture's pointer positions are in window points, so a change that moves a menu or the palette means re-aiming its cues: see the script.

## Re-shooting the module close-ups

`python tools/showcase/module_images.py` re-shoots every `module-<page>.png`. Each close-up has a small patch in `tools/showcase/modules/` that holds the moment its caption describes (a chord held, a loop overdubbing, a filter swept), or uses an example when the caption names one, and an entry in the script's `MODULES` table with the cues that pose it. The script finds the module, aims the view at it, plays the cues up to a still and crops to the node with 22 points of grid at the sides for its cable stubs and 6 above and below. Name close-ups to do only those, or pass `--check` to list the stale ones. A face change that alters a caption's moment means changing its patch or cues, then the caption.

The groups and sequencer pictures are still shot by hand.

## Wanted

- `interface-module-menu.png`: a module's right-click menu (Duplicate, Copy, Bypass, Reset to defaults, Delete), for the Interface Overview's "The module menu" section.
