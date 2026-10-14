"""A camera that tours a patch through its groups, for capture scripts.

A shot in shots.py hands its Script to a Tour, then aims in patch space on
whatever level the view is on:

    tour = Tour(s, "patches/canopy.json", editor=(0, 74.7, 1536, 838), bases=...)
    tour.previsit(-40)                       # every group opened once, unseen
    tour.look(2.0, tour.centre_of(tour.group("Kalimba")), 1.6, 3.0, "Kalimba", subject="kalimba")
    tour.enter(6.0, "Kalimba")               # a double-click: the app dives in
    tour.look(6.7, tour.middle(), 1.2, 2.0, "inside the Kalimba")
    tour.leave(12.0)                         # Escape: the app dives back out

How the view moves:

- A `camera` cue is relative: from its last state (s, o) to (s2, o2) it zooms
  the view by s2/s about the editor's centre, then pans by o2 - o*s2/s. The
  Tour keeps the view (zoom, origin: a patch point p is drawn at
  origin + p*zoom) and the camera's state, and works out each cue from them.
- Coming out of a group puts its node back where it was on screen, at the
  zoom of the moment: that's worked out. Going in keeps the zoom and frames
  the level, which depends on what the app has measured of its modules:
  that's measured. A dry run (`shots.py <shot> --dry`) makes the same moves
  packed tight and dumps the view at every arrival; `learn` keeps each dive's
  landing in the film's aims file (films/<film>/aims*.json), by level, zoom
  and first visit or not, for the next run to aim from. A dive aimed from a
  guess can miss its group, so dry-run until no arrival is left unmeasured
  (`shots.py <shot> --converge`): one to four runs for a long tour.
- A level's modules come from the patch, a group by `group("Bells")` or any
  node by its id; their sizes from the dry run's layouts, so `middle()`,
  `centre_of()` and `fit_zoom()` aim at what is really drawn.
- Each `look` can name its `subject`, the part it's a shot of; `holds` lists
  them for activity.py to check against what is playing.
"""

import json
from pathlib import Path

ZOOM_MIN, ZOOM_MAX = 0.2, 2.0
# A module never measured yet, at zoom 1
NOMINAL = (206.0, 280.0)
# A dive takes 0.36 s, from the click
DIVE = 0.36
# From the first press of a double-click to its last release
CLICKS = 0.21

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


