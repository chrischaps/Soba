"""One stem per part of a patch, from its mixers: each part rendered alone.

    python tools/showcase/stems.py patch.json out_dir [--seconds N] [--jobs N] [--list]

A part is whatever feeds a mixer channel: a group or a module, and a port
of it if it feeds more than one channel (a stereo pair's Out L and Out R are
one part). Each stem is the patch with every other channel of every Mixer
muted, rendered by `render` (target/capture/release/render.exe, built by
`CARGO_TARGET_DIR=target/capture cargo build --release`). So a part's stem
keeps its share of the sends (echo, reverb) and the master chain (tape,
limiter), as it sounds in the mix.

Stems are named from the part, lower case: "kalimba", "vocal-chops",
"drums-kit". activity.py reads them to say what plays when; a film's shots
name their subjects the same way. `--list` prints the parts and their
channels without rendering.

The patch's samples are found next to it, so each muted copy is written
with its sample paths made absolute.
"""

import argparse
import copy
import json
import re
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RENDER = ROOT / "target" / "capture" / "release" / "render.exe"


def slug(text):
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def parts_of(patch):
    """{part name: [(mixer id, channel)]} for every Mixer channel with a cable in."""
    names = {n["id"]: n["module_id"].split(".")[-1] for n in patch["nodes"]}
    names.update({g["id"]: g["name"] for g in patch.get("groups", [])})
    mixers = {n["id"] for n in patch["nodes"] if n["module_id"] == "util.mixer"}
    feeds = {}
    for c in patch["connections"]:
        m = re.fullmatch(r"Ch (\d)", c["to_port"])
        if c["to_node"] in mixers and m:
            port = re.sub(r" [LR]$", "", c["from_port"])
            feeds.setdefault((c["from_node"], port), []).append((c["to_node"], int(m.group(1))))
    ports = {}
    for node, port in feeds:
        ports.setdefault(node, set()).add(port)
    parts = {}
    for (node, port), channels in feeds.items():
        name = names.get(node, str(node))
        if len(ports[node]) > 1:
            name = f"{name} {port}"
        parts[slug(name)] = channels
    return parts


def absolute_samples(value, base):
    """Every relative "file" path in the patch, made absolute."""
    if isinstance(value, dict):
        return {k: (str((base / v).resolve()) if k == "file" and isinstance(v, str) and v and not Path(v).is_absolute()
                    else absolute_samples(v, base)) for k, v in value.items()}
    if isinstance(value, list):
        return [absolute_samples(v, base) for v in value]
    return value


def solo(patch, keep):
    """The patch with every Mixer channel muted but the ones in `keep`."""
    out = copy.deepcopy(patch)
    for n in out["nodes"]:
        if n["module_id"] != "util.mixer":
            continue
        for p in n["parameters"]:
            m = re.fullmatch(r"(Mute|Solo) (\d)", p["name"])
            if m:
                p["value"] = m.group(1) == "Mute" and (n["id"], int(m.group(2))) not in keep
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("patch")
    ap.add_argument("out")
    ap.add_argument("--seconds", type=float, default=60.0)
    ap.add_argument("--jobs", type=int, default=2)
    ap.add_argument("--list", action="store_true")
    a = ap.parse_args()
    path = Path(a.patch).resolve()
    patch = absolute_samples(json.loads(path.read_text(encoding="utf-8")), path.parent)
    parts = parts_of(patch)
    if a.list:
        for name, channels in sorted(parts.items()):
            print(f"{name:20} " + ", ".join(f"mixer {m} ch {c}" for m, c in channels))
        return 0
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="stems-"))

    def render(item):
        name, channels = item
        stem_patch = work / f"{name}.json"
        stem_patch.write_text(json.dumps(solo(patch, set(channels))), encoding="utf-8")
        subprocess.run([str(RENDER), str(stem_patch), str(out / f"{name}.wav"), "--seconds", str(a.seconds)],
                       check=True, capture_output=True)
        print(f"{name} done", flush=True)

    with ThreadPoolExecutor(a.jobs) as pool:
        list(pool.map(render, sorted(parts.items())))
    return 0


if __name__ == "__main__":
    sys.exit(main())
