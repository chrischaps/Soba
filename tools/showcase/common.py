"""What the showcase tools share: where the repo is, and where the capture
build put its executables.

The capture build is a release build of its own, so a running copy of the
app doesn't lock the exe:

    CARGO_TARGET_DIR=target/capture cargo build --release

The tools look in $CARGO_TARGET_DIR when it is set (a relative one from the
current directory, as cargo reads it), and in target/capture otherwise.
"""

import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# What goes into the build: a commit to any of these makes an older exe stale
BUILD_INPUTS = ("src", "assets", "library", "Cargo.toml", "Cargo.lock", "build.rs")


def target_dir():
    """Where the capture build is."""
    env = os.environ.get("CARGO_TARGET_DIR")
    return Path(env).resolve() if env else ROOT / "target" / "capture"


def exe(name="soba"):
    """One of the capture build's executables: soba or render."""
    return target_dir() / "release" / (name + (".exe" if sys.platform == "win32" else ""))


def build_is_old(path):
    """Whether `path` is missing or predates the last commit to what goes
    into it, so it can't have been built from HEAD."""
    if not path.exists():
        return True
    last = subprocess.run(["git", "log", "-1", "--format=%ct", "--", *BUILD_INPUTS], cwd=ROOT,
                          capture_output=True, text=True).stdout.strip()
    return bool(last) and path.stat().st_mtime < int(last)


def require_fresh_build(name="soba", stale_ok=False):
    """The executable, or exit saying how to build it if it is missing or
    older than HEAD's source: a film of an old build looks right and isn't.
    The picture tools pass `stale_ok`: they film an old build, but don't
    record it as filmed at HEAD."""
    path = exe(name)
    if not path.exists() or (not stale_ok and build_is_old(path)):
        why = "isn't built" if not path.exists() else "is older than HEAD's source"
        target = os.environ.get("CARGO_TARGET_DIR", "target/capture")
        sys.exit(f"{path} {why}. Build it first:\n    CARGO_TARGET_DIR={target} cargo build --release")
    return path
