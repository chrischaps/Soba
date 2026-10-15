"""A camera that tours a patch through its groups, for capture scripts.

A shot in shots.py hands its Script to a Tour, then aims in patch space on
whatever level the view is on:

    tour = Tour(s, "patches/canopy.json", editor=(0, 74.7, 1536, 838), bases=...)
    tour.previsit(-40)                       # every group opened once, unseen
    tour.look(2.0, tour.centre_of(tour.group("Kalimba")), 1.6, 3.0, "Kalimba", subject="kalimba")
    tour.enter(6.0, "Kalimba")               # the app dives in, through the miniature
    tour.look(6.7, tour.middle(), 1.2, 2.0, "inside the Kalimba")
    tour.leave(12.0)                         # and back out

How the view moves:

- `look` is an `aim` cue: the app glides to centre a patch point at a zoom,
  on whichever level is on show, from wherever the view is. Nothing about
  the view has to be known to aim, so nothing about it is measured.
- `enter` and `leave` are the cues of the same names: the app dives into a
  group by its name (or its path, `Bells/FM Bell`, where names repeat) and
  back out, as a double-click and Escape would, with no click to aim.
  `enter(..., land=("seq.trigger", 1.0))` lands the dive on a module inside.
- A level's modules come from the patch, a group by `group("Bells")` or any
  node by its id; their sizes from what the app drew, so `middle()`,
  `centre_of()` and `fit_zoom()` aim at what is really there. The previsit
  dumps a layout of every level it opens, and `learn` keeps the sizes in
  the film's aims file (films/<film>/aims*.json) for the next run to aim
  from. Until then a module is taken at a nominal size: dry-run first
  (`shots.py <shot> --dry`), or `--converge`, which dry-runs until the
  sizes stop changing (one run, or two when a module grew).
- A dry run makes the moves packed tight, with a still of each on a contact
  sheet: every framing at a glance before a long take.
- Each `look` can name its `subject`, the part it's a shot of; `holds` lists
  them for activity.py to check against what is playing.
"""

import json
from pathlib import Path

ZOOM_MIN, ZOOM_MAX = 0.2, 2.0
# A module never measured yet, at zoom 1
NOMINAL = (206.0, 280.0)
# A dive takes 0.36 s
DIVE = 0.36
# How the app frames a level it dives into (framing.rs): clear of the
# breadcrumb trail along the top, with a margin all round, never past 1:1
FRAME_MARGIN, TRAIL_HEIGHT = 40.0, 56.0

# The node editor in points, below the toolbar and above the status bar, for
# a capture's window size and ppp (read from a `layout` cue's view: run any
# capture with `0.2 layout x` and look at view.editor). A portrait cut is
# cropped to the editor: from y = editor top * ppp, in pixels
EDITORS = {
    ("1920x1080", 1.25): (0.0, 74.72, 1536.0, 838.0),
    ("1920x1080", 1.55): (0.0, 74.7, 1238.7, 670.8),
    ("1080x2124", 1.5): (0.0, 109.64, 720.0, 1390.0),
}


class PatchMap:
    """Every level of a patch: what's on it and where, and which groups hold
    which levels. A level is None (the top) or a group's id. A group's
    Inputs and Outputs are "inputs" and "outputs" on its level."""

    def __init__(self, path):
        patch = json.loads(Path(path).read_text(encoding="utf-8"))
        self.levels = {None: {n["id"]: tuple(n["position"]) for n in patch["nodes"]}}
        self.kinds = {None: {n["id"]: n["module_id"] for n in patch["nodes"]}}
        self.names = {}
        self.parent = {}
        self._walk(patch.get("groups", []), None)

    def _walk(self, groups, level):
        for g in groups:
            gid = g["id"]
            self.levels[level][gid] = tuple(g["position"])
            self.names[gid] = g["name"]
            self.parent[gid] = level
            inside = {n["id"]: tuple(n["position"]) for n in g["nodes"]}
            self.kinds[gid] = {n["id"]: n["module_id"] for n in g["nodes"]}
            inside["inputs"] = tuple(g["inputs_position"])
            inside["outputs"] = tuple(g["outputs_position"])
            self.levels[gid] = inside
            self._walk(g.get("groups", []), gid)

    def group(self, name, within=None):
        """A group's id by its name, on one level or anywhere."""
        found = [gid for gid, n in self.names.items() if n == name and (within is None or self.parent[gid] == within)]
        if len(found) != 1:
            raise KeyError(f"{len(found)} groups called {name!r}")
        return found[0]

    def children(self, level):
        return [k for k in self.levels[level] if k in self.names]

    def path(self, gid):
        """A group's name for an `enter` cue: its own, or where another
        group shares it, its path of names from the top, each numbered
        where groups side by side share it (`Strings/Pump#2`), counting as
        a page reads: down, then across."""
        if list(self.names.values()).count(self.names[gid]) == 1:
            return self.names[gid]
        names = []
        while gid is not None:
            name, parent = self.names[gid], self.parent[gid]
            twins = sorted((g for g in self.children(parent) if self.names[g] == name),
                           key=lambda g: (self.levels[parent][g][1], self.levels[parent][g][0]))
            names.append(f"{name}#{twins.index(gid) + 1}" if len(twins) > 1 else name)
            gid = parent
        return "/".join(reversed(names))


