# ADR 0005: Core-owned job provenance and delegated authority

Status: Accepted

## Context

A delegated action must not regain permissions by enqueueing work that resumes
as an unrestricted user. Application payloads must not select privileged native
workflow, hook or cron dispatchers.

## Decision

Core owns the job envelope's kind and authority. Worker continuations retain
their frozen delegated ceiling, connection and audit attribution. Delivery
checks current authority without widening that ceiling.

Manual workflow enqueue atomically binds the message to its execution,
application, responsible user and queue lease. Dispatch and execution validate
that binding before changing workflow state.

Raw agent enqueue is refused because its envelope cannot bind invocation task
scope. Supervised dispatch through an ordinary application action retains the
delegated ceiling.

## Consequences

- Payload data cannot grant native execution authority.
- Revocation can prevent queued delegated work from running.
- Legacy envelopes are quarantined: authority cannot safely be reconstructed
  from their payloads. Operators must review and resubmit through authorized APIs.
- Dispatchers sharing a queue must be upgraded together; older versions do not
  enforce the new authority contract.

Operational steps: [Queue upgrade and delegated continuations](../cross-app-collections.md#queue-upgrade-and-delegated-continuations).
Contributor checks: [Core testing](../testing.md).
