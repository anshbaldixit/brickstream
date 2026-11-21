# Builds the organizers' dataset generator. Build context: kit/generator/generator
#   docker build -t ss-gen -f tools/generator.Dockerfile kit/generator/generator
FROM rust:1.85-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=build /src/target/release/generate /usr/local/bin/generate
ENTRYPOINT ["generate"]
