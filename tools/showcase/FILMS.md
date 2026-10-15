# Films: a whole song, filmed as it plays

A *film* is a capture of a self-playing patch from start to end. The camera goes wherever the music is: into a group as its part plays, back out, and across to the next. This is how the [Canopy](../../docs/src/songs/canopy.md) films were made: a full-length YouTube cut, and a one-minute vertical cut for Instagram. The steps below re-shoot them, and are the template for the next song.

There is no text over the picture: what you see is the app.

## The kit

Everything is in `tools/showcase/`. The tools work on any patch:

| Tool | What it does |
|------|--------------|
| `shots.py` | The shot list. A film is a shot like any other, run with `--converge`, `--subjects` and `--dry` |
| `tour.py` | The camera. Aims in patch space on whichever level is on show (an `aim` cue), and dives into groups by name and back out (`enter` and `leave`, so the app's own dive plays) |
| `stems.py` | One WAV per part: the patch rendered with every other mixer channel muted |
| `activity.py` | Each part's level, per beat, from the stems: `activity.json` and a heat strip, `activity.png` |
| `check_film.py` | Frames to review a take by: where each move lands, partway through long glides, and mid-dive. Compares two takes |
| `encode.py` | The film to a file: crop, fades, loudness, an MP4 or a `.mov` master with 24-bit PCM sound |
| `zoom_jitter.py` | Whether everything scales smoothly as the view zooms (#255). Worth running before a film, since a film zooms all the time |

What a film keeps between takes is in `tools/showcase/films/<film>/`, in git:
- `aims*.json`: each module's measured size, which the camera aims from. One file per shot, written by each run's previsit.
- `activity.json` and `activity.png`: what plays when.

The stems themselves (about 100 MB a part) and the takes stay in `target/showcase/`.

## Re-shooting a film

Use Canopy as the example: the shots are `canopy` (1920×1080, the whole song) and `canopy-vertical` (a portrait window, cropped to 1080×1920).

1. **Build the capture exe**, from a tree rebased on today's master. A visual change that lands after you film means filming again.
   ```bash
   CARGO_TARGET_DIR=target/capture cargo build --release
   ```
   Film with other worktrees quiet. A parallel cargo build can use up the commit limit and kill a long capture ("memory allocation failed").
2. **If the song changed** (the score, a part, the mix), measure what plays when again:
   ```bash
   python tools/showcase/stems.py patches/canopy.json target/showcase/canopy-stems --seconds 262 --jobs 3
   python tools/showcase/activity.py patches/canopy.json target/showcase/canopy-stems --out tools/showcase/films/canopy
   ```
3. **Re-aim.** The camera aims at modules by their measured sizes, and a module that changed size, or a new app version, moves them. A dry run replays the tour packed tight, and its previsit measures every module again. `--converge` repeats dry runs until the sizes stop changing: one run of about 7 minutes, or two if a module grew. It leaves a contact sheet of every framing in `target/showcase/<shot>-dry/sheet.png`.
   ```bash
   python tools/showcase/shots.py canopy --converge
   python tools/showcase/shots.py canopy-vertical --converge
   ```
4. **Check each shot is of something playing.** The check flags any hold whose subject is quiet. The Canopy films accept a few on purpose: the harp and the break entering under the kit, and the riser roaring at only −40 dB RMS.
   ```bash
   python tools/showcase/shots.py canopy canopy-vertical --subjects
   ```
5. **Film.** The landscape take is about 8 minutes and 1 GB at crf 12; the portrait one is about 7 minutes. The shot scripts in `tools/showcase/scripts/` are rewritten as you film: commit them with the aims. A take measures the sizes too, and says if any changed: then the take aimed from stale ones.
   ```bash
   python tools/showcase/shots.py canopy
   python tools/showcase/shots.py canopy-vertical
   ```
6. **Review.** Read the sheets in `target/showcase/<shot>-check/`. To see what changed since an earlier take, keep that take's check folder and pass it with `--against`.
   ```bash
   python tools/showcase/check_film.py canopy
   python tools/showcase/check_film.py canopy-vertical --from 114 --to 178
   ```
   Look for:
   - empty canvas mid-glide;
   - a pill or tooltip lit up;
   - modules cut off at the edge.
7. **Encode.** The vertical cut is a minute of a capture of the whole song, so trim it with `--from` and `--to`. Crop it to the editor, which starts 165 px down at ppp 1.5.
   ```bash
   python tools/showcase/encode.py canopy "target/showcase/canopy-film/Canopy (YouTube master).mov" \
       --fps 30 --crf 16 --lufs -14 --fade-in 1 --fade-out 3 --poster 138.5
   python tools/showcase/encode.py canopy-vertical "target/showcase/canopy-film/Canopy (Instagram).mp4" \
       --from 115.86 --to 175.86 --crop 1080x1920+0+165 --fade-out 2 --fps 30 --crf 18 --lufs -14 --poster 30
   ```
   Check both with `ffprobe`: size, 30 fps, length, and about −14 LUFS with peaks under −1 dBTP:
   ```bash
   ffmpeg -i <file> -af ebur128=peak=true -f null -
   ```
   The deliverables go to `~/Music/Soba/`, next to the earlier films.

## A new film

1. **A patch that plays itself to the end.** If its Arranger loops, stop the Clock just before the loop: `s.at(<end>, "param util.clock Run 0")`, as `canopy` does at bar 125.
2. **Stems and activity**, as in step 2. A part is whatever feeds a Mixer channel; `stems.py --list` shows them, and their names are the shots' subjects.
   ```bash
   python tools/showcase/activity.py <patch> <stems> --out tools/showcase/films/<film>
   ```
   The sections are read from the patch's Arranger. A song with no Arranger names them with `--sections`, each at the bar it starts on. Orrery, which the Day drives, was measured like this:
   ```bash
   python tools/showcase/activity.py patches/orrery.json target/showcase/orrery-stems --out tools/showcase/films/orrery \
       --sections "Dawn@1,Morning@21,Noon@49,Tide@69,Afternoon@85,Dusk@117,Night@141"
   ```
   If a song doesn't stop on its own, stop its Clock once the last note is struck. Orrery's Day would begin a second day at 5:08, so `orrery` stops the Clock just after the last gong.
3. **Read the heat strip** (`activity.png`) with the score. Write the shot list in `shots.py` beside `canopy`, as one function per cut. The song's section times are the clock: `bar(n)` for Canopy. Each shot starts a `Tour`:
   ```python
   tour = Tour(s, ROOT / "patches/<patch>.json", editor=EDITORS[(s.size, s.ppp)],
               bases=FILMS / "<film>" / "aims.json", dry=s.dry)
   tour.previsit(-46.0)   # every group opened once, unseen, before recording starts
   ```
   The moves:
   - `look(t, point, zoom, dur, note, subject)` glides to centre a patch point. `hop` does the same for long ways, rising over the middle.
   - `enter(t, "Group")` and `leave(t)` dive in and out. The cue names the group, so nothing is clicked and nothing needs aiming.
   - `enter(t, "Group", land=("seq.trigger", 1.0))` lands the dive on one module inside (an id, or the first of a kind) at a zoom, rather than on the whole level. The cue is `enter Group at x y zoom`.
   - `inside(...)` settles on a whole level, then drifts in on one module. `inside_tall(...)` is the portrait version, which drifts from one module to another.
   - A level's modules come from the patch, through `centre_of`, `middle`, `fit_zoom` and `find("module.kind")`.

   A shot's window is its `size` and `ppp`. A new pair needs its editor rect in `tour.EDITORS`, read from any capture's `layout` dump (`view.editor`).
4. **Converge, check subjects, film, review, encode:** steps 3–7.

## What the films taught

These lessons are from the Canopy session (2026-10-14) and the Orrery one (2026-10-15). The vault note "Soba Films Canopy Diving Through Its Groups" has the full story.

- **Every group is opened once before recording.** Since #245 the app measures the modules inside groups when a patch opens, so miniatures and the framing of a level no longer wait for a first visit. The tour still opens each group once, unseen, and dumps its layout: that's how it learns the sizes it aims from.
- **Name the group; don't click it (#252).** The first Canopy films entered groups with a double-click on the title bar and steered the camera with relative `camera` cues. Every dive's landing had to be measured on dry runs, and a missed click left the tour on the wrong level, so every later aim was wrong. A gliding pointer also lit the Open pills of the faces it crossed. The `enter`, `leave` and `aim` cues need no aiming: the app finds the group by name and the point in patch space, from wherever the view is.
- **In a portrait window, inside a wide group, frame a module, not the level.** A whole level fitted to 720 points wide is a thin strip, and a dive lands on it. Land the dive on a module instead (`enter(..., land=(module, zoom))`, #259): the zoom through the miniature arrives on it in one move, with no stop on the strip. Without `land`, start the first look inside at the time `enter` returns: the dive's ease-out and the look's ease-in still meet at a standstill of about a third of a second.
- **A riser ramps up from silence.** Dive into it once it's audible, not when its section starts; the activity check catches this.
- **A capture's noise isn't the render's (#253).** Sync is exact, but a byte compare against `render` fails for patches with noise.
- **Text steps as the view zooms (#255).** Notes rewrap, and the Arranger's width and module heights saw. A film shows it more than anything, so check `zoom_jitter.py` before filming.
- **A dive lands where the app frames it, so the tour must agree (#257).** Opening a group fits all of its inside, below the breadcrumb trail. `Tour.framed()` mirrors that fit (`FRAME_MARGIN` and `TRAIL_HEIGHT` in `tour.py` match `framing.rs`), and `inside` holds on it. When the two disagreed, every dive landed and then zoomed about 10% closer. If the app's margins change, change them in `tour.py` too. The previsit's `lvl###.json` dumps show where the app really landed.
- **Centre a module at the patch's edge and a third of the frame is empty canvas.** Orrery's "Who is up" Logic modules sit at x = 24, the Tide's LFO at the right of its frame, and the Tape at the right of the desk. `orrery` aims through `clear()`, which moves the view in from the edge so it fills the frame and the module isn't centred.
- **In a portrait window, inside a group, centre a level's top row and the top third of the phone is empty.** Most levels start at y = 0, so `inside_tall` left the top of the frame empty on every dive. `orrery-vertical` aims through `upright()`. It centres a short level in the frame, and keeps a tall one in frame from just under the breadcrumb trail.
- **A small group's level dives in small.** The app never frames closer than 1:1, so Orrery's Eclipse lands as a little island. Push in on the module that's playing (`inside(..., focus=...)`), timed so it lands before the moment, here the gong.
- **To show a pattern, go inside it.** A group's miniature doesn't show its steps. The Twins' 16 against 15 only reads with a dive into each Step Sequencer.
- **A sequencer whose patterns change by CV should Follow.** Otherwise its grid shows pattern A while the groove plays. `previsit(..., follow={"Drums": ["seq.trigger"]})` clicks Follow on while the group is open, unseen, then takes the pointer out of the window (`move off`), so nothing lights up under it. A capture counts a kind across the whole patch, not the level, which is why Orrery's Bass sequencer is `seq.step#3`: the Twins hold the first two.
- **`check_film.py --against` matches frames by number.** Add or remove a move and every later frame compares against the wrong one. After changing moves, grab the holds you changed from the video at their times instead (`ffmpeg -ss <t> -frames:v 1`).