class Tour:
    def __init__(self, script, patch, editor, bases=None, dry=False, park=None):
        self.s = script
        script.tour = self
        self.map = PatchMap(patch)
        self.ed = editor
        x0, y0, x1, y1 = editor
        self.c = ((x0 + x1) / 2, (y0 + y1) / 2)
        # Where the pointer waits between clicks: on the status bar
        self.park = park or (x0 + 60, y1 + 12)
        self.zoom, self.origin = 1.0, (x0, y0)
        self.cam = (1.0, (0.0, 0.0))
        self.level = None
        self.trail = []
        self.dry = dry
        self.dry_t = 0.0
        self.bases = Path(bases) if bases else None
        saved = json.loads(self.bases.read_text()) if self.bases and self.bases.exists() else {}
        self.entries = saved.get("entries", {})
        self.sizes = saved.get("sizes", {})
        self.visited = set()
        self.landed = []
        self.unmeasured = 0
        # Whether the last dry run saw every arrival land (set by shots.py)
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
        return tuple(self.sizes.get(str(level), {}).get(str(key), NOMINAL))

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
        if self.unmeasured:
            print(f"tour: {self.unmeasured} of {len(self.landed)} arrivals not measured yet: dry-run first")
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
        r = zoom / self.zoom
        s, o = self.cam
        target = (self.c[0] - point[0] * zoom, self.c[1] - point[1] * zoom)
        o2 = tuple(target[i] - self.c[i] - (self.origin[i] - self.c[i]) * r + o[i] * r for i in range(2))
        self.s.at(t, f"camera {o2[0]:.2f} {o2[1]:.2f} {s * r:.6f} {round(dur, 4)}" + (f"   # {note}" if note else ""))
        self.cam = (s * r, o2)
        self.zoom, self.origin = zoom, target
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

    def inside(self, t, until, focus=None, zoom=1.3, note="", subject=None, widest=1.0):
        """In a group: settle on the whole level, then drift in on one module
        (an id, or the first of a kind) until `until`."""
        t = self.look(t, self.middle(), self.fit_zoom(most=widest), 0.9, f"{note}: the whole group", subject)
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

    def double_click(self, t, p):
        """Double-clicks a screen point. The pointer jumps there from where it
        waits and back, in a frame each way: gliding, it would cross faces
        and light their miniatures' Open pills on the way."""
        self.s.at(t - 0.3, f"move {p[0]:.1f} {p[1]:.1f} 0")
        self.s.at(t, "press")
        self.s.at(t + 0.07, "release")
        self.s.at(t + 0.14, "press")
        self.s.at(t + CLICKS, "release")
        self.s.at(t + CLICKS + 0.08, f"move {self.park[0]:.1f} {self.park[1]:.1f} 0")

    def enter(self, t, group, note="", fast=False):
        """Opens a group on show with a double-click on its title bar, and
        the app dives in through its miniature. `t` is the first press.
        Returns when the camera may move again."""
        gid = self.group(group)
        t = self._when(t, 1.6, fast)
        if self.dry and not fast:
            t += 0.35
        x0, y0, x1, y1 = self.rect_of(gid)
        self.double_click(t, self.screen(((x0 + x1) / 2 + 12, y0 + 11)))
        self.trail.append((gid, self.screen((x0, y0))))
        self.level = gid
        if not fast:
            self._close_hold(t)
        return self._arrive(t + CLICKS + DIVE + 0.1, note or f"into {self.map.names[gid]}", fast, entering=True)

    def leave(self, t, note="", fast=False):
        """Escape: the app dives back out to the level above. Returns when
        the camera may move again."""
        gid, at = self.trail.pop()
        t = self._when(t, 1.3, fast)
        self.s.at(t, "key Escape 0.05")
        self.level = self.map.parent[gid]
        # The group's node comes back where it was on screen, at today's
        # zoom (the measured arrival has the last word)
        x, y = self.map.levels[self.level][gid]
        self.origin = (at[0] - x * self.zoom, at[1] - y * self.zoom)
        if not fast:
            self._close_hold(t)
        return self._arrive(t + DIVE + 0.1, note or f"out of {self.map.names[gid]}", fast)

    def _arrive(self, t, note, fast, entering=False):
        """A level change lands. Coming out is worked out (the group's node
        goes back where it was on screen); going in is as a dry run saw it
        land, which depends only on the level, the zoom and whether the
        level has been drawn before."""
        k = len(self.landed)
        key = None
        if entering:
            key = f"{self.level}@{self.zoom:.3f}" + ("" if self.level in self.visited else " first")
            self.visited.add(self.level)
            if key in self.entries:
                self.origin = tuple(self.entries[key])
            else:
                self.unmeasured += 1
        self.landed.append({"level": self.level, "zoom": self.zoom, "origin": self.origin, "note": note, "key": key})
        if self.dry:
            self.s.at(t + 0.1, f"layout arr{k:03}")
            if not fast:
                self._still(t + 0.1, note)
        return t + 0.15
    def previsit(self, t):
        """Opens every group once, nested ones too, before recording starts
        (call it at a negative time, first). A module is measured the first
        time it's drawn; until then its group's miniature, and the view
        into it, use nominal sizes. Leaves the view as it found it, at
        zoom 1."""
        for gid in self.map.children(None):
            t = self.look(t, self.centre_of(gid), 1.0, 0.05, fast=True) + 0.4
            t = self._visit(t, gid)
        return t

    def _visit(self, t, gid):
        t = self.enter(t, gid, fast=True) + 0.05
        for child in self.map.children(gid):
            # In a narrow window a nested group can sit off the edge
            t = self.look(t, self.centre_of(child), self.zoom, 0.05, fast=True) + 0.4
            t = self._visit(t, child)
        return self.leave(t, fast=True) + 0.35

    # --- What a dry run saw --------------------------------------------

    def learn(self, out_dir):
        """Keeps where a dry run's dives landed, and the sizes its layouts
        measured, in the film's aims file for the next run to aim from. Says
        how far the worked-out arrivals were from where the view really went."""
        out_dir = Path(out_dir)
        entries, sizes = dict(self.entries), dict(self.sizes)
        worst, lost = 0.0, []
        for k, land in enumerate(self.landed):
            path = out_dir / f"arr{k:03}.json"
            if not path.exists():
                lost.append(f"{k} {land['note']} (no layout)")
                continue
            d = json.loads(path.read_text())
            spots = {(round(x), round(y)): key for key, (x, y) in self.map.levels[land["level"]].items()}
            found = {}
            for m in d["modules"]:
                x0, y0, x1, y1 = m["rect"]
                key = spots.get((round(x0), round(y0)))
                if key is not None:
                    found[str(key)] = [round(x1 - x0, 1), round(y1 - y0, 1)]
            # The level on show is the one meant if its modules are where the patch has them
            if len(found) < max(1, len(spots) - 1) or len(d["modules"]) != len(spots):
                lost.append(f"{k} {land['note']}")
                continue
            sizes.setdefault(str(land["level"]), {}).update(found)
            # Rounded, so a dry run that lands the same doesn't churn the file
            origin = [round(v, 2) for v in d["view"]["origin"]]
            if land["key"]:
                entries[land["key"]] = origin
            elif abs(d["view"]["zoom"] - land["zoom"]) < 1e-3:
                worst = max(worst, abs(origin[0] - land["origin"][0]), abs(origin[1] - land["origin"][1]))
        self.bases.parent.mkdir(parents=True, exist_ok=True)
        self.bases.write_text(json.dumps({"entries": entries, "sizes": sizes}, indent=1))
        print(f"tour: {len(entries)} dives kept in {self.bases}; worked-out arrivals off by {worst:.1f} pt at most")
        for line in lost:
            print(f"tour: arrival {line} didn't land on its level")
        return not lost