class Tour:
    def __init__(self, script, patch, editor, bases=None, dry=False):
        self.s = script
        script.tour = self
        self.map = PatchMap(patch)
        self.ed = editor
        x0, y0, x1, y1 = editor
        self.c = ((x0 + x1) / 2, (y0 + y1) / 2)
        # About where the view is (a patch point p is drawn at origin +
        # p*zoom), for a hop to know how far it has to go
        self.zoom, self.origin = 1.0, (x0, y0)
        self.level = None
        self.trail = []
        self.dry = dry
        self.dry_t = 0.0
        self.bases = Path(bases) if bases else None
        saved = json.loads(self.bases.read_text()) if self.bases and self.bases.exists() else {}
        self.sizes = saved.get("sizes", {})
        # The previsit's layout dumps: file name to level
        self.dumps = {}
        # (level, module) aimed at by a nominal size, never having been measured
        self.unmeasured = set()
        # Whether the last dry run measured nothing new (set by shots.py)
        self.settled = False
        self.labels = []
        self.holds = []

    # --- Where things are ----------------------------------------------

    def group(self, name):
        """A group on the level on show, by name (or an id, as is)."""
        return self.map.group(name, within=self.level) if isinstance(name, str) else name

    def find(self, module_id, nth=0):
        """The nth module of a kind on the level on show (in the patch's order)."""
        return [k for k, m in self.map.kinds[self.level].items() if m == module_id][nth]

    def size_of(self, key, level=None):
        level = self.level if level is None else level
        size = self.sizes.get(str(level), {}).get(str(key))
        if size is None:
            self.unmeasured.add((level, key))
            return NOMINAL
        return tuple(size)

    def rect_of(self, key, level=None):
        """A module's or group's rect on a level, in patch points."""
        level = self.level if level is None else level
        x, y = self.map.levels[level][key]
        w, h = self.size_of(key, level)
        return (x, y, x + w, y + h)

    def centre_of(self, key, dx=0.0, dy=0.0):
        x0, y0, x1, y1 = self.rect_of(self.group(key))
        return ((x0 + x1) / 2 + dx, (y0 + y1) / 2 + dy)

    def extent(self, keys=None):
        keys = [self.group(k) for k in keys] if keys else [k for k in self.map.levels[self.level]]
        rects = [self.rect_of(k) for k in keys]
        return (min(r[0] for r in rects), min(r[1] for r in rects), max(r[2] for r in rects), max(r[3] for r in rects))

    def middle(self, keys=None):
        """The middle of a level's modules (or of some of them)."""
        x0, y0, x1, y1 = self.extent(keys)
        return ((x0 + x1) / 2, (y0 + y1) / 2)

    def fit_zoom(self, keys=None, margin=30.0, most=ZOOM_MAX):
        """The zoom that fits a level's modules (or some of them) in the editor."""
        x0, y0, x1, y1 = self.extent(keys)
        ex0, ey0, ex1, ey1 = self.ed
        z = min((ex1 - ex0 - 2 * margin) / (x1 - x0), (ey1 - ey0 - 2 * margin) / (y1 - y0))
        return max(ZOOM_MIN, min(most, z))

    def framed(self):
        """Where the app frames the level on show as it dives in: the
        editor-centre point and zoom, as `look` takes them. The whole level
        sits in the middle of the room below the trail."""
        x0, y0, x1, y1 = self.extent()
        ex0, ey0, ex1, ey1 = self.ed
        room = (ex1 - ex0 - 2 * FRAME_MARGIN, ey1 - ey0 - TRAIL_HEIGHT - 2 * FRAME_MARGIN)
        zoom = max(ZOOM_MIN, min(1.0, room[0] / (x1 - x0), room[1] / (y1 - y0)))
        return ((x0 + x1) / 2, (y0 + y1) / 2 - TRAIL_HEIGHT / 2 / zoom), zoom

    def screen(self, p):
        return (self.origin[0] + p[0] * self.zoom, self.origin[1] + p[1] * self.zoom)

    # --- Time ----------------------------------------------------------

    def _when(self, t, dry_len, fast):
        """When a move starts: as asked, or packed tight on a dry run."""
        if not self.dry or fast:
            return t
        start = self.dry_t
        self.dry_t += dry_len
        return start

    def _close_hold(self, t):
        if self.holds and self.holds[-1][1] is None:
            t0, _, subject, note = self.holds[-1]
            self.holds[-1] = (t0, t, subject, note)

    def end(self, t):
        """The last shot's hold ends here."""
        self._close_hold(t)
        if self.unmeasured and not self.dry:
            print(f"tour: aimed at {len(self.unmeasured)} modules never measured: dry-run first")
        return self.dry_t + 0.5 if self.dry else t

    def _still(self, t, note):
        self.s.at(t, f"still m{len(self.labels):03}")
        self.labels.append(note)

    # --- Moves ---------------------------------------------------------

    def look(self, t, point, zoom, dur, note="", subject=None, fast=False):
        """Glides over `dur` seconds to centre a patch point at a zoom, on the
        level on show. `subject`: the part (or parts) the shot is of. Returns
        when it lands."""
        zoom = max(ZOOM_MIN, min(ZOOM_MAX, zoom))
        t = self._when(t, 0.9, fast)
        if self.dry and not fast:
            dur = 0.3
        self.s.at(t, f"aim {point[0]:.2f} {point[1]:.2f} {zoom:.6f} {round(dur, 4)}" + (f"   # {note}" if note else ""))
        self.zoom, self.origin = zoom, (self.c[0] - point[0] * zoom, self.c[1] - point[1] * zoom)
        if not fast:
            self._close_hold(t)
            if subject is not None:
                self.holds.append((t + dur, None, subject, note))
            if self.dry:
                self._still(t + 0.6, note)
        return t + dur

    def hop(self, t, point, zoom, dur, note="", subject=None):
        """A long way at a close zoom: rises over the first half until both
        ends are in view, then comes down on the point, like a crane, so
        the patch doesn't smear past. A short way is just a look."""
        ex0, ey0, ex1, ey1 = self.ed
        here = ((self.c[0] - self.origin[0]) / self.zoom, (self.c[1] - self.origin[1]) / self.zoom)
        dx, dy = abs(point[0] - here[0]), abs(point[1] - here[1])
        apex = min(self.zoom, zoom, 0.8 * (ex1 - ex0) / max(dx, 1), 0.8 * (ey1 - ey0) / max(dy, 1))
        if apex > 0.8 * min(self.zoom, zoom):
            return self.look(t, point, zoom, dur, note, subject)
        mid = ((here[0] + point[0]) / 2, (here[1] + point[1]) / 2)
        t1 = self.look(t, mid, apex, dur / 2, f"{note} (over)" if note else "")
        return self.look(t1 if not self.dry else t, point, zoom, dur / 2, note, subject)

    def inside(self, t, until, focus=None, zoom=1.3, note="", subject=None):
        """In a group: hold on the whole level, where the dive framed it, then
        drift in on one module (an id, or the first of a kind) until `until`."""
        whole, fit = self.framed()
        t = self.look(t, whole, fit, 0.9, f"{note}: the whole group", subject)
        if focus is not None:
            key = self.find(focus) if isinstance(focus, str) else focus
            t = self.look(t + 0.3, self.centre_of(key), zoom, max(1.0, until - t - 0.3), f"{note}: in on {focus}", subject)
        return t

    def inside_tall(self, t, until, start, focus, zoom=1.5, note="", subject=None):
        """In a group, in a portrait frame, where a whole level is a thin strip:
        open on one module, then drift along to another."""
        first = self.find(start) if isinstance(start, str) else start
        t = self.look(t, self.centre_of(first), zoom * 0.85, 0.9, f"{note}: {start}", subject)
        key = self.find(focus) if isinstance(focus, str) else focus
        return self.look(t + 0.3, self.centre_of(key), zoom, max(1.0, until - t - 0.3), f"{note}: along to {focus}", subject)

    def enter(self, t, group, note="", fast=False, land=None, subject=None):
        """Dives into a group on the level on show, through its miniature.
        `land=(focus, zoom)` lands the dive on one module inside (an id, or
        the first of a kind) rather than on the whole level, so the zoom
        through the miniature and the push in on it are one move: for a
        portrait cut, where a wide group dives in as a thin strip.
        Returns when the camera may move again."""
        gid = self.group(group)
        t = self._when(t, 1.3, fast)
        self.trail.append((gid, self.zoom, self.origin))
        self.level = gid
        if land is None:
            self.s.at(t, f"enter {self.map.path(gid)}")
            # The app frames all of the level, never closer than 1:1
            (mx, my), self.zoom = self.framed()
        else:
            focus, zoom = land
            key = self.find(focus) if isinstance(focus, str) else focus
            (mx, my), self.zoom = self.centre_of(key), max(ZOOM_MIN, min(ZOOM_MAX, zoom))
            self.s.at(t, f"enter {self.map.path(gid)} at {mx:.2f} {my:.2f} {self.zoom:.6f}")
        self.origin = (self.c[0] - mx * self.zoom, self.c[1] - my * self.zoom)
        if not fast:
            self._close_hold(t)
            if subject is not None:
                self.holds.append((t + DIVE, None, subject, note))
        return self._arrive(t + DIVE + 0.1, note or f"into {self.map.names[gid]}", fast)

    def leave(self, t, note="", fast=False):
        """Dives back out to the level above. Returns when the camera may
        move again."""
        gid, self.zoom, self.origin = self.trail.pop()
        t = self._when(t, 1.3, fast)
        self.s.at(t, "leave")
        # The app goes back to the view the group was entered from
        self.level = self.map.parent[gid]
        if not fast:
            self._close_hold(t)
        return self._arrive(t + DIVE + 0.1, note or f"out of {self.map.names[gid]}", fast)

    def _arrive(self, t, note, fast):
        """A level change lands: on a dry run, a still of where."""
        if self.dry and not fast:
            self._still(t + 0.1, note)
        return t + 0.15

    def previsit(self, t):
        """Opens every group once, nested ones too, before recording starts
        (call it at a negative time, first). A module is measured the first
        time it's drawn; until then its group's miniature, and the framing
        of its level, use nominal sizes. Dumps each level's layout for
        `learn`, and leaves the view as it found it."""
        for gid in self.map.children(None):
            t = self._visit(t, gid)
        # The top last, once the window has settled and every group's face
        # shows its measured insides
        self._dump(t, None)
        return t + 0.05

    def _visit(self, t, gid):
        # Dumped once the dive has landed and the level is drawn
        t = self.enter(t, gid, fast=True)
        self._dump(t, gid)
        t += 0.05
        for child in self.map.children(gid):
            t = self._visit(t, child)
        return self.leave(t, fast=True) + 0.05

    def _dump(self, t, level):
        name = f"lvl{len(self.dumps):03}"
        self.dumps[name] = level
        self.s.at(t, f"layout {name}")

    # --- What a run saw ------------------------------------------------

    def learn(self, out_dir):
        """Keeps the sizes the previsit's layouts measured in the film's aims
        file, for the next run to aim from. Returns whether they're the sizes
        this run aimed from: nothing new, and nothing changed."""
        out_dir = Path(out_dir)
        sizes = {level: dict(found) for level, found in self.sizes.items()}
        lost = []
        for name, level in self.dumps.items():
            path = out_dir / f"{name}.json"
            if not path.exists():
                lost.append(f"{self.map.names.get(level, 'the top')} (no layout)")
                continue
            d = json.loads(path.read_text())
            spots = {(round(x), round(y)): key for key, (x, y) in self.map.levels[level].items()}
            found = {}
            for m in d["modules"]:
                x0, y0, x1, y1 = m["rect"]
                key = spots.get((round(x0), round(y0)))
                if key is not None:
                    found[str(key)] = [round(x1 - x0, 1), round(y1 - y0, 1)]
            # The level on show is the one meant if its modules are where the patch has them
            if len(found) < max(1, len(spots) - 1) or len(d["modules"]) != len(spots):
                lost.append(self.map.names.get(level, "the top"))
                continue
            sizes.setdefault(str(level), {}).update(found)
        changed = sum(1 for level, found in sizes.items() for key, size in found.items()
                      if self.sizes.get(level, {}).get(key) != size)
        self.bases.parent.mkdir(parents=True, exist_ok=True)
        self.bases.write_text(json.dumps({"sizes": sizes}, indent=1))
        print(f"tour: {sum(map(len, sizes.values()))} module sizes kept in {self.bases}, {changed} new or changed")
        for name in lost:
            print(f"tour: the previsit's layout of {name} isn't that level")
        return not lost and changed == 0
