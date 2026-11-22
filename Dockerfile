# Submission image: the scorer builds this file. It mirrors fast2/Dockerfile; keep the two identical.
# Build context: the repository root.
# /pipeline needs AVX-512 (Cascade Lake or newer). On any other CPU it hands over to /fast, a portable
# (x86-64-v3) build of the fast/ pipeline.
FROM rust@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe AS build
WORKDIR /src
COPY fast2/Cargo.toml fast2/Cargo.lock ./
COPY fast2/.cargo .cargo
COPY fast2/src src
RUN cargo build --release --target x86_64-unknown-linux-gnu \
 && cp target/x86_64-unknown-linux-gnu/release/pipeline /pipeline

FROM rust@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe AS portable
WORKDIR /src
COPY fast/Cargo.toml fast/Cargo.lock ./
COPY fast/src src
RUN RUSTFLAGS="-C target-cpu=x86-64-v3 -C target-feature=+crt-static" \
    cargo build --release --locked --target x86_64-unknown-linux-gnu \
 && cp target/x86_64-unknown-linux-gnu/release/fast /fast

FROM scratch
COPY fast2/LICENSE /LICENSE
COPY --from=build /pipeline /pipeline
COPY --from=portable /fast /fast
USER 1000:1000
CMD ["/pipeline"]
