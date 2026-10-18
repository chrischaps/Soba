"""What the picture tools share: which pictures a change could have touched,
and filming several at once (#217).

recipe_images.py, manual_images.py and module_images.py each film their
pictures with soba --capture and, with --check or --refresh, compare them
with the ones in docs/src/images. Filming is slow (From One Sine plays
minutes of song to reach its moments), and most pictures can't have
changed: a change to the Tape's face only touches the pictures that show a
Tape. So --check and --refresh film only the pictures whose inputs have
moved since they were last seen to match, and say why they skipped the rest.

A picture's inputs are:

- its patch and the samples the patch plays
- the source of every module in the patch: src/modules/<file>.rs (or its
  folder) and the files in src/graph that draw that module's face, listed
  in FACES
- everything else that ends up in the app: the rest of src/, assets/,
  library/, Cargo.toml and the like, and the picture tools themselves.
  A change to any of these films every picture

What can't reach a picture is skipped: docs/, tests/, the other tools and
patches, test fixtures and src/bin/.

A module's own file only counts as its own while nothing but that module's
other files (and the dispatch hubs in HUBS) uses it. Sampler code that the
Chopper uses counts for both. Sampler code that the app's record menu uses
counts for every picture. A file left out of FACES counts for every picture
too, so a missing entry costs time, never a stale picture.

The ledger, in the git folder that every worktree on this machine shares
(soba-pictures.json), records for each picture the commit at which it was
last filmed and found to match (or saved), and the PNG it matched. A
picture is filmed again when any of these holds:

- it has no entry
- its PNG has changed since its entry
- `git diff` from that commit to the working tree touches its inputs
- its entry is a week old, so whatever the map above misses can't hide
  for long

Nothing is recorded for a picture whose inputs have uncommitted changes,
since no commit then describes what was filmed, nor for any picture when
target/capture's build is older than the last commit to the source.

--all films every picture, as before #217. --jobs N films N at once
(default: JOBS). Captures are frame-stepped, so a picture comes out the
same whichever order they run in: the 41 close-ups gave the same verdicts
one, four and eight at a time, in 6 min, 1.9 min and 1 min. Each capture
takes about 1.2 GB of commit charge, and too many at once beside a cargo
build can run Windows out of it ("memory allocation failed"), so JOBS stops
at six.
"""

import datetime
import functools
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import textwrap
import threading
from collections import namedtuple
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from common import ROOT, build_is_old, exe, require_fresh_build

JOBS = max(1, min((os.cpu_count() or 2) - 1, 6))
# A picture is filmed at least this often, whatever the map says
STALEST = datetime.timedelta(days=7)

# The files in src/graph that draw one module's face (or several modules'),
# by the module ids they draw. src/modules/<file>.rs and src/modules/<file>/
# are found by the id they register, so they needn't be listed
FACES = {
    "arranger_display": ["seq.arranger"],
    "chopper_display": ["source.chopper"],
    "chord_chime": ["seq.chord"],
    "chord_ear": ["seq.chord"],
    "chord_grid": ["seq.chord"],
    "chorus_display": ["fx.chorus"],
    "clock_display": ["util.clock"],
    "compressor_display": ["fx.compressor"],
    "dancer_display": ["util.dancer"],
    "dancer_moves": ["util.dancer"],
    "delay_display": ["fx.delay"],
    "distortion_display": ["fx.distortion"],
    "divider_display": ["util.divider"],
    "drum_display": ["source.drum"],
    "envelope_display": ["mod.adsr"],
    "eq_display": ["fx.eq"],
    "filter_display": ["filter.svf", "filter.ladder"],
    "input_display": ["source.audio_input"],
    "lfo_display": ["mod.lfo"],
    "logic_display": ["util.logic"],
    "looper_display": ["util.looper"],
    "mixer_strips": ["util.mixer"],
    "oscillator_display": ["osc.sine"],
    "oscillator_trace": ["osc.sine"],
    "reverb_display": ["fx.reverb"],
    "sampler_display": ["source.sampler"],
    "sampler_slices": ["source.sampler"],
    "sampler_take": ["source.sampler"],
    "sampler_tidy": ["source.sampler"],
    "slicer_display": ["fx.slicer"],
    "slope_display": ["mod.slope"],
    "step_ear": ["seq.step"],
    "step_grid": ["seq.step"],
    "tape_display": ["fx.tape"],
    "trigger_display": ["seq.trigger"],
    "vocoder_display": ["fx.vocoder"],
}
# Files that name every module to declare or dispatch it: using a module's
# file from one of these doesn't make the file shared
HUBS = {"src/lib.rs", "src/main.rs", "src/modules/mod.rs", "src/graph/mod.rs", "src/graph/node_data.rs",
        "src/graph/module_ui.rs", "src/graph/templates.rs"}
