# Gmail Integration v0.2 — PRODUCT.md

## Summary

A RootCX integration that exposes Gmail as a stable set of RPC actions consumable by any RootCX app (CRM, helpdesk, prospection, custom). The integration owns OAuth, token lifecycle, RFC-compliant MIME composition, retry on transient errors, and a typed error contract. It owns no business state — consuming apps build their own data model on top.

## Problem

The v0.1 integration only exposes Gmail send / list / get / modify / history with `gmail.send + gmail.readonly` scopes. Consumers cannot reply in-thread, attach files, parse structured headers, or react to typed errors. The integration also burns a Google refresh on every action because access tokens are never persisted between calls, and the `modify_email` action is unusable because the scope is read-only. The webhook endpoint discards `emailAddress` from the Pub/Sub payload, so push notifications cannot be routed to the correct user in a multi-user install.

## Goals

- A single integration that is **complete enough** that a CRM, a helpdesk, and a prospection tool can all be built on it without forking it.
- **Typed, stable error codes** as a public contract.
- **Reply-in-thread** and **attachments** as first-class send features.
- **HTML and text** preserved separately on receive.
- **Push notifications** routable to a specific user in a multi-user install.
- Token refresh handled by the Google SDK, not reimplemented.

## Non-goals

- Open / click tracking pixels — those are business features, belong in consuming apps.
- Domain-wide delegation (Workspace service account) — large surface, deferred.
- Email templating, signatures, mailmerge — belong in consuming apps.
- The integration does not persist message bodies, threads, or sync state of its own. Storage is the app's responsibility.

## Behavior

### Authentication and credentials

1. The integration declares the OAuth scope set `email`, `profile`, `https://www.googleapis.com/auth/gmail.readonly`, `https://www.googleapis.com/auth/gmail.send`, `https://www.googleapis.com/auth/gmail.compose`, `https://www.googleapis.com/auth/profile.emails.read`. The integration does not request `gmail.modify` by default. Consumers that need mark-as-read / archive must opt-in via a config flag `enableModifyScope: true`; when set the integration adds `https://www.googleapis.com/auth/gmail.modify` to the consent request and exposes the `modify_email` action.

2. On `__auth_start`, the integration redirects to Google with `access_type=offline` and `prompt=consent`. On `__auth_callback`, the integration persists only the `refresh_token` in encrypted user-scoped secrets. Access tokens are never persisted.

3. Each RPC call constructs (or reuses from a per-worker LRU keyed by `userId`) a `google.auth.OAuth2` client with `refresh_token` set. The Google SDK transparently fetches and caches the access token. The integration performs no manual `/token` exchanges itself.

4. When Google rejects the refresh token with `invalid_grant`, the integration returns `{ok: false, error: {code: "INSUFFICIENT_PERMISSIONS", message}}`. It does not retry. The consuming app is responsible for marking the user as needing reauth.

5. The integration supports a **managed proxy mode** (config `proxyToken` + `baseUrl`). In managed mode, `__auth_start` redirects to the proxy and `__auth_callback` accepts `code=MANAGED_OK` from the proxy. All token fetches are delegated to `POST {baseUrl}/token` with `Authorization: Bearer {proxyToken}` and `{userId}` in the body. Managed mode behaves identically from the consumer's perspective.

### Error contract

6. Every action returns `{ok: true, data}` or `{ok: false, error}`. Errors are never thrown across the RPC boundary. The error envelope is:

   ```
   error: { code: GmailErrorCode, message: string, retryAfter?: number }
   ```

7. `GmailErrorCode` is one of:
   - `INSUFFICIENT_PERMISSIONS` — auth failure or scope missing (HTTP 401, 403 `domainPolicy` / `insufficientPermissions`, 400 `invalid_grant`, 400 `failedPrecondition` "Mail service not enabled").
   - `TEMPORARY_ERROR` — rate-limited or transient backend (HTTP 429, 503, 500/502/504 with reason `backendError` / `internal_failure`, 403 `rateLimitExceeded` / `userRateLimitExceeded` / `dailyLimitExceeded`, 400 `failedPrecondition` non-mail).
   - `SYNC_CURSOR_ERROR` — history cursor expired (HTTP 404 on `/history`).
   - `NOT_FOUND` — entity does not exist (HTTP 404 on `/messages/{id}` or `/threads/{id}`).
   - `MISCONFIGURED` — integration config is missing or invalid for the action.
   - `UNKNOWN` — any unmapped error.

