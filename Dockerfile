# syntax=docker/dockerfile:1
FROM rust:slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libsqlite3-dev libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates/proxy-core/Cargo.toml crates/proxy-core/
COPY crates/proxy-daemon/Cargo.toml crates/proxy-daemon/
COPY crates/proxy-cli/Cargo.toml crates/proxy-cli/
COPY crates/proxy-test/Cargo.toml crates/proxy-test/
COPY crates/proxy-tun-test/Cargo.toml crates/proxy-tun-test/

RUN mkdir -p crates/proxy-core/src crates/proxy-daemon/src crates/proxy-cli/src \
    crates/proxy-test/src crates/proxy-tun-test/src \
    && echo "fn main() {}" > crates/proxy-core/src/lib.rs \
    && echo "fn main() {}" > crates/proxy-daemon/src/main.rs \
    && echo "fn main() {}" > crates/proxy-cli/src/main.rs \
    && echo "fn main() {}" > crates/proxy-test/src/main.rs \
    && echo "fn main() {}" > crates/proxy-tun-test/src/main.rs \
    && cargo build --release --workspace 2>&1

COPY . .
RUN touch crates/proxy-core/src/lib.rs crates/proxy-daemon/src/main.rs \
    crates/proxy-cli/src/main.rs crates/proxy-test/src/main.rs \
    crates/proxy-tun-test/src/main.rs \
    && cargo build --release --workspace 2>&1

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    iproute2 ca-certificates libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/proxy-daemon /usr/local/bin/proxy-daemon
COPY --from=builder /app/target/release/proxy-cli /usr/local/bin/proxy-cli

EXPOSE 9090

ENTRYPOINT ["proxy-daemon"]
CMD ["--config", "/etc/proxy-switcher/config.toml"]
