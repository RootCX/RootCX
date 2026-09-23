FROM oven/bun:1.3.10@sha256:b86c67b531d87b4db11470d9b2bd0c519b1976eee6fcd71634e73abfa6230d2e AS bun
FROM node:22-bookworm-slim@sha256:48e4b67d85f87bd551df43704e24d252f56cc5f8e9718841aace50f19948f0f9
RUN apt-get update && apt-get install -y --no-install-recommends bubblewrap ca-certificates tar git curl ripgrep \
    && rm -rf /var/lib/apt/lists/*
COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun
RUN npm install --global opencode-ai@1.18.32 && npm cache clean --force
COPY --from=rootcx-skills skills/rootcx /opt/rootcx-skills/rootcx
COPY --from=rootcx-skills scripts/install.sh /tmp/install-rootcx.sh
RUN ROOTCX_INSTALL=/usr/local/lib/rootcx-cli sh /tmp/install-rootcx.sh --no-path-update 0.17.2 \
    && ln -s /usr/local/lib/rootcx-cli/bin/rootcx /usr/local/bin/rootcx && rm /tmp/install-rootcx.sh
WORKDIR /opt/rootcx-builder
COPY services/app-builder/ ./
RUN mkdir -p /var/lib/rootcx-builder /var/lib/rootcx-backups \
    && chown node:node /var/lib/rootcx-builder /var/lib/rootcx-backups
USER node
EXPOSE 9201
CMD ["node", "server.mjs"]
