"""Runs the checks too slow for every ticket, against the tip of origin/master.

    python tools/nightly.py                    every step, if master has moved since the last green run
    python tools/nightly.py --force            every step, even if it hasn't
    python tools/nightly.py --only layout,images
    python tools/nightly.py --ref <commit>     check that commit instead: files no issues, pushes nothing
    python tools/nightly.py --no-issues        report only
    python tools/nightly.py --only perf --rebaseline
                                               take tonight's timings as the new
                                               baseline, even if they're slower

Steps, in order (--only picks some):

    tests      cargo test
    loudness   the ignored example_loudness test: every example at length
    idle       the ignored idle test: every example, stopped, asks for no
               frames, so left open it costs next to nothing (#307)
    perf       render --bench on a few heavy examples, best of three: fails
               if any renders more than 15% slower than the stored baseline,
               the timings of the last run it passed (#347). The renders
               run at normal priority, one core at a time
    layout     check_layout.py: every example's modules inside their frames
    zoom       zoom_check.py: zoom_jitter.py --max 0.5 on each patch that
               something it holds has changed since it last passed, several at
               once
    images     the recipe, manual and module pictures; stale ones are re-shot
               and pushed to master as a [Docs] commit of their own. Only the
               pictures something they show has changed since are filmed,
               several at once (tools/showcase/picture_check.py)

A ticket checks only what it touched (CLAUDE.md, step 4) and pushes, and
GitHub CI runs the full cargo test on every push. This is the rest: the
checks that need this machine's window and pixels, or take long enough to
make it unusable. Tools/nightly_schedule.ps1 runs it at night.

It checks the commit out into its own detached worktree, ../Modular-nightly,
which builds into its own target/ and target/capture, so it never touches a
session's working tree or locks an exe someone is running. Every process it
starts runs at below-normal priority, cargo on all but two cores. One run at
a time: a second exits at once.

A failing step opens an issue labelled nightly-failure ("Nightly: <step>
fails on master"), or comments on the one already open; a step that passes
again closes it. The report and each step's log go to target/nightly/<stamp>/
in the main checkout. Exits 1 if any step failed.

The perf baseline is soba-nightly-perf.json in the git directory, beside the
lock: timings belong to the machine that took them, so it isn't committed.
The first run has nothing to compare with and writes it. A run that passes
replaces it, so the gate catches a step slower, not a slow creep; a change
that is slower on purpose is taken with --rebaseline. A --ref run compares
but never writes it.
"""

import datetime
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

STEPS = ["tests", "loudness", "idle", "perf", "layout", "zoom", "images"]
CAPTURE_STEPS = {"layout", "zoom", "images"}
LABEL = "nightly-failure"
STEP_TIMEOUT = 3 * 60 * 60
LOG_TAIL = 40
WINDOWS = os.name == "nt"

# The examples perf times, and how each is played: the heaviest songs, and a
# patch that waits for a player
PERF_PATCHES = [
    ("canopy", []),
    ("clair-de-satellite", []),
    ("orrery", []),
    ("lush-pad", ["--audition"]),
    ("afterglow", []),
]
PERF_SECONDS = 20
PERF_RUNS = 3
# How much slower than the baseline a render may be. Run to run, one render
# of canopy varies by about a fifth; the best of three by much less
PERF_TOLERANCE = 0.15
BENCH_LINE = re.compile(r"^bench: ([0-9.]+) s ")


def run(*args, check=True, cwd=None):
    result = subprocess.run(args, capture_output=True, text=True, cwd=cwd)
    if check and result.returncode != 0:
        sys.exit(f"{' '.join(args)} failed:\n{result.stderr.strip()}")
    return result


def main_checkout():
    """The first worktree git lists is the main checkout; the nightly worktree sits beside it."""
    porcelain = run("git", "worktree", "list", "--porcelain").stdout
    return Path(porcelain.splitlines()[0].removeprefix("worktree "))


