FROM rust:1.90-bookworm AS builder
WORKDIR /app
ENV CARGO_TARGET_DIR=/tmp/mallow-target
COPY rust-bot/Cargo.toml rust-bot/Cargo.toml
COPY rust-bot/src rust-bot/src
COPY TERMS_OF_SERVICE.md TERMS_OF_SERVICE.md
COPY halloween_facts.txt halloween_facts.txt
COPY description.txt description.txt
COPY status*.txt ./
COPY update.txt update.txt
RUN cargo build --manifest-path rust-bot/Cargo.toml --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates fonts-dejavu-core && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /tmp/mallow-target/release/mallow-bot /usr/local/bin/mallow-bot
COPY --from=builder /app/status*.txt /app/
COPY --from=builder /app/update.txt /app/update.txt
ENV RUST_LOG=info
EXPOSE 25572
CMD ["mallow-bot"]
