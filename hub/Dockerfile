# The hub as a container: built with the pinned Rust release, run on a slim Debian with nothing but the binary.
# Build from the repository root:  docker build -f hub/Dockerfile -t yo-hub .
FROM rust:1.99.1-slim-bookworm AS build
WORKDIR /yo
COPY . .
RUN cargo build --release --locked -p yo-hub

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
# The hub runs as nobody and owns its data directory (a mounted volume keeps the owner of the host's directory).
RUN mkdir /data && chown 65534:65534 /data
COPY --from=build /yo/target/release/yo-hub /usr/local/bin/yo-hub
# The database lives in /data (a volume); the hub listens inside the container on 8080.
ENV YO_DB=/data/yo-hub.sqlite YO_LISTEN=0.0.0.0:8080
VOLUME /data
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/yo-hub"]