def common_dir(main):
    return Path(run("git", "rev-parse", "--path-format=absolute", "--git-common-dir", cwd=main).stdout.strip())


def alive(pid):
    if WINDOWS:
        import ctypes
        kernel32 = ctypes.windll.kernel32
        handle = kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
        if not handle:
            return False
        code = ctypes.c_ulong()
        kernel32.GetExitCodeProcess(handle, ctypes.byref(code))
        kernel32.CloseHandle(handle)
        return code.value == 259  # STILL_ACTIVE
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def take_lock(path):
    """One run at a time. A lock left by a run that died is taken over."""
    for _ in range(2):
        try:
            fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
            os.write(fd, str(os.getpid()).encode())
            os.close(fd)
            return True
        except FileExistsError:
            try:
                pid = int(path.read_text().strip() or 0)
            except (OSError, ValueError):
                pid = 0
            if pid and alive(pid):
                return False
            path.unlink(missing_ok=True)
    return False


def quiet_env(target=None):
    """Cargo and the tests on all but two cores, so the machine stays usable."""
    env = dict(os.environ)
    jobs = str(max(1, (os.cpu_count() or 4) - 2))
    env["CARGO_BUILD_JOBS"] = jobs
    env["RUST_TEST_THREADS"] = jobs
    env["CARGO_TERM_COLOR"] = "never"
    if target:
        env["CARGO_TARGET_DIR"] = str(target)
    else:
        env.pop("CARGO_TARGET_DIR", None)
    return env


def kill_tree(proc):
    """Only ever the process this script started, and its children, by PID."""
    if WINDOWS:
        subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"], capture_output=True)
    else:
        proc.kill()


def step_run(args, log, cwd, env, low=True):
    """Runs one command at low priority (unless not low), appending its output to log. Returns its exit code."""
    with open(log, "a", encoding="utf-8", errors="replace") as out:
        out.write(f"$ {' '.join(str(a) for a in args)}\n")
        out.flush()
        if not low:
            extra = {}
        elif WINDOWS:
            extra = {"creationflags": subprocess.BELOW_NORMAL_PRIORITY_CLASS}
        else:
            extra = {"preexec_fn": lambda: os.nice(10)}
        proc = subprocess.Popen([str(a) for a in args], cwd=cwd, env=env, stdout=out, stderr=subprocess.STDOUT, **extra)
        try:
            code = proc.wait(timeout=STEP_TIMEOUT)
        except subprocess.TimeoutExpired:
            kill_tree(proc)
            proc.wait()
            out.write(f"\n(killed after {STEP_TIMEOUT // 60} minutes)\n")
            code = -1
        out.write(f"(exit {code})\n\n")
    return code


def tail(log, lines=LOG_TAIL):
    try:
        return "\n".join(log.read_text(encoding="utf-8", errors="replace").splitlines()[-lines:])
    except OSError:
        return "(no log)"


def prepare_worktree(main, sha):
    worktree = main.parent / "Modular-nightly"
    if worktree.exists():
        run("git", "checkout", "--quiet", "--force", "--detach", sha, cwd=worktree)
        run("git", "clean", "-qfd", cwd=worktree)  # leaves target/ alone: it's ignored
    else:
        run("git", "worktree", "add", "--quiet", "--detach", str(worktree), sha, cwd=main)
    return worktree