8. When the error is `TEMPORARY_ERROR` and the Google response contains a parseable `Retry after YYYY-MM-DDTHH:MM:SSZ` clause in the message body, the integration extracts that timestamp as a Unix-millisecond `retryAfter` and includes it in the error. If the parsed time is in the past or unparseable, `retryAfter` is omitted.

9. The integration retries `TEMPORARY_ERROR` internally up to **3 times** with exponential backoff (1s, 2s, 4s) plus 0–250 ms of jitter. If `retryAfter` is present and earlier than the backoff would resume, the integration sleeps until `retryAfter` instead. After exhausting retries the error is returned to the consumer unmodified except that `retryAfter` reflects the final attempt's value. `INSUFFICIENT_PERMISSIONS`, `SYNC_CURSOR_ERROR`, `NOT_FOUND`, and `MISCONFIGURED` are never retried.

### Profile and account info

10. `get_profile` returns `{emailAddress, messagesTotal, threadsTotal, historyId}` from `users.getProfile`. Consumers use `historyId` as a bootstrap cursor before any sync; using the first message's `historyId` is incorrect and not supported.

11. `list_send_as` returns the list of `sendAs` aliases the user has configured: `[{sendAsEmail, displayName, replyToAddress, isDefault, isPrimary, verificationStatus, signature}]`. Consumers use this to populate "send-from" pickers and to compute the user's `handle` + `handleAliases` set for direction detection.

### Send

12. `send_email` accepts:
    - `to`: `string | string[]` (required, at least one recipient when no `bcc`)
    - `cc`: `string | string[]` (optional)
    - `bcc`: `string | string[]` (optional)
    - `subject`: `string` (required, may be empty)
    - `text`: `string` (optional)
    - `html`: `string` (optional)
    - `attachments`: `[{filename, content (base64 string), contentType}]` (optional)
    - `inReplyTo`: `string` (RFC 2822 Message-ID of the message being replied to, optional)
    - `references`: `string | string[]` (optional, defaults to `inReplyTo` when omitted)
    - `threadId`: `string` (Gmail thread ID, optional)
    - `from`: `string` (must be one of `sendAs` aliases of the connected account; defaults to the primary handle)

13. `send_email` returns `{messageId, threadId, headerMessageId}`. `headerMessageId` is the RFC 2822 Message-ID generated by the MIME composer and is globally unique across mailboxes. The composer always emits this header; if it is missing for any reason the field is the empty string and the action still succeeds.

14. The MIME message is built via a nodemailer `MailComposer`. The composer is responsible for RFC 2047 encoding of headers (non-ASCII subjects, display names), `multipart/alternative` when both `text` and `html` are present, `multipart/mixed` when attachments are present, RFC-compliant `In-Reply-To` / `References` headers, and proper folding. The integration never builds MIME by hand.

15. When `threadId` is provided, it is set on the Gmail send body so Gmail places the new message in that thread. If `threadId` is provided but `inReplyTo` is not, the integration does not fabricate one — threading still works at Gmail's level via `threadId`, but mail clients reading the message will not see the In-Reply-To chain.

16. The `Bcc` header is kept in the compiled buffer (`keepBcc = true`) so the consuming app can persist a faithful copy of what was sent. Gmail strips it from the delivered message; this is Gmail's behavior, not the integration's.

17. When `from` is provided and is not one of the `sendAs` aliases of the connected account, the integration returns `{ok: false, error: {code: "MISCONFIGURED", message}}`. Gmail itself enforces this; the integration validates pre-flight to avoid a wasted API call.

18. When `attachments` is provided, each attachment's `content` is a base64-encoded string of the file bytes. Total raw MIME size after composition must not exceed 35 MB (Gmail's `users.messages.send` documented limit). If exceeded, the integration returns `{ok: false, error: {code: "MISCONFIGURED", message: "message too large"}}` without attempting the send.

19. `create_draft` accepts the same input as `send_email` and returns `{draftId, messageId, threadId, headerMessageId}`. It uses `users.drafts.create` instead of `users.messages.send`. All other invariants apply.

### List and retrieve

