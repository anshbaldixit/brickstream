#!/usr/bin/env python3
"""Differential test: run two images on the same datasets and require identical output.

Usage (from the repo root):
  .venv/Scripts/python.exe tools/diffseeds.py IMAGE_A IMAGE_B [DATASET ...]

DATASET defaults to every data/s*-1m folder plus data/dev. Each can be a host folder or vol:NAME.
Outputs go to out/diff/<dataset>/{a,b}; the organizers' compare.py decides MATCH/DIFFER.
Exit code 0 only if every dataset matches.
"""
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY = sys.executable
COMPARE = ROOT / "kit" / "starter-duckdb" / "starter-duckdb" / "scripts" / "compare.py"


def run(image, data, out):
    """IMAGE is a Docker image, or py:path/to/script.py for a native implementation taking DATA OUT."""
    if image.startswith("py:"):
        if data.startswith("vol:"):
            return 2, ["py: implementations run natively and can't read Docker volumes; use a host folder"]
        out_path = Path(out)
        if out_path.exists():
            shutil.rmtree(out_path)
        out_path.mkdir(parents=True)
        command = [PY, str(ROOT / image[3:]), data, out]
    else:
        command = [PY, str(ROOT / "tools" / "run.py"), image, data, out]
    p = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", errors="replace")
    return p.returncode, (p.stdout + p.stderr).strip().splitlines()[-1:] or [""]


def fetch_if_volume(spec, host):
    if spec.startswith("vol:"):
        subprocess.run([PY, str(ROOT / "tools" / "fetch.py"), spec, host], check=True, capture_output=True)
        return host
    return spec


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    image_a, image_b = sys.argv[1], sys.argv[2]
    datasets = sys.argv[3:] or ["data/dev"] + sorted(str(p.relative_to(ROOT)) for p in (ROOT / "data").glob("s*-1m"))
    failures = []
    for data in datasets:
        name = data.replace("vol:", "").replace("/", "_").replace("\\", "_")
        outs = {}
        for side, image in (("a", image_a), ("b", image_b)):
            out = f"out/diff/{name}/{side}"
            spec = f"vol:ss-diff-{side}" if data.startswith("vol:") else out
            code, tail = run(image, data, spec)
            if code != 0:
                print(f"{data}: {image} FAILED (exit {code}) {tail[0]}")
                failures.append(data)
                break
            outs[side] = fetch_if_volume(spec, out)
        else:
            p = subprocess.run([PY, str(COMPARE), outs["a"], outs["b"]], capture_output=True, text=True, encoding="utf-8")
            verdict = "MATCH" if p.returncode == 0 else "DIFFER"
            print(f"{data}: {verdict}")
            if verdict == "DIFFER":
                failures.append(data)
                print("   " + p.stdout.strip().replace("\n", "\n   "))
    print(f"\n{len(datasets) - len(failures)}/{len(datasets)} datasets identical")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