def push_pictures(worktree, short, log):
    """Commits the re-shot pictures alone and lands them on master. Returns what happened, or None."""
    images = "docs/src/images"
    changed = run("git", "status", "--porcelain", "--", images, cwd=worktree).stdout.split("\n")
    names = sorted(Path(line[3:].strip().strip('"')).name for line in changed if line.strip())
    if not names:
        return None
    message = (f"[Docs] Re-shoot pictures for {short}\n\n"
               + "".join(f"- {n}\n" for n in names)
               + "\nShot by tools/nightly.py from the commit that made them stale.\n")
    # A failure here is the step's note, not an exit: the report and the other steps still matter
    for args in (["add", "--", images], ["commit", "--quiet", "-m", message]):
        result = run("git", *args, cwd=worktree, check=False)
        if result.returncode != 0:
            with open(log, "a", encoding="utf-8") as out:
                out.write(result.stdout + result.stderr)
            return f"re-shot {', '.join(names)}, but git {args[0]} failed; nothing pushed"
    for _ in range(3):
        run("git", "fetch", "--quiet", "origin", cwd=worktree, check=False)
        if run("git", "rebase", "--quiet", "origin/master", cwd=worktree, check=False).returncode != 0:
            run("git", "rebase", "--abort", cwd=worktree, check=False)
            return f"re-shot {', '.join(names)}, but they didn't rebase onto origin/master; nothing pushed"
        # A push to master prints a protected-ref bypass notice on success: go by the exit code
        push = run("git", "push", "--quiet", "origin", "HEAD:refs/heads/master", cwd=worktree, check=False)
        with open(log, "a", encoding="utf-8") as out:
            out.write(push.stderr)
        if push.returncode == 0:
            pushed = run("git", "rev-parse", "--short", "HEAD", cwd=worktree).stdout.strip()
            return f"re-shot and pushed {', '.join(names)} as {pushed}"
    return f"re-shot {', '.join(names)}, but the push was rejected three times; nothing pushed"


def bench_examples(worktree, log, env):
    """Times each of PERF_PATCHES with render --bench: {name: seconds}, or None if the build fails."""
    if step_run(["cargo", "build", "--release", "--bin", "render"], log, worktree, env) != 0:
        return None
    render = worktree / "target" / "release" / ("render.exe" if WINDOWS else "render")
    out = worktree / "target" / "perf"
    out.mkdir(parents=True, exist_ok=True)
    timings = {}
    # At normal priority: below it, anything else running slows a render by
    # a quarter. A render is one thread, so the machine keeps its other cores
    for name, flags in PERF_PATCHES:
        before = log.stat().st_size
        step_run([render, Path("patches") / f"{name}.json", out / f"{name}.wav", "--seconds", PERF_SECONDS,
                  "--bench", "--runs", PERF_RUNS, *flags], log, worktree, env, low=False)
        with open(log, encoding="utf-8", errors="replace") as text:
            text.seek(before)
            found = [m.group(1) for m in map(BENCH_LINE.match, text.read().splitlines()) if m]
        if found:
            timings[name] = float(found[-1])
    return timings


def judge_perf(timings, baseline_path, log):
    """Compares timings with the baseline. Returns (passed, note), and writes how each moved to log."""
    try:
        baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        baseline = {}
    base = baseline.get("seconds", {})
    since = baseline.get("commit", "?")[:7]
    slower, missing, lines = [], [], []
    for name, _ in PERF_PATCHES:
        now = timings.get(name)
        if now is None:
            missing.append(name)
            lines.append(f"{name:20} no bench line: see above")
        elif name in base:
            change = now / base[name] - 1
            lines.append(f"{name:20} {now:8.3f} s   was {base[name]:8.3f} s at {since}   {change:+.0%}")
            if change > PERF_TOLERANCE:
                slower.append(f"{name} {change:+.0%}")
        else:
            lines.append(f"{name:20} {now:8.3f} s   (no baseline)")
    with open(log, "a", encoding="utf-8") as out:
        out.write("\n".join(lines) + "\n")
    notes = []
    if slower:
        notes.append(f"slower than at {since}: {', '.join(slower)}")
    if missing:
        notes.append(f"didn't render: {', '.join(missing)}")
    if not base:
        notes.append("no baseline yet")
    return not slower and not missing, "; ".join(notes)


