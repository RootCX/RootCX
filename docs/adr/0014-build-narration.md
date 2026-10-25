# ADR 0014: Build narration is an optional observer

Status: implemented 14 October 2026; deployed with the hosted Builder on 20 October 2026.

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

The observer now distinguishes bounded action categories (rules, edits, tests,
version preparation) and deduplicates tool events by private identity and status
in a bounded recent set. Returning to the same phase remains observable. Tool
identity never reaches Nova or the user. Calls are separated by four seconds
and capped at forty per execution; no timed update is invented without evidence.

## Conversation context and evidence, 20 October 2026

No intermediary model prepares narration. The server keeps the last three business
exchanges in a bounded four-turn journal outside both application sources and the
coding sandbox. It keys the journal by the real OpenCode session and upserts Core's
run ID, so repair attempts never become new user turns. Only the original narration
prompt and filtered prepared reply enter this journal, never SDK prompts or repair
diagnostics. Prepared replies explicitly have unknown publication status. A missing
or corrupt journal disables context only; it cannot stop coding.

An outward business statement followed by a native tool carries untrusted intent
separately from action/status facts. Final text with no following tool is not progress;
private reasoning is never consumed. This uses the existing OpenCode text events,
not a custom reporting tool or another model.
The build pipeline reports its own check start, success and failure without output.
Nova may describe a successful verification only with a completed platform check;
a later failure or edit invalidates earlier evidence. Every new observation invalidates
a pending translation, including same-phase outcomes. Batched facts preserve actual
arrival order. A running operation or busy engine may trigger a factual reminder after
20 seconds without activity; neither an idle engine nor a completed operation may do so.
The route accepts this optional context during rolling upgrades and requires structured
output for enriched requests. Publication remains exclusively Core's responsibility.
