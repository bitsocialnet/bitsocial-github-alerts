FROM rust:1.87 AS builder
COPY . .
RUN cargo build --release

FROM ubuntu:22.04
EXPOSE 8080

RUN apt update && \
    apt install build-essential pkg-config libssl-dev libpq-dev ca-certificates -y && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder ./target/release/bitsocial-github-alerts ./target/release/bitsocial-github-alerts
RUN chmod +x ./target/release/bitsocial-github-alerts
CMD ["/target/release/bitsocial-github-alerts"]
