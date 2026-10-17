"""Builds Soba's CLAP plugin (plugin/) into dist/Soba.clap, and installs it.

    python tools/plugin_bundle.py                # build dist/Soba.clap
    python tools/plugin_bundle.py --install      # ...and copy it to your CLAP folder
    python tools/plugin_bundle.py --vst3         # dist/Soba.vst3 too (GPLv3: see below)
    python tools/plugin_bundle.py --vst3 --install

On Windows and Linux a .clap is the plugin's library, renamed. On macOS it's
a bundle: Soba.clap/Contents/MacOS/Soba, with an Info.plist beside it.

A .vst3 is a folder everywhere: Soba.vst3/Contents/<arch>/Soba.vst3 (.so on
Linux), or Contents/MacOS/Soba on macOS. It's built with the plugin's vst3
feature, whose wrapper (vst3-sys) is GPLv3, so the .vst3 is GPLv3. The .clap
is built without it, and stays MIT.

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


def vst3_folder() -> Path:
    """Where DAWs look for the user's own VST3 plugins."""
    system = platform.system()
    if system == "Windows":
        return Path(os.environ["LOCALAPPDATA"]) / "Programs" / "Common" / "VST3"
    if system == "Darwin":
        return Path.home() / "Library" / "Audio" / "Plug-Ins" / "VST3"
    return Path.home() / ".vst3"


def version() -> str:
    for line in (ROOT / "plugin" / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("version"):
            return line.split('"')[1]
    return "0.0.0"


def build(target_dir: Path, features: list[str]) -> Path:
    env = dict(os.environ, CARGO_TARGET_DIR=str(target_dir))
    command = ["cargo", "build", "--release", "-p", "soba-plugin"]
    if features:
        command += ["--features", ",".join(features)]
    subprocess.run(command, cwd=ROOT, env=env, check=True)
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


def bundle_vst3(library: Path, out: Path) -> None:
    if out.exists():
        shutil.rmtree(out)
    system = platform.system()
    if system == "Darwin":
        (out / "Contents" / "MacOS").mkdir(parents=True)
        shutil.copy2(library, out / "Contents" / "MacOS" / "Soba")
        (out / "Contents" / "Info.plist").write_text(INFO_PLIST.format(version=version()), encoding="utf-8")
        (out / "Contents" / "PkgInfo").write_text("BNDL????", encoding="ascii")
        return
    machine = platform.machine().lower()
    arch = "aarch64" if machine in ("arm64", "aarch64") else "x86_64"
    if system == "Windows":
        folder, name = out / "Contents" / f"{arch}-win", "Soba.vst3"
    else:
        folder, name = out / "Contents" / f"{arch}-linux", "Soba.so"
    folder.mkdir(parents=True)
    shutil.copy2(library, folder / name)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--install", action="store_true", help="copy the plugin to your CLAP (and VST3) folder too")
    parser.add_argument("--vst3", action="store_true", help="build dist/Soba.vst3 too, for DAWs without CLAP (GPLv3)")
    parser.add_argument("--out", type=Path, default=ROOT / "dist" / "Soba.clap", help="where to put the plugin")
    args = parser.parse_args()

    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target" / "plugin"))
    library = build(target_dir, [])
    bundle(library, args.out)
    print(f"Built {args.out}")
    if args.install:
        folder = clap_folder()
        folder.mkdir(parents=True, exist_ok=True)
        bundle(library, folder / "Soba.clap")
        print(f"Installed {folder / 'Soba.clap'}")

    if args.vst3:
        # The same library with the VST3 export, so the .clap above stays MIT
        library = build(target_dir, ["vst3"])
        vst3 = args.out.with_suffix(".vst3")
        bundle_vst3(library, vst3)
        print(f"Built {vst3} (GPLv3)")
        if args.install:
            folder = vst3_folder()
            folder.mkdir(parents=True, exist_ok=True)
            bundle_vst3(library, folder / "Soba.vst3")
            print(f"Installed {folder / 'Soba.vst3'}")

    if args.install:
        print("Rescan plugins in your DAW")
    return 0


if __name__ == "__main__":
    sys.exit(main())