def run_steps(steps, worktree, logs, push, perf_baseline=None, rebaseline=False):
    """Each step's result: (passed, minutes, note)."""
    results = {}
    env = quiet_env()
    python = sys.executable
    showcase = worktree / "tools" / "showcase"
    capture_ok = None

    def log_of(step):
        return logs / f"{step}.log"

    for step in steps:
        start = time.monotonic()
        log = log_of(step)
        note = ""
        if step in CAPTURE_STEPS and capture_ok is None:
            capture_ok = step_run(["cargo", "build", "--release"], logs / "capture-build.log", worktree,
                                  quiet_env(worktree / "target" / "capture")) == 0
        if step in CAPTURE_STEPS and not capture_ok:
            log.write_text("the capture build failed: see capture-build.log\n" + tail(logs / "capture-build.log"),
                           encoding="utf-8")
            passed = False
        elif step == "tests":
            passed = step_run(["cargo", "test"], log, worktree, env) == 0
        elif step == "loudness":
            passed = step_run(["cargo", "test", "--release", "--test", "example_loudness", "--",
                               "--ignored", "--nocapture"], log, worktree, env) == 0
        elif step == "idle":
            passed = step_run(["cargo", "test", "--release", "--lib", "a_stopped_example_asks_for_no_frames", "--",
                               "--ignored", "--nocapture"], log, worktree, env) == 0
        elif step == "perf":
            timings = bench_examples(worktree, log, env)
            if timings is None:
                passed, note = False, "render didn't build"
            else:
                passed, note = judge_perf(timings, perf_baseline, log)
                if rebaseline and len(timings) == len(PERF_PATCHES):
                    passed, note = True, "; ".join(filter(None, ["rebaselined", note]))
                # Only a run of master keeps its timings, and only one with every example timed
                if push and passed and len(timings) == len(PERF_PATCHES):
                    sha = run("git", "rev-parse", "HEAD", cwd=worktree).stdout.strip()
                    perf_baseline.write_text(json.dumps({"commit": sha, "seconds": timings}, indent=2) + "\n",
                                             encoding="utf-8")
        elif step == "layout":
            passed = step_run([python, showcase / "check_layout.py"], log, worktree, env) == 0
        elif step == "zoom":
            passed = step_run([python, showcase / "zoom_check.py"], log, worktree, env) == 0
            steps_in = [line for line in tail(log).splitlines() if line.startswith("Steps in: ")]
            note = steps_in[-1].lower() if steps_in else ""
        elif step == "images":
            # Every tool runs even when one fails, so one bad capture can't leave the rest stale
            tools = ["recipe_images.py", "module_images.py", "manual_images.py"]
            failed = [tool for tool in tools
                      if step_run([python, showcase / tool, "--refresh"], log, worktree, env) != 0]
            passed = not failed
            if push:
                short = run("git", "rev-parse", "--short", "HEAD", cwd=worktree).stdout.strip()
                note = push_pictures(worktree, short, log) or ""
            else:
                note = "re-shot in the nightly worktree only; nothing pushed"
            if failed:
                note = "; ".join(filter(None, [f"{', '.join(failed)} failed", note]))
        results[step] = (passed, (time.monotonic() - start) / 60, note)
        print(f"  {step:9} {'ok' if passed else 'FAILED'} in {results[step][1]:.1f} min  {note}".rstrip(), flush=True)
    return results


def ensure_label():
    run("gh", "label", "create", LABEL, "--color", "B60205",
        "--description", "A check tools/nightly.py runs fails on master", check=False)


