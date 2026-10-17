"""Builds Soba's CLAP plugin (plugin/) into dist/Soba.clap, and installs it.

    python tools/plugin_bundle.py              # build dist/Soba.clap
    python tools/plugin_bundle.py --install    # ...and copy it to your CLAP folder

On Windows and Linux a .clap is the plugin's library, renamed. On macOS it's
a bundle: Soba.clap/Contents/MacOS/Soba, with an Info.plist beside it.

The build goes to target/plugin (or $CARGO_TARGET_DIR), so a running copy of
the app or another build doesn't lock it.
"""

import argparse
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

INFO_PLIST = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>Soba</string>
    <key>CFBundleIdentifier</key><string>dev.chaps.soba</string>
    <key>CFBundleName</key><string>Soba</string>
    <key>CFBundlePackageType</key><string>BNDL</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
    <key>CFBundleVersion</key><string>{version}</string>
</dict>
</plist>
"""


def library_name() -> str:
    system = platform.system()
    if system == "Windows":
        return "soba_plugin.dll"
    if system == "Darwin":
        return "libsoba_plugin.dylib"
    return "libsoba_plugin.so"


def clap_folder() -> Path:
    """Where DAWs look for the user's own CLAP plugins."""
    system = platform.system()
    if system == "Windows":
        return Path(os.environ["LOCALAPPDATA"]) / "Programs" / "Common" / "CLAP"
    if system == "Darwin":
        return Path.home() / "Library" / "Audio" / "Plug-Ins" / "CLAP"
    return Path.home() / ".clap"


def version() -> str:
    for line in (ROOT / "plugin" / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("version"):
            return line.split('"')[1]
    return "0.0.0"


def build(target_dir: Path) -> Path:
    env = dict(os.environ, CARGO_TARGET_DIR=str(target_dir))
    subprocess.run(["cargo", "build", "--release", "-p", "soba-plugin"], cwd=ROOT, env=env, check=True)
    return target_dir / "release" / library_name()


def bundle(library: Path, out: Path) -> None:
    if out.is_dir():
        shutil.rmtree(out)
    elif out.exists():
        out.unlink()
    if platform.system() == "Darwin":
        (out / "Contents" / "MacOS").mkdir(parents=True)
        shutil.copy2(library, out / "Contents" / "MacOS" / "Soba")
        (out / "Contents" / "Info.plist").write_text(INFO_PLIST.format(version=version()), encoding="utf-8")
        (out / "Contents" / "PkgInfo").write_text("BNDL????", encoding="ascii")
    else:
        out.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(library, out)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--install", action="store_true", help="copy the plugin to your CLAP folder too")
    parser.add_argument("--out", type=Path, default=ROOT / "dist" / "Soba.clap", help="where to put the plugin")
    args = parser.parse_args()

    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target" / "plugin"))
    library = build(target_dir)
    bundle(library, args.out)
    print(f"Built {args.out}")

    if args.install:
        folder = clap_folder()
        folder.mkdir(parents=True, exist_ok=True)
        bundle(library, folder / "Soba.clap")
        print(f"Installed {folder / 'Soba.clap'}: rescan plugins in your DAW")
    return 0


if __name__ == "__main__":
    sys.exit(main())
