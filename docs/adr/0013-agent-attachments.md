# ADR 0013: Agent attachments belong to Core

Status: implemented and verified locally, 20 October 2026. No production rollout.

## Context

Web invocation and communication channels separately constructed worker download
capabilities. Web accepted app-scoped file IDs with invocation permission alone,
although ordinary storage downloads require `storage.read`. Channel images had
provider-labelled validation and represented inline bytes as a remote file ID.
A standalone photo laboratory proved model vision but bypassed Core authority,
storage and the actual channel intake path.

## Decision

The Agent attachment Module (`extensions/agents/attachments.rs`) owns image
validation, app-scoped stored-file resolution, received-file storage and one-use
worker download capabilities. Its Interface has two real entries: stored files
for Web, and received bytes for communication channels. Both produce the existing
`FileAttachment` worker contract. There is no Shappy-specific model path.

Web callers need `app:{app}:invoke` and, when attaching stored files,
`app:{app}:storage.read`. File IDs cannot cross Apps. Channel intake preserves
its verified Channel association, Channel grant and Execution origin under ADR
0011. Received files are attributed to the responsible human. This does not
require channel users to have general storage access merely to send a photo.
No tool authority or delegation is widened.

The ChannelProvider Seam remains responsible for provider retrieval. MediaSource
explicitly distinguishes remote provider identifiers from authenticated inline
bytes. The managed relay carries `attachments: [{name, content_type, data}]` with
base64 data and one JPEG/PNG photo at most; it never fetches a caller URL. Legacy
prototype `images` payloads are rejected. Native providers and Web use at most ten
attachments. Every image is nonempty, at most 4 MiB and has a matching JPEG/PNG
signature. These checks validate the envelope, not complete image decoding.
Native HTTP downloads are bounded while streaming; other file types retain the
existing storage size limit and document handling. Failed download or validation
prevents model invocation rather than silently dropping a photo.

Locality: validation and capability creation have one Implementation. Leverage:
Web, managed WhatsApp, managed Telegram and native provider Adapters all reuse
that Implementation. Deleting the Module would redistribute those rules among
callers; it is not a pass-through abstraction.

## Verification

Unit tests cover invalid formats, lengths, count and ambiguous relay envelopes.
`core/tests/agent_attachments_integration.rs` is an opt-in local recipe: it bundles
the actual installed Assistant, then exercises real Core HTTP, PostgreSQL,
authentication, channel links, storage and worker execution. A loopback Anthropic
endpoint checks exact image bytes and captions from Web upload/invoke and both
managed providers. Provider delivery is simulated; external Meta/Telegram
configuration is untouched. Negative cases cover malformed images, missing
storage permission, excessive file IDs and cross-App file IDs.

```sh
TEST_DATABASE_URL=postgres://rootcx:rootcx@127.0.0.1:57862/rootcx_test \
ROOTCX_TEST_ASSISTANT_DIR=/path/to/ai_agent_base/backend \
cargo test -p rootcx-core --test agent_attachments_integration -- --ignored --nocapture
```

The database must be disposable and local; the harness recreates the entire `rootcx_test` database.
The Assistant directory must already contain dependencies for local `bun build`.
This recipe proves transmission to the model protocol; real-model visual
understanding and real provider delivery require separate validation.
