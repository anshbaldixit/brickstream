#!/usr/bin/env python3
"""Copy a Docker named volume's contents into a host folder (for diffing outputs written to volumes).

Usage: .venv/Scripts/python.exe tools/fetch.py vol:NAME HOST_FOLDER
"""
import shutil
import subprocess
import sys
from pathlib import Path


def main():
    if len(sys.argv) != 3 or not sys.argv[1].startswith("vol:"):
        sys.exit(__doc__)
    volume, dest = sys.argv[1][4:], Path(sys.argv[2]).resolve()
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    subprocess.run(["docker", "run", "--rm",
                    "--mount", f"type=volume,src={volume},dst=/src,readonly",
                    "--mount", f"type=bind,src={dest},dst=/dst",
                    "busybox:1.36", "cp", "-r", "/src/.", "/dst/"], check=True)
    print(f"copied vol:{volume} -> {dest}")


if __name__ == "__main__":
    main()
