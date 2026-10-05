# ADR 0011: Agent execution authority and confirmations belong to Core

Status: accepted and implemented, 1 October 2026. Deployed to the local SHAPP
tenant fewfwfe as `rootcx-core:shapp-agent-authority-20261001-r1`, preserving
the hosted-builder base `cd2bfce`. No production rollout.

## Context

WhatsApp introduced a live grant check in the generic tool executor, an explicit
Assistant root, and a provider-specific confirmation path. Passing the grant
through worker, tool and child dispatch leaked transport knowledge throughout
Core. The existing channel Adapters already represent real variation; tools
and permission intersections do not vary by transport.

## Decision

The Execution authority Module owns the responsible human, agent lineage, frozen
permission ceiling, task scope, origin and lifetime. Core constructs it before
worker selection and transmits it outside worker IPC. Its fields cannot be
supplied through model arguments, request bodies or worker messages.

At tool admission, including after a confirmation wait, Core revalidates the
human, every agent and invocation permission in the lineage, then intersects
current permissions with the run's original ceiling. Child admission revalidates
and narrows its parent authority and preserves task scope. New grants cannot
expand an existing run. The existing worker limit of one sub-agent level remains;
the authority Module does not assume a fixed root App or lineage length.

Execution origins supply a small live-validation Seam. ChannelSession is its
channel Adapter: it validates channel activation, the linked identity and the
particular delegation recorded at admission. Relinking creates a new delegation
and cannot revive a previously admitted run. The ChannelProvider Adapter may
add provider-side session validation (the SHAPP gateway for WhatsApp). Core owns
permission decisions; provider responses cannot grant tool or App permissions.
Selecting another root agent with `/agent` does not grant it a channel delegation.
A new link to the selected agent is required when no matching channel delegation
exists; an unrelated automation's delegation cannot stand in for this link.

Worker identity includes the origin and lineage as well as its delegated
permissions. Distinct origins cannot share a process and borrow one another's
live invocation IDs. This retains bounded, cached workers rather than creating
a process per message. Dropping the active run cancels pending authority and
its descendants, including tool requests waiting for confirmation.

The pending-confirmation Module binds an exact request to its responsible human,
origin and originating App. It owns expiration, single consumption, cancellation
and ownership checks for both listing and replying. Web and channel routes use
the same Interface. Descendant confirmations are reviewed through the originating
App. Delivery failure cancels the request; an expired reply is rejected even if
the waiting worker's timer has not yet run. Agent confirmation never confers
additional permissions and does not substitute for Approved action review.

Slack and Telegram peer identities bind both authenticated sender and conversation.
A conversation ID alone cannot confer another participant's linked authority.
Their Adapters encode and decode opaque peer keys; legacy room-only associations
require relinking, because the missing sender cannot safely be inferred. WhatsApp
already uses a verified per-user Company grant and retains its link format.

## Consequences

Locality: live agent authority is checked in one Module; confirmation ownership
is checked in another. Generic workers and tools contain no WhatsApp condition.
Leverage: Web and all channel Adapters exercise the same authority and supervision
rules, without a second RBAC implementation or a generic plugin framework.

ADR 0001's common tool dispatch, ADR 0004's collection grants, ADR 0005's governed
jobs and ADR 0008's approved backend authority remain in force. No role, collection
grant, RLS policy, approval database role or business schema is widened.

Live checks govern admission of the next tool operation. They do not roll back
external effects already admitted, and they are not a new distributed transaction
barrier between SHAPP and a tenant. Existing database-specific revocation locks
and worker SQL enforcement retain their own guarantees. This change does not
claim host-level isolation against hostile processes sharing an OS account.

Validation crosses the authority and confirmation Interfaces and the real agent
HTTP/worker/PostgreSQL path. Provider network delivery is simulated, not certified.
