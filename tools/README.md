# tools/

Every command runs from the repo root with the venv's Python. They call `docker` directly, so Git Bash path mangling doesn't apply.

## Datasets
```
.venv/Scripts/python.exe tools/gen.py --seed 1 --events 1000000 --clicks 250000 data/dev     # the public dev set
.venv/Scripts/python.exe tools/gen.py --seed 2 --events 1000000 data/s2-1m                   # extra seeds for differential tests
.venv/Scripts/python.exe tools/gen.py --seed 3 --events 20000000 vol:ss-s3-20m               # practice-sized, for timing
.venv/Scripts/python.exe tools/gen.py --seed 4 --events 60000000 vol:ss-s4-60m               # final-sized: memory and time checks
```
Use host folders (`data/...`) for small sets you diff natively, and named volumes (`vol:...`) for the big timing sets. Windows bind mounts are slow and would distort timings.

## Run an image the scorer's way
```
docker build -f ref/Dockerfile -t ss:ref .
.venv/Scripts/python.exe tools/run.py ss:ref data/dev out/ref-dev                       # correctness run
.venv/Scripts/python.exe kit/starter-duckdb/starter-duckdb/scripts/compare.py out/ref-dev kit/dev-answer/dev-answer
.venv/Scripts/python.exe tools/run.py ss:ref vol:ss-s3-20m vol:ss-out --repeat 5        # timing run (median of 5)
.venv/Scripts/python.exe tools/fetch.py vol:ss-out out/ref-s3                          # copy a volume's output out to diff it
```
`run.py` applies the scorer's flags: read-only root, `/tmp` tmpfs of 64 MB, uid 1000, no network, `--cpus 8 --memory 24g` with no swap, and `/scratch` on a fresh volume. It times the container from start to exit using Docker's own timestamps.

## Fixtures against a Docker image
```
.venv/Scripts/python.exe kit/starter-duckdb/starter-duckdb/scripts/run_fixtures.py kit/starter-duckdb/starter-duckdb/fixtures -- .venv/Scripts/python.exe tools/run.py ss:ref
```

## Local machine vs scorer
This laptop has a Ryzen 7 6800H (8 cores, 16 threads, AVX2, **no AVX-512**) and 31 GB RAM. The scorer has 8 hyperthreads of a Xeon (about 4 cores, AVX-512). Absolute times won't match, so compare builds against each other on the same dataset. Docker Desktop's VM gets about half the RAM unless `%USERPROFILE%\.wslconfig` raises it, and the 60M test needs about 24 GB.
