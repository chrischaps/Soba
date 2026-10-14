"""Turns a filmed shot (target/showcase/<shot>/video.mkv + audio.wav) into a
web clip with sound, plus a poster frame.

    python tools/showcase/encode.py <shot> <out.mp4> [--from S] [--to S]
        [--poster S] [--loop S] [--fps N] [--crf N] [--lufs L]
        [--crop WxH+X+Y] [--fade-in S] [--fade-out S]

--loop S     crossfades the last S seconds under the first S (picture and
             sound), so the clip loops without a seam; the result is S shorter.
--poster S   writes <out>-poster.jpg from that time (in the output clip).
--crop       keeps a W x H window of the picture from (X, Y), in pixels: a
             portrait capture loses its toolbar to make a 1080x1920 cut.
--fade-in S, --fade-out S
             fade the picture from and to black, and the sound with it.

An <out.mov> is a master for a video site: the same picture, with the sound
kept as 24-bit PCM rather than AAC.

Audio is brought to a common loudness (two-pass EBU R128, linear, so the
dynamics are untouched) so every clip on the page plays at the same level.
"""

import argparse
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SHOWCASE = ROOT / "target" / "showcase"


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", *args], check=True)


def measure_loudness(src, af_prefix):
    """First loudnorm pass: returns the measured values."""
    out = subprocess.run(
        ["ffmpeg", "-hide_banner", "-i", str(src), "-af", f"{af_prefix}loudnorm=print_format=json", "-f", "null", "-"],
        capture_output=True, text=True, check=True).stderr
    return json.loads(re.findall(r"\{[^{}]*\}", out)[-1])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("shot")
    ap.add_argument("out")
    ap.add_argument("--from", dest="start", type=float, default=0.0)
    ap.add_argument("--to", type=float)
    ap.add_argument("--poster", type=float)
    ap.add_argument("--loop", type=float)
    ap.add_argument("--fps", type=int, default=60)
    ap.add_argument("--crf", type=int, default=22)
    ap.add_argument("--lufs", type=float, default=-18.0)
    ap.add_argument("--crop")
    ap.add_argument("--fade-in", type=float)
    ap.add_argument("--fade-out", type=float)
    a = ap.parse_args()

    src = SHOWCASE / a.shot
    out = Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    trim = ["-ss", str(a.start)] + (["-to", str(a.to)] if a.to else [])

    if a.loop:
        # Tail crossfaded under head: out = head..(end-S) with the first S
        # seconds blended from tail into head
        length = (a.to or float(subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(src / "audio.wav")],
            capture_output=True, text=True, check=True).stdout)) - a.start
        L, S = length, a.loop
        # The tail fades out over the head's first S seconds
        vf = (f"[0:v]trim={a.start}:{a.start + L},setpts=PTS-STARTPTS,split[v1][v2];"
              f"[v2]trim={L - S}:{L},setpts=PTS-STARTPTS[tail];"
              f"[v1]trim=0:{L - S},setpts=PTS-STARTPTS[body];"
              f"[tail][body]xfade=transition=fade:duration={S}:offset=0[vx];"
              f"[vx]trim=0:{L - S},setpts=PTS-STARTPTS[v]")
        af = (f"[1:a]atrim={a.start}:{a.start + L},asetpts=PTS-STARTPTS,asplit[a1][a2];"
              f"[a2]atrim={L - S}:{L},asetpts=PTS-STARTPTS,afade=t=out:d={S}:curve=qsin[atail];"
              f"[a1]atrim=0:{L - S},asetpts=PTS-STARTPTS,afade=t=in:d={S}:curve=qsin[abody];"
              f"[abody][atail]amix=inputs=2:duration=first:normalize=0[araw]")
        mid = out.with_suffix(".loop.mkv")
        ffmpeg("-i", str(src / "video.mkv"), "-i", str(src / "audio.wav"),
               "-filter_complex", vf + ";" + af, "-map", "[v]", "-map", "[araw]",
               "-c:v", "libx264rgb", "-crf", "0", "-preset", "ultrafast", "-c:a", "pcm_f32le", str(mid))
        vin, ain, trim = [str(mid)], [str(mid)], []
    else:
        mid = None
        vin, ain = [str(src / "video.mkv")], [str(src / "audio.wav")]

    # Loudness, measured on exactly the audio that will play
    m = measure_loudness(ain[0], f"atrim=start={a.start}{(':end=' + str(a.to)) if a.to else ''}," if trim else "")
    norm = (f"loudnorm=I={a.lufs}:TP=-1.5:LRA=20:linear=true:"
            f"measured_I={m['input_i']}:measured_TP={m['input_tp']}:"
            f"measured_LRA={m['input_lra']}:measured_thresh={m['input_thresh']}:offset={m['target_offset']}")

    vf, af = [], [norm]
    if a.crop:
        w, h, x, y = re.fullmatch(r"(\d+)x(\d+)\+(\d+)\+(\d+)", a.crop).groups()
        vf.append(f"crop={w}:{h}:{x}:{y}")
    vf.append(f"fps={a.fps}")
    if a.fade_in:
        vf.append(f"fade=t=in:d={a.fade_in}")
        af.append(f"afade=t=in:d={a.fade_in}:curve=qsin")
    if a.fade_out:
        length = (a.to - a.start) if a.to else float(subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", ain[0]],
            capture_output=True, text=True, check=True).stdout) - a.start
        vf.append(f"fade=t=out:st={length - a.fade_out:.3f}:d={a.fade_out}")
        af.append(f"afade=t=out:st={length - a.fade_out:.3f}:d={a.fade_out}:curve=qsin")
    vf.append("format=yuv420p")
    af.append("aresample=48000")
    sound = ["-c:a", "pcm_s24le"] if out.suffix == ".mov" else ["-c:a", "aac", "-b:a", "160k"]
    ffmpeg(*trim, "-i", vin[0], *trim, "-i", ain[0],
           "-map", "0:v", "-map", "1:a",
           "-vf", ",".join(vf),
           "-af", ",".join(af),
           "-c:v", "libx264", "-profile:v", "high", "-preset", "slow", "-crf", str(a.crf),
           *sound, "-movflags", "+faststart", str(out))
    if mid:
        mid.unlink()

    if a.poster is not None:
        ffmpeg("-ss", str(a.poster), "-i", str(out), "-frames:v", "1", "-q:v", "3",
               str(out.with_name(out.stem + "-poster.jpg")))
    size = out.stat().st_size / 1e6
    print(f"{out.name}: {size:.1f} MB, measured {m['input_i']} LUFS -> {a.lufs}")


if __name__ == "__main__":
    main()
