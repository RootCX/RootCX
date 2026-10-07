#!/usr/bin/env bash
set -euo pipefail
image=${1:?Usage: verify-image.sh BUILDER_IMAGE}
# No source mount: exercise exactly the files shipped in the candidate image.
docker run --rm --network none \
  --security-opt seccomp=unconfined --security-opt systempaths=unconfined \
  --entrypoint node "$image" --test \
  test/engine.test.mjs test/provider-proxy.test.mjs \
  test/opencode.integration.test.mjs test/sdk.test.mjs test/sandbox.test.mjs \
  test/network-sandbox.test.mjs test/backup.test.mjs test/startup.test.mjs test/tls.test.mjs
# Qualify dependency installation separately; only the public registry is allowed
# from the sandbox even though this disposable container has external networking.
docker run --rm \
  --security-opt seccomp=unconfined --security-opt systempaths=unconfined \
  -e ROOTCX_SANDBOX_NETWORK_TESTS=true --entrypoint node "$image" \
  --test --test-name-pattern='npm and Bun download' test/network-sandbox.test.mjs
