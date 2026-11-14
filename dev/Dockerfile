# The toolchain dev/cargo runs in: the pinned Rust release with clippy and rustfmt.
FROM rust:1.99.1-slim-bookworm
RUN rustup component add clippy rustfmt && apt-get update -qq && apt-get install -y -qq protobuf-compiler && rm -rf /var/lib/apt/lists/*