# The picture tools: a change to any films all of their pictures
TOOLS = {"tools/showcase/picture_check.py", "tools/showcase/recipe_images.py",
         "tools/showcase/manual_images.py", "tools/showcase/module_images.py"}
# What never reaches a picture (a picture's own patch, samples and tools are
# checked first)
UNPICTURED = ("docs/", "tests/", ".github/", "tools/", "packaging/", "licenses/", "patches/", "src/bin/")
UNPICTURED_ROOT = {".gitignore", "LICENSE", "index.html", "Trunk.toml"}
AUDIO = (".wav", ".mp3", ".ogg", ".flac", ".opus")

# A picture's inputs: repo paths, and the module ids it shows (None: every
# module, for a menu that lists them all)
Inputs = namedtuple("Inputs", "paths modules")


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)


def ledger_path():
    common = git("rev-parse", "--git-common-dir").stdout.strip()
    return (ROOT / common).resolve() / "soba-pictures.json"


def read_ledger():
    try:
        return json.loads(ledger_path().read_text())
    except (OSError, ValueError):
        return {}


def write_ledger(entries):
    """Merges `entries` into the ledger, keeping what another run wrote meanwhile."""
    path = ledger_path()
    ledger = read_ledger()
    ledger.update(entries)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(ledger, indent=1, sort_keys=True) + "\n")
    os.replace(tmp, path)


def png_hash(path):
    return hashlib.sha1(path.read_bytes()).hexdigest() if path.exists() else None


def patch_inputs(patch, extra=(), every_module=False):
    """A patch's inputs: the file, the samples it plays and the modules it holds."""
    patch = Path(patch)
    paths, modules = {rel(patch)}, set()

    def walk(value):
        if isinstance(value, dict):
            if isinstance(value.get("module_id"), str):
                modules.add(value["module_id"])
            for v in value.values():
                walk(v)
        elif isinstance(value, list):
            for v in value:
                walk(v)
        elif isinstance(value, str) and value.lower().endswith(AUDIO):
            name = value.removeprefix("example:")
            # Relative to the patch, or one of the examples' samples
            for base in (patch.parent, ROOT / "patches"):
                paths.add(rel(base / name))

    walk(json.loads(patch.read_text()))
    return Inputs(paths | {rel(p) for p in extra}, None if every_module else modules)


def rel(path):
    path = Path(path).resolve()
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return path.as_posix()


# ---- Who owns which source file ----

def strip_tests(text):
    """A file without its trailing test module, whose uses don't reach the app."""
    cut = re.search(r"#\[cfg\(test\)\]\s*mod tests", text)
    return text[:cut.start()] if cut else text


def ownership():
    """{unit: set of module ids} for every source file that belongs to some
    modules alone. A unit is a file in src/, or a module's folder
    src/modules/<name>/ with its src/modules/<name>.rs."""
    src = ROOT / "src"
    texts = {}
    for path in src.rglob("*.rs"):
        parts = path.relative_to(src).parts
        if parts[0] == "modules" and len(parts) > 2:
            unit = f"src/modules/{parts[1]}"
        elif parts[0] == "modules" and len(parts) == 2 and path.stem != "mod":
            unit = f"src/modules/{path.stem}"
        else:
            unit = rel(path)
        texts[unit] = texts.get(unit, "") + strip_tests(path.read_text(encoding="utf-8", errors="replace"))

    owners = {}
    # Each module's file, by the id it registers
    for unit, text in texts.items():
        if unit.startswith("src/modules/"):
            ids = set(re.findall(r'\bid: "([a-z_]+\.[a-z_]+)"', text))
            ids |= set(re.findall(r'_ID: &str = "([a-z_]+\.[a-z_]+)"', text))
            if ids:
                owners[unit] = ids
    for name, ids in FACES.items():
        unit = f"src/graph/{name}.rs"
        if unit in texts:
            owners[unit] = set(ids)

    def users(unit):
        """The files that use `unit` by path: modules::tape anywhere, or
        super::tape beside it (src/dsp/primitives has a tape of its own)."""
        parent, name = unit.split("/")[1], Path(unit).stem
        anywhere = re.compile(rf"\b{parent}::{name}\b|\b{parent}::\{{[^}}]*\b{name}\b")
        beside = re.compile(rf"\bsuper::{name}\b|\bsuper::\{{[^}}]*\b{name}\b")
        return [u for u, text in texts.items() if u != unit and u not in HUBS
                and (anywhere.search(text) or u.startswith(f"src/{parent}/") and beside.search(text))]

    used_by = {unit: users(unit) for unit in owners}
    # A file used from outside its modules belongs to the users' modules too,
    # or to every picture once any user is shared
    changed = True
    while changed:
        changed = False
        for unit in list(owners):
            if unit not in owners:
                continue
            ids = set(owners[unit])
            for user in used_by[unit]:
                if user not in owners:
                    del owners[unit]
                    changed = True
                    break
                ids |= owners[user]
            else:
                if ids != owners[unit]:
                    owners[unit] = ids
                    changed = True
    return owners


