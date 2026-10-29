"""Builds the browser app twice into one dist/, for soba.chaps.dev (#375).

    python tools/web/build.py [--dist dist]
    python tools/web/build.py --check     # just cargo check the threaded build (CI)

The plain build is `trunk build --release --cargo-profile dist`: it renders sound on
the page's own thread, between frames (cpal's WebAudio host). The threaded
build adds wasm threads and the `web-worklet` feature, so it can render in an
AudioWorklet on the browser's audio thread instead, where drawing a big patch
can't make the sound late. It needs a nightly toolchain for `-Z build-std`:

    rustup toolchain install nightly-2026-10-17 --component rust-src --target wasm32-unknown-unknown

Both go into dist/, and the loader trunk wrote into index.html is swapped
for one that asks index.html's sobaIsolate() which to run: the threaded
build once the page is cross-origin isolated (isolate-sw.js), and the plain
one where it can't be (embeds, Safari for now, browsers without service
workers). WEB.md has the measurements.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
NIGHTLY = "nightly-2026-10-17"
# Both builds take Cargo.toml's dist profile, fat LTO: a quarter smaller
# (5-6% served gzipped) and a little faster in Chrome, for longer to build
# (#494; WEB.md has the numbers)
PROFILE = ["--release", "--cargo-profile", "dist"]
# Its own target folder, so the two builds don't keep rebuilding each other
THREADS_TARGET = ROOT / "target" / "web-threads"

# Shared memory, imported so each thread (the page, the worklet) can be
# handed it, and the thread-local storage wasm-bindgen sets up per thread.
# simd128 as in .cargo/config.toml, which RUSTFLAGS replaces
THREADS_RUSTFLAGS = " ".join([
    "-C target-feature=+atomics,+bulk-memory,+simd128",
    "-C link-arg=--shared-memory",
    "-C link-arg=--max-memory=2147483648",
    "-C link-arg=--import-memory",
    "-C link-arg=--export=__wasm_init_tls",
    "-C link-arg=--export=__tls_size",
    "-C link-arg=--export=__tls_align",
    "-C link-arg=--export=__tls_base",
    "-C link-arg=--export=__heap_base",
])

# What trunk writes into index.html to start the app
TRUNK_LOADER = re.compile(
    r"import init, \* as bindings from '\./(?P<name>soba-[0-9a-f]+)\.js';\s*"
    r"const wasm = await init\(\{ module_or_path: '\./(?P=name)_bg\.wasm' \}\);"
)
# ...and the preloads it puts before it, which would fetch the plain build
# whichever runs
TRUNK_PRELOADS = re.compile(r'<link rel="modulepreload" href="\./soba-[0-9a-f]+\.js"[^>]*>'
                            r'<link rel="preload" href="\./soba-[0-9a-f]+_bg\.wasm"[^>]*>')


def threads_env():
    env = dict(os.environ)
    env.update({
        "RUSTUP_TOOLCHAIN": NIGHTLY,
        "RUSTFLAGS": THREADS_RUSTFLAGS,
        "CARGO_UNSTABLE_BUILD_STD": "std,panic_abort",
        "CARGO_TARGET_DIR": str(THREADS_TARGET),
    })
    return env


def run(command, env=None):
    print("+", " ".join(command), flush=True)
    subprocess.run(command, cwd=ROOT, env=env, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dist", default="dist", help="where the site goes (default dist/)")
    parser.add_argument("--check", action="store_true", help="only cargo check the threaded build")
    args = parser.parse_args()

    if args.check:
        env = threads_env()
        env["RUSTFLAGS"] += " -D warnings"
        run(["cargo", "check", "--locked", "--target", "wasm32-unknown-unknown", "--bin", "soba",
             "--features", "web-worklet"], env)
        return

    dist = ROOT / args.dist
    threads_dist = THREADS_TARGET / "dist"
    run(["trunk", "build", *PROFILE, "--dist", str(dist)])
    run(["trunk", "build", *PROFILE, "--features", "web-worklet", "--dist", str(threads_dist)], threads_env())

    index = dist / "index.html"
    html = index.read_text(encoding="utf-8")
    plain = TRUNK_LOADER.search(html)
    threaded = TRUNK_LOADER.search((threads_dist / "index.html").read_text(encoding="utf-8"))
    if not plain or not threaded:
        sys.exit("build.py: trunk's loader in index.html isn't the one expected; update TRUNK_LOADER")
    for suffix in (".js", "_bg.wasm"):
        shutil.copy2(threads_dist / (threaded["name"] + suffix), dist)

    loader = (
        f"const build = (await window.sobaIsolate()) ? './{threaded['name']}' : './{plain['name']}';\n"
        "const bindings = await import(build + '.js');\n"
        "const init = bindings.default;\n"
        "const wasm = await init({ module_or_path: build + '_bg.wasm' });"
    )
    html = TRUNK_LOADER.sub(lambda _: loader, html, count=1)
    html = TRUNK_PRELOADS.sub("", html, count=1)
    index.write_text(html, encoding="utf-8")
    print(f"build.py: {dist} runs {threaded['name']} (threads) on an isolated page, else {plain['name']}")


if __name__ == "__main__":
    main()
