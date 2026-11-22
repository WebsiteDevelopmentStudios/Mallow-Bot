FROM rust:1.86-bookworm AS builder
WORKDIR /app
ENV CARGO_TARGET_DIR=/tmp/mallow-target
COPY rust-bot/Cargo.toml rust-bot/Cargo.toml
COPY rust-bot/src rust-bot/src
COPY TERMS_OF_SERVICE.md TERMS_OF_SERVICE.md
RUN cargo build --manifest-path rust-bot/Cargo.toml --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates fonts-dejavu-core fonts-dejavu-core && rm -rf /var/lib/apt/lists/*
COPY --from=builder /tmp/mallow-target/release/mallow-bot /usr/local/bin/mallow-bot
ENV RUST_LOG=info
EXPOSE 25572
CMD ["mallow-bot"]
