# rootcx-core — multi-stage, multi-arch build
# Usage: docker build -t rootcx/core .
#        docker run --init -p 9100:9100 -v rootcx-data:/data rootcx/core

ARG PG_VERSION=18.2.0
ARG BUN_VERSION=1.3.10

# ── Stage 1: build ────────────────────────────────────────────────────────────

FROM rust:1.93-bookworm AS builder

WORKDIR /src
COPY . .

RUN apt-get update -q && apt-get install -yq --no-install-recommends libssl-dev && rm -rf /var/lib/apt/lists/*
RUN cargo build --release -p rootcx-core

# ── Stage 2: fetch resources ──────────────────────────────────────────────────

FROM debian:bookworm-slim AS deps

ARG PG_VERSION
ARG BUN_VERSION
ARG TARGETARCH

RUN apt-get update -q && apt-get install -yq --no-install-recommends curl unzip ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /resources

# Map Docker arch to Rust target triple
RUN case "$TARGETARCH" in \
      amd64) TARGET="x86_64-unknown-linux-gnu" ;; \
      arm64) TARGET="aarch64-unknown-linux-gnu" ;; \
      *) echo "unsupported arch: $TARGETARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/theseus-rs/postgresql-binaries/releases/download/${PG_VERSION}/postgresql-${PG_VERSION}-${TARGET}.tar.gz" | tar -xz

RUN case "$TARGETARCH" in \
      amd64) BUN_ARCH="x64" ;; \
      arm64) BUN_ARCH="aarch64" ;; \
    esac && \
    curl -fsSL -o bun.zip "https://github.com/oven-sh/bun/releases/download/bun-v${BUN_VERSION}/bun-linux-${BUN_ARCH}.zip" && \
    unzip -q bun.zip && mv bun-linux-*/bun . && chmod +x bun && rm -rf bun.zip bun-linux-*

# ── Stage 3: runtime ─────────────────────────────────────────────────────────

FROM debian:bookworm-slim

RUN apt-get update -q \
    && apt-get install -yq --no-install-recommends ca-certificates libssl3 curl libxml2 libreadline8 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -m -d /home/rootcx rootcx \
    && mkdir -p /data && chown rootcx:rootcx /data

COPY --from=builder /src/target/release/rootcx-core /usr/local/bin/rootcx-core
COPY --from=deps    /resources /opt/rootcx/resources

ENV ROOTCX_RESOURCES=/opt/rootcx/resources \
    XDG_DATA_HOME=/data \
    ROOTCX_BIND=1 \
    RUST_LOG=info

VOLUME /data
EXPOSE 9100

USER rootcx
HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
    CMD curl -fs http://localhost:9100/health || exit 1
ENTRYPOINT ["rootcx-core"]