20. `list_emails` accepts `{query?, maxResults?, labelIds?, pageToken?, format?, metadataHeaders?}`:
    - `query`: Gmail search query string, passed through unchanged. Maximum 1024 characters.
    - `maxResults`: integer, default 100, capped at 500.
    - `labelIds`: list of label IDs to filter by; omitted means no label filter.
    - `pageToken`: opaque cursor from a previous call.
    - `format`: `"id"` (default — returns only ids) | `"metadata"` (returns headers selected by `metadataHeaders`) | `"full"` (returns full parsed messages).
    - `metadataHeaders`: list of header names to include when `format=metadata`; defaults to `["From","To","Cc","Subject","Date","Message-ID","In-Reply-To","References"]`.

21. `list_emails` returns `{messages, nextPageToken, resultSizeEstimate}`. Each message item in `messages` reflects the requested `format`. When `format` is `metadata` or `full`, the integration parallelizes the per-message fetch using the batched googleapis client at a concurrency that matches the underlying batch (max 50 per batch). On individual message failure (404, transient), that message is omitted from the result and an entry `{id, error: GmailError}` is appended to a `failures` field on the response.

22. `get_email` returns the parsed message described in invariants 24–28. It accepts `{messageId, format?}` where `format` is `"metadata"` | `"full"` (default).

23. `batch_get_emails` accepts `{messageIds, format?}` (where `format` defaults to `"full"`) and returns `{messages, failures}` using the batched googleapis client at `maxBatchSize: 50`. If the batch endpoint returns an error for the whole batch, the integration retries each ID individually with `Promise.allSettled` to maximize partial success. If `@jrmdayn/googleapis-batcher` is unavailable or fails at module load, the integration falls back transparently to individual calls with `Promise.allSettled` capped at concurrency 25.

### Message parsing

24. The parsed message shape is:

    ```
    {
      id: string,
      threadId: string,
      historyId: string,
      headerMessageId: string,           // RFC 2822 Message-ID, globally unique
      internalDate: number,              // ms since epoch (Gmail's internalDate)
      from: { name: string, address: string },
      to: [{ name, address }],
      cc: [{ name, address }],
      bcc: [{ name, address }],
      replyTo: [{ name, address }],
      deliveredTo: [{ name, address }],
      inReplyTo: string | null,          // header value, may be null
      references: string[],              // header value split, may be empty
      subject: string,
      date: string,                      // ISO 8601, from Date header or internalDate fallback
      snippet: string,
      labelIds: string[],
      bodyHtml: string,                  // raw HTML, decoded; empty string if not present
      bodyText: string,                  // raw text/plain, decoded; empty string if not present
      attachments: [{
        id: string,                      // Gmail attachmentId, used by get_attachment
        filename: string,
        mimeType: string,
        size: number,                    // bytes
        contentId: string | null,        // RFC 2392 cid: for inline images, null otherwise
        isInline: boolean
      }]
    }
    ```

25. The integration parses recipient headers into structured `[{name, address}]` lists. The `address` is lowercased; `name` preserves original casing. Multiple addresses in a single header (comma-separated, RFC 2822) all appear in the list. Malformed addresses are skipped but do not fail the call.

26. `bodyHtml` and `bodyText` are returned independently. If the message has only one of the two, the other is the empty string. The integration does not synthesize a missing `bodyText` from `bodyHtml`; that conversion belongs in the consuming app.

27. Attachments are listed without their bytes. Consumers fetch bytes via `get_attachment`.

28. When required fields cannot be parsed — `headerMessageId` is missing, both `from` and `to`/`cc`/`bcc`/`deliveredTo` are missing, or `internalDate` is missing — the integration returns the message with the partial fields filled, an additional `parseWarnings: string[]` field listing what was missing, and proceeds. It does not drop the message. Consumers decide whether to persist partial messages.

### Attachments

29. `get_attachment` accepts `{messageId, attachmentId}` and returns `{data, size}` where `data` is a base64-encoded string of the attachment bytes. The integration does not cache attachments. Consumers download lazily.

### History (incremental sync)

