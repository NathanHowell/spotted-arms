# syntax=docker/dockerfile:1

FROM rust:1.95-slim-trixie AS builder

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean && \
    apt-get update && \
    apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        ca-certificates \
        build-essential \
        git

WORKDIR /app

COPY Cargo.toml Cargo.lock build.rs ./
COPY .cargo ./.cargo
COPY src ./src
COPY .git ./.git

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --bin spotted-arms && \
    cp /app/target/release/spotted-arms /usr/local/bin/spotted-arms

FROM gcr.io/distroless/cc-debian13:latest

COPY --from=builder /usr/local/bin/spotted-arms /usr/local/bin/spotted-arms

USER 65534:65534

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/spotted-arms"]
