# Dev image: the Rust toolchain plus Python with polars, pyarrow and duckdb, so the
# organizers' fixture runner and compare.py run inside Linux next to the binary.
# Built and started by fast/dev.py (build context: fast/). Not the submission image.
FROM rust:1.99-slim-bookworm
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3 python3-venv \
 && rm -rf /var/lib/apt/lists/* \
 && python3 -m venv /venv \
 && /venv/bin/pip install --no-cache-dir polars==2.1.0 pyarrow==26.0.1 duckdb==1.5.6
ENV PATH=/venv/bin:$PATH \
    CARGO_TARGET_DIR=/target \
    CARGO_TERM_COLOR=never
WORKDIR /work/fast
