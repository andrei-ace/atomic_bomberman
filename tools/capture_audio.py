#!/usr/bin/env python3
"""Verify original game audio through an isolated ALSA file output (Linux/X11)."""
import argparse
import array
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--exe", type=Path, default=Path("target/debug/bomberman"))
parser.add_argument("--assets", required=True, type=Path)
parser.add_argument("--report", type=Path)
parser.add_argument("--display", default=os.environ.get("DISPLAY", ":99"))
args = parser.parse_args()

with tempfile.TemporaryDirectory(prefix="atomic-audio-check-") as temp:
    root = Path(temp)
    raw = root / "playback.raw"
    config = root / "alsa.conf"
    destination = str(raw).replace("\\", "\\\\").replace('"', '\\"')
    config.write_text('pcm.null { type null }\npcm.!default { type file slave.pcm "null" '
                      f'file "{destination}" format "raw" }}\n')
    env = os.environ.copy()
    env.pop("ATOMIC_BOMBERMAN_SILENT_AUDIO", None)
    env.update(ALSA_CONFIG_PATH=str(config), ATOMIC_BOMBERMAN_AUDIO_PACE="1",
               DISPLAY=args.display, LP_NUM_THREADS="2", LIBGL_ALWAYS_SOFTWARE="1")
    subprocess.run([str(args.exe.resolve()), "--assets", str(args.assets.resolve()),
                    "--settings", str(root / "settings.json"), "--seed", "1997",
                    "--smoke-test", "300"], env=env, check=True, timeout=180)
    samples = array.array("f", raw.read_bytes())
    count = len(samples)
    report = dict(format="ALSA float32 stereo capture", sample_rate=44100, channels=2,
                  samples=count, duration_seconds=count / 88200,
                  finite=all(math.isfinite(x) for x in samples),
                  nonzero_samples=sum(x != 0 for x in samples),
                  rms=math.sqrt(sum(x*x for x in samples) / max(count, 1)),
                  peak=max((abs(x) for x in samples), default=0), bytes=raw.stat().st_size,
                  seed=1997, gui_frames=300,
                  limitations=["Decoded/mixed output captured; physical speaker playback unmeasured."])
    if not report["finite"] or not report["nonzero_samples"] or count % 2:
        raise SystemExit(f"Invalid or silent audio capture: {report}")
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