30. `history_list` accepts `{startHistoryId, maxResults?, pageToken?, historyTypes?}`:
    - `startHistoryId`: required.
    - `maxResults`: default 500, capped at 500 (Gmail's limit).
    - `historyTypes`: default `["messageAdded", "messageDeleted", "labelAdded", "labelRemoved"]`.

31. `history_list` paginates internally and returns the full result for the page-token window: `{messagesAdded: string[], messagesDeleted: string[], labelsAdded: [{messageId, labelIds}], labelsRemoved: [{messageId, labelIds}], historyId: string, nextPageToken: string | null}`. Duplicate IDs (e.g. a message both added and deleted in the same history window) are de-duplicated, with deletion winning over addition for the same id.

32. When Gmail returns HTTP 404 on `/history` (history ID expired, more than ~7 days old or pruned), the integration returns `{ok: false, error: {code: "SYNC_CURSOR_ERROR"}}`. It does not auto-recover; consumers are responsible for falling back to a full resync.

### Threads

33. `threads_list` mirrors `list_emails` for threads: `{query?, maxResults?, labelIds?, pageToken?}` → `{threads: [{id, snippet, historyId}], nextPageToken, resultSizeEstimate}`.

34. `threads_get` accepts `{threadId, format?}` (default `"full"`) and returns `{id, historyId, messages: [parsedMessage]}` where each `parsedMessage` is the shape from invariant 24. Consumers use this to reconstruct full threads when their folder-import policy requires sibling messages.

### Modify (opt-in)

35. `modify_email` is exposed only when `enableModifyScope: true` is set in the integration's platform config and the user's refresh token was minted with `gmail.modify`. If either condition is unmet, the action returns `{ok: false, error: {code: "MISCONFIGURED"}}`. When available, it accepts `{messageId, addLabels?, removeLabels?}` and returns `{ok: true, data: {labelIds}}` reflecting the message's labels after modification.

### Push notifications

36. `watch` accepts `{topicName, labelIds?, labelFilterAction?}` and returns `{historyId, expiration}` from `users.watch`. `expiration` is a Unix-millisecond timestamp ~7 days in the future (Gmail's documented behavior). Consumers are responsible for re-calling `watch` before expiration; the integration does not schedule renewal.

37. `stop_watch` calls `users.stop` for the user and returns `{ok: true}`.

38. The `__webhook` handler decodes the Cloud Pub/Sub envelope. Its return value is:

    ```
    { event: "push_notification", emailAddress: string, historyId: string }
    ```

    The integration does **not** map `emailAddress` to a RootCX user; that mapping is the consuming app's responsibility (each app maintains its own `(emailAddress → userId)` lookup as part of its sync state). When the envelope is malformed or missing required fields, the handler returns `{skipped: true, reason: string}` and the request still completes with 200 to satisfy Pub/Sub's at-least-once delivery contract.

### Configuration

39. The integration's `configSchema` exposes:
    - `clientId`, `clientSecret` (platform secret) — used in self-hosted OAuth.
    - `proxyToken`, `baseUrl` (platform secret) — used in managed mode.
    - `enableModifyScope` (boolean, default false) — opt-in for `gmail.modify`.
    - `pubsubTopicName` (string, optional) — pre-configured Pub/Sub topic for `watch` calls.

40. When a consumer calls an action that requires a config field the integration doesn't have (e.g. `watch` without `pubsubTopicName` and without `topicName` in input), the integration returns `{ok: false, error: {code: "MISCONFIGURED", message}}`.

### Permissions

41. Each action declares an RBAC permission key `integration:gmail:<action>` matching the existing RootCX convention. The runtime enforces these before dispatching to the integration; the integration itself trusts that authorization has already happened upstream.

### Versioning

42. The integration manifest declares `version: "0.2.0"`. Field additions to response shapes are backward-compatible minor bumps; field removals or rename are major bumps. The error contract (invariants 6–9) is part of the public version contract: codes are never removed; new codes may be added in minor bumps and consumers must treat unrecognized codes as `UNKNOWN`.

## Open questions

- **Open question (invariant 18):** Should the integration accept attachments by `file_id` (RootCX platform storage) in addition to inline base64? An RPC payload with 35 MB of base64 is unwieldy. Reference-by-id would require the integration to call back into the runtime to fetch bytes, breaking its "stateless wire" property. Proposed answer: keep base64-only in v0.2; revisit if helpdesk needs large attachments routinely.

- **Open question (invariant 36):** Should `watch` accept a renewal callback URL so the integration can self-schedule renewal? Proposed answer: no — the integration stays stateless. Apps schedule their own renewal via RootCX crons.
