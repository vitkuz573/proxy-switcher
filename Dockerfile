FROM rust:1.80-slim-bookworm AS builder

WORKDIR /app
COPY . .

RUN apt-get update && apt-get install -y pkg-config libsqlite3-dev && \
    cargo build --release --workspace

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y iproute2 ca-certificates libsqlite3-0 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/proxy-daemon /usr/local/bin/proxy-daemon
COPY --from=builder /app/target/release/proxy-cli /usr/local/bin/proxy-cli
COPY config/proxy-switcher.toml /etc/proxy-switcher/config.toml
COPY config/proxy-switcher.service /etc/systemd/system/proxy-switcher.service

EXPOSE 8080

ENTRYPOINT ["proxy-daemon"]
CMD ["--config", "/etc/proxy-switcher/config.toml"]
