# Shopping Sessions pipeline (Rust). Build context: the repository root.
#   docker build -f fast/Dockerfile -t ss:fast .
FROM rust:1.99-slim-bookworm AS build
WORKDIR /build
COPY fast/Cargo.toml fast/Cargo.lock ./
COPY fast/.cargo ./.cargo
# Dependencies first, so a source change doesn't rebuild them.
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release --locked && rm -rf src
COPY fast/src ./src
RUN touch src/main.rs && cargo build --release --locked && cp target/release/fast /usr/local/bin/fast

FROM debian:bookworm-slim
COPY --from=build /usr/local/bin/fast /usr/local/bin/fast
CMD ["/usr/local/bin/fast"]
