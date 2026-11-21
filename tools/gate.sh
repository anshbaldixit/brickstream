#!/bin/bash
# Full correctness gate for one commit, built from a clean checkout:
#   bash tools/gate.sh <commit> [tag]
# Builds the image, runs every 1M seed in normal and FAST_FORCE_INTERN mode against saved
# oracle outputs (out/diff/<dataset>/a), both fixture suites, and the 20M diff against out/ref-s3.
set -u
cd "$(dirname "$0")/.."
REV=${1:?commit}; TAG=${2:-ss:gate}
PY=.venv/Scripts/python.exe; CMP=kit/starter-duckdb/starter-duckdb/scripts/compare.py
RF=kit/starter-duckdb/starter-duckdb/scripts/run_fixtures.py
rm -rf out/archive && mkdir -p out/archive && git archive "$REV" | tar -x -C out/archive
docker build -q -t "$TAG" out/archive >/dev/null || { echo "BUILD FAILED"; exit 1; }
ok=0; n=0
for d in data/dev data/s*-1m data/stress-*; do
  name=$(echo "$d" | tr '/' '_')
  [ -d "out/diff/$name/a" ] || continue   # stress sets without an oracle answer are skipped
  for mode in normal intern; do
    env=(); [ $mode = intern ] && env=(--env FAST_FORCE_INTERN=1)
    out=out/gate/$name-$mode; n=$((n+1))
    $PY tools/run.py "$TAG" "$d" "$out" "${env[@]}" >/dev/null 2>&1 || { echo "$d $mode: RUN FAILED"; continue; }
    if $PY $CMP "out/diff/$name/a" "$out" >/dev/null 2>&1; then ok=$((ok+1)); else echo "$d $mode: DIFFER"; fi
  done
done
echo "seeds: $ok/$n identical"
for f in kit/starter-duckdb/starter-duckdb/fixtures tests/fixtures-extra; do
  echo "fixtures $(basename $f): $($PY $RF $f -- '.venv\Scripts\python.exe' tools/run.py "$TAG" 2>&1 | tail -1)"
done
$PY tools/run.py "$TAG" vol:ss-s3-20m vol:ss-out >/dev/null 2>&1 && $PY tools/fetch.py vol:ss-out out/gate-s3 >/dev/null \
  && echo "20M vs ref: $($PY $CMP out/ref-s3 out/gate-s3 | tail -1)"