def unit_of(path):
    parts = path.split("/")
    if len(parts) >= 3 and parts[:2] == ["src", "modules"]:
        name = parts[2].removesuffix(".rs")
        return f"src/modules/{name}" if name != "mod" else path
    return path


# ---- What a change touches ----

ALL = "every picture"


def reach(path, owners):
    """What a changed path can touch: ALL, a set of module ids, or None for
    nothing pictured. (A picture's own patch, samples and PNG are matched
    before this.)"""
    if path in TOOLS:
        return ALL
    if path.startswith("src/"):
        if path.startswith("src/bin/") or "/fixtures/" in path or "/testdata/" in path:
            return None
        if not path.endswith(".rs"):
            return ALL
        return owners.get(unit_of(path), ALL)
    if path.startswith(UNPICTURED) or path in UNPICTURED_ROOT or ("/" not in path and path.endswith((".md", ".png"))):
        return None
    return ALL


def touches(path, inputs, owners):
    if path in inputs.paths:
        return True
    what = reach(path, owners)
    if what is None:
        return False
    if what == ALL or inputs.modules is None:
        return True
    return bool(what & inputs.modules)


@functools.lru_cache(maxsize=None)
def changes_since(commit):
    """The paths that differ between `commit` and the working tree, untracked
    files included, or None if the commit is unknown here."""
    diff = git("diff", "--name-only", commit)
    if diff.returncode != 0:
        return None
    untracked = git("ls-files", "--others", "--exclude-standard").stdout.split()
    return frozenset(diff.stdout.split()) | frozenset(untracked)


def describe(paths, owners):
    """A few words on what moved: the modules, and whether anything else did."""
    ids = set()
    other = False
    for path in paths:
        what = reach(path, owners)
        if isinstance(what, set):
            ids |= what
        else:
            other = True
    words = sorted(ids)[:4] + (["more"] if len(ids) > 4 else [])
    if other:
        words.append("things no picture shows")
    return ", ".join(words) or "nothing"


class Plan:
    """Which pictures to film, and why the rest can wait."""

    def __init__(self, names, png, inputs, forced=None, key=None):
        """`forced` is why every picture is filmed (--all, say), if it is.

        A check that films no picture (zoom_check.py's sweeps, #276) passes
        png=None and `key(name)`, its ledger entry's name. Its entries are
        recorded once it passes, and it's filmed again only when its inputs
        move or the entry ages."""
        self.png, self.inputs = png, inputs
        self.key = key or (lambda name: png(name).name)
        self.head = git("rev-parse", "HEAD").stdout.strip()
        self.owners = ownership()
        self.ledger = read_ledger()
        # Uncommitted changes: what they touch can't be recorded against HEAD
        self.dirty = changes_since("HEAD") or set()
        self.old_exe = exe_is_old()
        self.film, self.why = [], {}
        # {why: [names]} for the pictures that can wait
        self.skipped = {}
        for name in names:
            reason = forced or self.reason(name)
            if reason:
                self.film.append(name)
                self.why[name] = reason
            else:
                commit = self.ledger[self.key(name)]["commit"]
                moved = changes_since(commit)
                why = (f"nothing they show moved since {commit[:7]} ({describe(moved, self.owners)} did)"
                       if moved else f"nothing moved since {commit[:7]}")
                self.skipped.setdefault(why, []).append(name)

    def reason(self, name):
        """Why the picture needs filming, or None if it can wait."""
        entry = self.ledger.get(self.key(name))
        if self.png:
            path = self.png(name)
            if not entry or not path.exists():
                return "never filmed here" if path.exists() else "missing"
            if entry.get("png") != png_hash(path):
                return "its PNG changed"
        elif not entry:
            return "never passed here"
        age = datetime.datetime.now() - datetime.datetime.fromisoformat(entry["date"])
        if age > STALEST:
            return f"last filmed {age.days} days ago"
        moved = changes_since(entry["commit"])
        if moved is None:
            return f"shot at {entry['commit'][:7]}, which isn't here"
        hit = sorted(p for p in moved if touches(p, self.inputs(name), self.owners))
        if hit:
            return f"{hit[0]} changed" + (f" (and {len(hit) - 1} more)" if len(hit) > 1 else "")
        return None

    def entry(self, name):
        """The ledger entry for a picture just filmed and matched (or saved),
        or None if uncommitted changes touch it or the build is older than HEAD."""
        inputs = self.inputs(name)
        if self.old_exe or any(touches(p, inputs, self.owners) for p in self.dirty):
            return None
        entry = {"commit": self.head, "date": datetime.datetime.now().isoformat(timespec="seconds")}
        if self.png:
            entry["png"] = png_hash(self.png(name))
        return entry


