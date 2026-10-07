FROM rust:1-bookworm@sha256:77fac8b98f9f46062bb680b6d25d5bcaabfc400143952ebc572e924bcbedc3fa AS builder

WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY core ./core
COPY crates ./crates
COPY runtime/client ./runtime/client

RUN cargo build --locked --release --package rootcx-core

FROM oven/bun:1.3.10@sha256:b86c67b531d87b4db11470d9b2bd0c519b1976eee6fcd71634e73abfa6230d2e AS bun

FROM node:22-bookworm-slim@sha256:48e4b67d85f87bd551df43704e24d252f56cc5f8e9718841aace50f19948f0f9 AS worker-dependencies
WORKDIR /opt/rootcx-worker-sandbox
COPY services/worker-sandbox/package.json services/worker-sandbox/package-lock.json ./
RUN npm ci --ignore-scripts --omit=dev

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git bubblewrap socat ripgrep \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/rootcx-core /usr/local/bin/rootcx-core
COPY --from=bun /usr/local/bin/bun /opt/rootcx/resources/bun
COPY core/resources/integrations /opt/rootcx/resources/integrations
COPY --from=worker-dependencies /opt/rootcx-worker-sandbox/node_modules /opt/rootcx-worker-sandbox/node_modules
COPY services/worker-sandbox /opt/rootcx-worker-sandbox
COPY docker/core-entrypoint.sh /usr/local/bin/core-entrypoint

RUN chmod 0755 /usr/local/bin/rootcx-core /usr/local/bin/core-entrypoint /opt/rootcx/resources/bun \
    && mkdir -p /data \
    && chgrp -R 0 /data \
    && chmod -R g=u /data

ENV BUN_PATH=/opt/rootcx/resources/bun \
    ROOTCX_RESOURCES=/opt/rootcx/resources \
    HOME=/data \
    XDG_DATA_HOME=/data

USER 1000
EXPOSE 9100

ENTRYPOINT ["/usr/local/bin/core-entrypoint"]
CMD ["rootcx-core", "--daemon"]
