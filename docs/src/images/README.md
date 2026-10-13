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
| `recipe-<page>.png` | The whole example patch, playing, frames and notes included | Each recipe (`recipe-lush-pad.png` for `lush-pad.md`) |

## Guidelines

- Capture while the patch is playing, so cables glow and displays show signal.
- Keep module shots at one scale (1.5 pixels per point) so modules compare at a glance.
- Crop modules tightly, leaving a little of the dark grid around the edge. Don't let cables cross a module's face.
- Keep each PNG under about 400 KB, and full-window shots no wider than 1600 px.
- Recapture a module's shot whenever its ports, knobs or display change. A screenshot that disagrees with the table under it is worse than none.

## Re-shooting recipe images

`python tools/showcase/recipe_images.py` films every example that has a `recipe-<page>.png` and writes it again (name examples to do only those, or a new one). It plays the patch for two seconds, frames it with the capture's `fit` cue, crops to its modules, frames and notes with a little grid around, and saves it 1594 px wide in a 256-colour palette. `--check` writes nothing and lists the pictures that no longer match the build. Build into `target/capture` first (see `tools/showcase/shots.py`).

`recipe-from-one-sine-harmony.png` and `recipe-from-one-sine-line.png` show the song mid-performance, so they're shot by hand.

## Re-shooting the interface pictures

`python tools/showcase/manual_images.py` re-shoots the four `interface-*.png` pictures and `tutorial-complete-patch.png` the same way: each patch is played and posed by a short capture script (a chord held, a menu opened, a key down), then cropped to the window, the patch or a fixed box, and saved as the recipe images are. Name pictures to do only those, or pass `--check` to list the stale ones. A picture's pointer positions are in window points, so a change that moves a menu or the palette means re-aiming its cues: see the script.

The groups and sequencer pictures are still shot by hand.

## Wanted

- `interface-module-menu.png`: a module's right-click menu (Duplicate, Copy, Bypass, Reset to defaults, Delete), for the Interface Overview's "The module menu" section.