def exe_is_old():
    """Whether the capture build is older than what goes into it, so it
    can't have filmed HEAD."""
    return build_is_old(exe())


# Lines of a failed capture's stderr worth showing: its last words
TAIL = 20


def failure(e):
    """What went wrong filming, for the report: the exception, and for a
    capture that exited non-zero the last lines it wrote to stderr (#333)."""
    if isinstance(e, subprocess.CalledProcessError):
        said = e.stderr or ""
        if isinstance(said, bytes):
            said = said.decode(errors="replace")
        lines = said.strip().splitlines()[-TAIL:]
        return f"the capture failed (exit {e.returncode})" + ("".join("\n" + line for line in lines)
                                                             if lines else ", saying nothing")
    return f"{type(e).__name__}: {e}"


def jobs_arg(args):
    if "--jobs" in args:
        i = args.index("--jobs")
        return max(1, int(args[i + 1]))
    return JOBS


def names_arg(args):
    """The names on the command line, leaving out --jobs's value."""
    names, skip = [], False
    for a in args:
        if skip:
            skip = False
        elif a == "--jobs":
            skip = True
        elif not a.startswith("--"):
            names.append(a)
    return names


def run(args, tool, every, png, inputs, shoot, compare, width):
    """The picture tools' main: films the pictures named in `args` (or
    `every`), saving them, or with --check comparing them, or with --refresh
    saving just the stale ones.

    `png(name)` is the picture's path, `inputs(name)` its Inputs,
    `shoot(name, tmp)` films it and returns the image, and `compare(image,
    path)` the fraction of it that differs. Returns the exit code."""
    from recipe_images import STALE_FRACTION

    refresh = "--refresh" in args
    check = "--check" in args or refresh
    named = names_arg(args)
    names = named or every
    jobs = jobs_arg(args)
    # Named pictures are always filmed: someone asked for them
    forced = ("--all" if "--all" in args else "named" if named else None if check else "saving them all")
    plan = Plan(names, png, inputs, forced)
    if plan.old_exe:
        print("(the capture build is older than HEAD's source, so nothing is recorded as filmed at HEAD)")
    for why, skipped in plan.skipped.items():
        print(f"Skipped {len(skipped)}, {why}:")
        print(textwrap.fill(" ".join(skipped), 100, initial_indent="    ", subsequent_indent="    ",
                            break_on_hyphens=False))
    if plan.skipped and plan.film:
        print()
    if plan.film:
        require_fresh_build(stale_ok=True)
    stale, failed, entries = [], [], {}
    with tempfile.TemporaryDirectory() as tmp, ThreadPoolExecutor(jobs) as pool:
        lock = threading.Lock()

        def film(name):
            """The picture, or None and what went wrong: one picture that
            can't be filmed doesn't stop the rest."""
            try:
                image, error = shoot(name, tmp), None
            except Exception as e:
                image, error = None, failure(e)
            with lock:
                print(f"  {'filmed' if error is None else 'failed'} {name}", file=sys.stderr, flush=True)
            return image, error

        for name, (image, error) in zip(plan.film, pool.map(film, plan.film)):
            path = png(name)
            why = plan.why[name]
            if error is not None:
                print(f"{name:{width}} {'FAILED':22} filmed: {why}")
                print(textwrap.indent(error, "    "))
                failed.append(name)
            elif check:
                fraction = compare(image, path)
                verdict = "ok" if fraction <= STALE_FRACTION else ("missing" if not path.exists() else f"stale ({fraction:.1%} changed)")
                print(f"{name:{width}} {verdict:22} filmed: {why}")
                if fraction > STALE_FRACTION:
                    stale.append(name)
                    if refresh:
                        image.save(path, optimize=True)
                if fraction <= STALE_FRACTION or refresh:
                    entries[path.name] = plan.entry(name)
            else:
                image.save(path, optimize=True)
                print(f"{name:{width}} {image.width}x{image.height}, {path.stat().st_size // 1024} KB")
                entries[path.name] = plan.entry(name)
    write_ledger({k: v for k, v in entries.items() if v})
    skipped = sum(len(n) for n in plan.skipped.values())
    print(f"\nFilmed {len(plan.film)} of {len(names)} ({jobs} at a time)"
          + (f", skipped {skipped}: --all films them too" if skipped else ""))
    if refresh and stale:
        print(f"Re-shot {' '.join(stale)}")
    elif check and stale:
        print(f"Re-shoot with: python tools/showcase/{tool} {' '.join(stale)}")
    if failed:
        print(f"Couldn't film {' '.join(failed)}")
    return 1 if failed or (stale and not refresh) else 0
