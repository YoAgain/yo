# The hub as a container: built with the pinned Rust release, run on a slim Debian with nothing but the binary.
# Build from the repository root:  docker build -f server/hub/Dockerfile -t yo-hub .
FROM rust:1.100.0-slim-trixie AS build
WORKDIR /yo
COPY . .
RUN cargo build --release --locked -p yo-hub

FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
# The hub runs as nobody and owns its data directory (a mounted volume keeps the owner of the host's directory).
RUN mkdir /data && chown 65534:65534 /data
COPY --from=build /yo/target/release/yo-hub /usr/local/bin/yo-hub
# The database lives in /data (a volume); the hub listens inside the container on 8080. Its signing key (spec 4.2):
# (`yo-hub hub-key new FILE`, a Curve25519 private key) mount a 32-byte file and point YO_HUB_KEY_FILE at it (docker run -e YO_HUB_KEY_FILE=/run/secrets/hub-key …). Its
# server certificate (spec 4.1, `yo-hub trust-root server`): the same way, YO_SERVER_CERT_FILE; its groups' zkgroup
# params (spec 3.4, `yo-hub zk new`): YO_ZK_PARAMS_FILE.
ENV YO_DB=/data/yo-hub.sqlite YO_LISTEN=0.0.0.0:8080
VOLUME /data
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/yo-hub"]
