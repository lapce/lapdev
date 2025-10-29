#
# Multi-stage build producing runtime images for Lapdev's Kubernetes services.
# Build all binaries once in the shared "builder" stage, then select the desired
# runtime stage with `--target`.
#

FROM docker.io/library/rust:1.90-bullseye AS builder

WORKDIR /app

# Pre-copy manifest files to leverage Docker layer caching.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src ./src
COPY pkg ./pkg

# Install additional build dependencies and targets required for cross-compiling.
RUN apt-get update \
 && apt-get install --yes --no-install-recommends musl-tools pkg-config \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add x86_64-unknown-linux-musl

# Build auxiliary artifacts required at compile time (e.g. guest agent binary).
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked --target x86_64-unknown-linux-musl \
    --package lapdev-guest-agent

# Compile the service binaries we ship to customer clusters.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked \
    --package lapdev \
    --package lapdev-kube-manager \
    --package lapdev-kube-sidecar-proxy \
    --package lapdev-kube-devbox-proxy \
    && mkdir -p /artifacts \
    && install -Dm755 /app/target/release/lapdev /artifacts/lapdev-api \
    && install -Dm755 /app/target/release/lapdev-kube-manager /artifacts/lapdev-kube-manager \
    && install -Dm755 /app/target/release/lapdev-kube-sidecar-proxy /artifacts/lapdev-kube-sidecar-proxy \
    && install -Dm755 /app/target/release/lapdev-kube-devbox-proxy /artifacts/lapdev-kube-devbox-proxy

# Base runtime image shared by all targets.
FROM docker.io/library/debian:12-slim AS runtime-base

ENV RUST_BACKTRACE=1 \
    RUST_LOG=info

RUN apt-get update \
 && apt-get install --yes --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

# -----------------------------------------------------------------------------
# lapdev-kube-manager image
# -----------------------------------------------------------------------------
FROM runtime-base AS kube-manager

WORKDIR /app

COPY --from=builder /artifacts/lapdev-kube-manager /usr/local/bin/lapdev-kube-manager

EXPOSE 5001 7771

ENTRYPOINT ["/usr/local/bin/lapdev-kube-manager"]

# -----------------------------------------------------------------------------
# lapdev-kube-sidecar-proxy image
# -----------------------------------------------------------------------------
FROM runtime-base AS kube-sidecar-proxy

WORKDIR /app

COPY --from=builder /artifacts/lapdev-kube-sidecar-proxy /usr/local/bin/lapdev-kube-sidecar-proxy

EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/lapdev-kube-sidecar-proxy"]

# -----------------------------------------------------------------------------
# lapdev-kube-devbox-proxy image
# -----------------------------------------------------------------------------
FROM runtime-base AS kube-devbox-proxy

WORKDIR /app

COPY --from=builder /artifacts/lapdev-kube-devbox-proxy /usr/local/bin/lapdev-kube-devbox-proxy

ENTRYPOINT ["/usr/local/bin/lapdev-kube-devbox-proxy"]
# -----------------------------------------------------------------------------
# lapdev-api image
# -----------------------------------------------------------------------------
FROM runtime-base AS lapdev-api

WORKDIR /app

COPY --from=builder /artifacts/lapdev-api /usr/local/bin/lapdev

EXPOSE 8080 2222 8443

ENTRYPOINT ["/usr/local/bin/lapdev"]

# -----------------------------------------------------------------------------
