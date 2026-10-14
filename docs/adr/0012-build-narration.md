# ADR 0012: Build narration is an optional observer

Status: implemented locally, 14 October 2026; not deployed.

## Context

Progress used a separate model call but reused the expensive coding endpoint.
Core replaced contextual messages with phase boilerplate, and the companion
rendered every historical sentence. Presentation therefore leaked across three
Modules and remained repetitive.

## Decision

Build narration consumes reduced facts through a small Interface. Its
Implementation owns timing, budget, duplicate suppression and cancellation.
An asynchronous translator Seam separates these rules from the hosted Nova
Micro Adapter. Tests exercise the same Interface with controlled Adapters.
The managed route owns model selection, instructions and credit accounting.
Coding credentials cannot select an expensive model through the narration route.
No narration failure changes execution authority or blocks the builder.
Core alone confirms publication, and missing narration preserves the last text.
The companion displays one current response per request.

## Consequences

Locality concentrates presentation policy in the narration Module; Leverage
comes from keeping all callers free of provider and scheduling rules. Removing
it would spread those rules back across the engine, Core and the companion.
Observations deliberately carry categories rather than raw source or logs;
updates can describe the requested work and actual stage, not assert unverified
feature completion. Existing execution-authority ADRs remain unchanged.
