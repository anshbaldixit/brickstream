# Submission image: the scorer builds this file. It mirrors fast2/Dockerfile; keep the two identical.
# Build context: the repository root.
FROM rust:1.99-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY fast2/Cargo.toml fast2/Cargo.lock ./
COPY fast2/.cargo .cargo
COPY fast2/src src
RUN cargo build --release --target x86_64-unknown-linux-musl \
 && cp target/x86_64-unknown-linux-musl/release/pipeline /pipeline

FROM scratch
COPY fast2/LICENSE /LICENSE
COPY --from=build /pipeline /pipeline
USER 1000:1000
CMD ["/pipeline"]
