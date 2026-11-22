# Shopping Sessions pipeline (Rust). Build context: the repository root.
#   docker build -f fast2/Dockerfile -t ss:fast2 .
# The build uses a profile (fast2/pgo/pipeline.profdata) recorded from runs on generated data, so the
# compiler lays out and inlines the hot paths for the real workload. The flags match .cargo/config.toml.
FROM rust@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe AS build
WORKDIR /src
COPY fast2/Cargo.toml fast2/Cargo.lock ./
COPY fast2/.cargo .cargo
COPY fast2/src src
COPY fast2/pgo pgo
RUN RUSTFLAGS="-C target-cpu=cascadelake -C relocation-model=static -C target-feature=+crt-static -C llvm-args=-x86-branches-within-32B-boundaries -C llvm-args=-x86-pad-max-prefix-size=5 -C profile-use=/src/pgo/pipeline.profdata" \
    cargo build --release --target x86_64-unknown-linux-gnu \
 && cp target/x86_64-unknown-linux-gnu/release/pipeline /pipeline

FROM scratch
COPY fast2/LICENSE /LICENSE
COPY --from=build /pipeline /pipeline
USER 1000:1000
CMD ["/pipeline"]
