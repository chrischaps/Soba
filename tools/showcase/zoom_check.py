"""Does every example still scale smoothly as the view zooms? (#255, #276)

    python tools/showcase/zoom_check.py [patch ...] [--all] [--jobs N] [--max 0.5] [--plan]

Runs zoom_jitter.py's sweep over each patch in patches/ (or the ones named)
and fails any where something changes size by more than --max patch units as
the view zooms (default 0.5: see zoom_jitter.py). The nightly run's zoom step.

A sweep takes a minute or two, and one at a time the 22 examples took 37
minutes. So, like the picture tools (picture_check.py, #217):

- it sweeps --jobs patches at once (default: picture_check's JOBS, six).
  Captures are frame-stepped, so a patch measures the same whichever order
  they run in
- it skips a patch nothing it holds has changed since it last held still:
  its file, its samples, the source of its modules, and the code every
  module shares. A change to the Tape's face sweeps only the examples with a
  Tape in them. The ledger is picture_check's, under zoom:<patch>, and
  a patch is swept again at least weekly whatever the map says

--all sweeps every patch; a patch named on the command line is always swept.
--plan says what would be swept and why, and sweeps nothing.
"""

import argparse
import json
import subprocess
import sys
import textwrap
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import picture_check
import zoom_jitter
from common import ROOT, require_fresh_build

HERE = Path(__file__).resolve().parent
# The sweep's own code: a change to it sweeps every patch (a change to
# picture_check.py does already)
TOOLS = [HERE / "zoom_check.py", HERE / "zoom_jitter.py", HERE / "tour.py"]


def key(patch):
    return f"zoom:{Path(patch).name}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("patches", nargs="*")
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--jobs", type=int, default=picture_check.JOBS)
    ap.add_argument("--max", type=float, default=0.5)
    ap.add_argument("--plan", action="store_true")
    a = ap.parse_args()

    patches = [Path(p).resolve() for p in a.patches] or sorted((ROOT / "patches").glob("*.json"))
    forced = "--all" if a.all else "named" if a.patches else None
    plan = picture_check.Plan(patches, None, lambda p: picture_check.patch_inputs(p, TOOLS), forced, key)
    if plan.old_exe:
        print("(the capture build is older than HEAD's source, so nothing is recorded as passing at HEAD)")
    for why, skipped in plan.skipped.items():
        print(f"Skipped {len(skipped)}, {why}:")
        print(textwrap.fill(" ".join(p.stem for p in skipped), 100, initial_indent="    ",
                            subsequent_indent="    ", break_on_hyphens=False))
    if plan.skipped and plan.film:
        print()
    width = max((len(p.stem) for p in plan.film), default=0)
    if a.plan:
        for p in plan.film:
            print(f"{p.stem:{width}}  would sweep: {plan.why[p]}")
        return 0

    require_fresh_build(stale_ok=True)
    jobs = max(1, a.jobs)
    lock = threading.Lock()
    start = time.monotonic()

    def sweep(patch):
        """(worst, the table or the error) for one patch, printed as it lands."""
        try:
            rows = zoom_jitter.sweep(patch)
            worst, text = zoom_jitter.worst(rows), zoom_jitter.table(rows)
        except subprocess.CalledProcessError as e:
            worst, text = None, f"the capture failed (exit {e.returncode}):\n{(e.stderr or b'').decode(errors='replace')[-2000:]}"
        verdict = ("FAILED" if worst is None else f"steps {worst:.2f}" if worst > a.max else f"ok {worst:.2f}")
        with lock:
            print(f"{patch.stem:{width}}  {verdict:12} {time.monotonic() - start:5.0f} s  swept: {plan.why[patch]}",
                  flush=True)
            if worst is None or worst > a.max:
                print(textwrap.indent(text, "    "), flush=True)
        return worst

    # The patches with the most frames to sweep go first, so the longest
    # (From One Sine's twelve) doesn't start late and run on alone
    order = sorted(plan.film, key=lambda p: -len(zoom_jitter.centres(json.loads(p.read_text(encoding="utf-8")))))
    with ThreadPoolExecutor(jobs) as pool:
        results = list(zip(order, pool.map(sweep, order)))

    bad = [p.stem for p, worst in results if worst is None or worst > a.max]
    picture_check.write_ledger({key(p): e for p, worst in results if p.stem not in bad and (e := plan.entry(p))})
    skipped = sum(len(n) for n in plan.skipped.values())
    print(f"\nSwept {len(plan.film)} of {len(patches)} ({jobs} at a time) in {(time.monotonic() - start) / 60:.1f} min"
          + (f", skipped {skipped}: --all sweeps them too" if skipped else ""))
    if bad:
        # tools/nightly.py quotes this line in its report
        print(f"Steps in: {', '.join(bad)}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