def file_issues(results, sha, logs, report):
    ensure_label()
    listed = run("gh", "issue", "list", "--label", LABEL, "--state", "open", "--limit", "50",
                 "--json", "number,title", "-q", '.[] | "\\(.number)\\t\\(.title)"').stdout
    open_issues = {title: number for number, title in
                   (line.split("\t", 1) for line in listed.splitlines() if "\t" in line)}
    short = sha[:7]
    for step, (passed, _, note) in results.items():
        title = f"Nightly: {step} fails on master"
        number = open_issues.get(title)
        if passed and number:
            run("gh", "issue", "close", number, "--comment", f"Passes again at {short}.", check=False)
        elif not passed:
            body = (f"`python tools/nightly.py --only {step}` fails at {short}"
                    f"{f' ({note})' if note else ''}. The full log is {logs / (step + '.log')} "
                    f"on the machine that ran it; the report is {report}.\n\n"
                    f"Last lines:\n\n```\n{tail(logs / (step + '.log'))}\n```\n")
            if number:
                run("gh", "issue", "comment", number, "--body", body, check=False)
            else:
                run("gh", "issue", "create", "--title", title, "--label", LABEL, "--body", body, check=False)


def write_report(path, sha, results):
    lines = [f"# Nightly {path.parent.name}", "", f"Commit {sha}", "",
             "| Step | Result | Minutes | Note |", "|---|---|---|---|"]
    for step, (passed, minutes, note) in results.items():
        lines.append(f"| {step} | {'ok' if passed else '**failed**'} | {minutes:.1f} | {note} |")
    for step, (passed, _, _) in results.items():
        if not passed:
            lines += ["", f"## {step}", "", "```", tail(path.parent / f"{step}.log"), "```"]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main():
    args = sys.argv[1:]
    force = "--force" in args
    issues = "--no-issues" not in args
    rebaseline = "--rebaseline" in args
    steps, ref = STEPS, None
    for flag in ("--only", "--ref"):
        if flag in args:
            i = args.index(flag)
            if i + 1 >= len(args):
                sys.exit(__doc__)
            value = args[i + 1]
            if flag == "--only":
                steps = [s for s in STEPS if s in value.split(",")]
                if not steps or set(value.split(",")) - set(STEPS):
                    sys.exit(f"--only takes some of: {','.join(STEPS)}")
            else:
                ref = value

    checkout = main_checkout()
    git_dir = common_dir(checkout)
    lock = git_dir / "soba-nightly.lock"
    if not take_lock(lock):
        sys.exit(f"a nightly run is already going (PID {lock.read_text().strip()})")
    try:
        run("git", "fetch", "--quiet", "origin", cwd=checkout)
        # A --ref is read where it was run from, so HEAD is the caller's worktree
        sha = (run("git", "rev-parse", "--verify", f"{ref}^{{commit}}").stdout.strip() if ref
               else run("git", "rev-parse", "origin/master", cwd=checkout).stdout.strip())
        last_green = git_dir / "soba-nightly-last-green"
        whole = steps == STEPS and ref is None
        if whole and not force and last_green.exists() and last_green.read_text().strip() == sha:
            print(f"nothing new: {sha[:7]} passed every step last time (--force runs it anyway)")
            return 0

        stamp = datetime.datetime.now().strftime("%Y-%m-%d-%H%M")
        logs = checkout / "target" / "nightly" / stamp
        logs.mkdir(parents=True, exist_ok=True)
        worktree = prepare_worktree(checkout, sha)
        print(f"nightly: {sha[:7]} in {worktree}: {', '.join(steps)} (logs in {logs})", flush=True)

        results = run_steps(steps, worktree, logs, push=issues and ref is None,
                            perf_baseline=git_dir / "soba-nightly-perf.json", rebaseline=rebaseline)
        report = logs / "report.md"
        write_report(report, sha, results)
        if issues and ref is None:
            file_issues(results, sha, logs, report)
        failed = [s for s, (passed, _, _) in results.items() if not passed]
        if whole and not failed:
            last_green.write_text(sha + "\n")
        print(f"nightly: {'FAILED: ' + ', '.join(failed) if failed else 'all green'}. Report: {report}")
        return 1 if failed else 0
    finally:
        lock.unlink(missing_ok=True)


if __name__ == "__main__":
    sys.exit(main())
