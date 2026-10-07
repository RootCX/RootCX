# Hosted application workers

`ROOTCX_WORKER_SANDBOX=srt` opts a Core instance into OS isolation for Bun workers and dependency installation. The flag is only supported on Linux; an invalid flag or missing sandbox dependency fails closed. Existing Core deployments without the flag keep their current execution path.

Core still governs identities, database queries, application permissions and jobs through the existing standard-input/output protocol. Each worker gets a separate SRT 0.0.78 supervisor, user/PID/network/mount namespaces, its application code read-only, a private temporary home and no inherited Core environment. Installation alone may write its staging application directory, with package lifecycle scripts disabled. The supervisor executes only image-owned code and never imports an application module.

Outbound connections go through SRT's authenticated HTTP/SOCKS proxy. Only DNS names on ports 443, 993 and 587 are accepted. The proxy resolves once, rejects private, loopback, link-local and metadata addresses, then connects to the vetted address. These port rules do not themselves enforce TLS; HTTPS and the mail integration enforce certificate validation. Direct network sockets cannot reach outside the namespace. The mail integration uses SOCKS remote DNS in this mode.

The Core HTTP API is not exposed inside the sandbox. A separate Unix socket serves only the existing single-use upload/download nonce routes. The prelude transparently sends those existing storage URLs over that socket, including assistant attachments. Other host socket paths and abstract Unix sockets are inaccessible. Application secrets explicitly supplied through Core's existing secret mechanism retain their existing scope; they are different from inherited Core process secrets.

The pod requires an unprivileged user namespace, the qualified local seccomp profile and an unmasked proc mount so bubblewrap can mount a fresh isolated `/proc`. Run as UID/GID 1000, drop all capabilities, disable service-account-token automount, keep the root filesystem read-only, mount the tenant data volume and a bounded writable `/tmp`. SHAPP schedules this profile on its existing reserved builder node pool; this requires no shared CNI change. SHAPP uses a minimum of 1 GiB per Core pod. The opt-in worker budget is 256 MiB plus a 160 MiB Core reserve, yielding three workers at 1 GiB. Linux qualification measured about 141 MiB per minimal worker and 210 MiB per idle assistant including its supervisor; peak workload still requires qualification. Legacy deployments retain their 70 MiB worker budget.

Custom MCP connections are explicitly unavailable in this SHAPP opening: their current stdio, CLI and HTTP-via-bunx transports execute parent processes, so the opt-in Core refuses them before any command starts. The 21 business applications, built-in assistant, Builder, channels and native integration catalogue keep their existing paths. This is a known availability limit, not a claim that arbitrary MCP servers have been qualified.

Qualification in the built image:

```sh
ROOTCX_TEST_PUBLIC_NETWORK=1 /opt/rootcx/resources/bun test /opt/rootcx-worker-sandbox/test/
```

The public-network flag enables the real HTTPS and npm-install checks. Offline runs intentionally skip those two checks. The Core integration test `worker_storage_roundtrip_preserves_nonce_authority`, run with the opt-in flag against a disposable PostgreSQL fixture, exercises actual IPC, collection writes and the nonce storage handlers.
