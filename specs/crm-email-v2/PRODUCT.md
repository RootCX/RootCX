# CRM Email v2 — PRODUCT.md

## Summary

CRM users (sales reps, account managers) can connect their Gmail, Outlook, or IMAP account and read, reply to, and send emails from inside the CRM with first-class threading, attachments, HTML rendering, and contact auto-matching. Emails synced by one user are deduplicated across the workspace so the team shares a single source of truth for each conversation.

## Problem

v0.1 supports inbox sync from Gmail/Outlook/IMAP and shows emails as plain text on the contact page. It cannot reply in-thread, attach files, or render HTML. Token refresh, retry, and error handling are ad-hoc. Direction (incoming/outgoing) is detected by regex on folder names, which breaks in non-English locales. The bootstrap cursor for Gmail full sync is taken from the first message's `historyId`, which can drop messages received during the initial list. Sync state is not robust under quota errors — failures don't back off, they just retry every 5 minutes.

## Goals

- Reply, forward, and compose with attachments, in-thread.
- Thread view on the contact page grouping related messages.
- HTML body rendering, safely sandboxed.
- Per-user connection that survives token revocation, transient errors, and quota throttling without user intervention beyond reconnect when needed.
- Workspace-shared email store with per-association direction (the same email is "outgoing" for Alice and "incoming" for Bob, naturally).
- Sync continues to run on a cron with no infrastructure change. Real-time push is a later iteration.

## Non-goals

- Open / click tracking pixels (separate feature).
- Email templates and mailmerge (separate feature).
- Domain-wide delegation for Workspace admin (separate feature).
- Per-row ACLs on emails ("Alice cannot see Bob's mailbox"). The CRM is a shared store; gating is RBAC at the entity level.

## Behavior

### Provider connection

1. The Email Settings tab lists three providers — Gmail, Outlook, IMAP/SMTP — each with a "Connect" CTA. The connected state is per-user: each workspace member connects their own account.

2. Connecting Gmail opens an OAuth popup using the Gmail integration's `__auth_start`. On success, the popup closes and the Email Settings card flips to a Connected state within 5 seconds. If the popup is closed without consent, the card stays in the disconnected state and shows an inline "Cancelled" message until the user clicks Connect again.

3. Each successful connection creates (or reuses) a `sync_state` row with `{user_id, provider, enabled: true, status: "idle", handle, handle_aliases, watch_expires_at: null}`. `handle` is the primary address from `list_send_as` (Gmail), `userPrincipalName` (Outlook), or `username` (IMAP). `handle_aliases` is the full list of `sendAs.sendAsEmail` for Gmail; empty for Outlook/IMAP today.

4. Each connection registers a 5-minute cron job (`*/5 * * * *`) with payload `{user_id, provider}` and `overlapPolicy: "skip"`. Disconnecting removes the cron and sets `sync_state.enabled = false`. The `emails`, `email_channel_associations`, `email_participants`, and `email_threads` rows are not deleted on disconnect — only the cron and credentials are removed.

5. The connection card shows live status from `sync_state`:
   - `idle` (Connected, last synced N min ago)
   - `syncing` (spinner, "Syncing…")
   - `needs_reauth` (red, "Reconnect" button replacing "Sync now")
   - `failed_temporary` (orange, "Throttled — next retry at HH:MM")
   - `failed_permanent` (red, "Sync failed — contact support" with error message)

6. "Sync now" triggers the cron immediately. It is disabled while `status = syncing` or `status = failed_temporary` with `throttle_retry_after > now`.

### Sync — list fetch

7. On the first run for a Gmail account, the CRM calls `get_profile` on the Gmail integration to obtain a snapshot `historyId`, stores it as `sync_state.cursor`, then calls `list_emails` paginated with the workspace's exclusion filter (`-in:spam -in:trash -in:drafts -category:promotions -category:social`, max 500 per page) until exhausted, and enqueues all new message IDs in `email_import_queue`. The cursor is set to the profile's `historyId` taken **before** the list call, not after — this guarantees no messages received during the initial list are missed.

8. On subsequent runs for Gmail, the CRM calls `history_list` with `historyTypes: ["messageAdded","messageDeleted","labelAdded","labelRemoved"]` and the stored cursor. Added messages are enqueued. Deleted messages immediately delete the corresponding `email_channel_associations` rows for this `(user_id, provider, external_id)` — the `emails` row is left intact because another user's association may still reference it. Label changes update the `email_channel_associations.label_ids` JSON column.

9. When `history_list` returns `SYNC_CURSOR_ERROR`, the CRM clears the cursor, sets `sync_stage = "list_fetch"`, and re-runs the bootstrap from invariant 7 on the next tick. The user sees this as a one-cycle pause in sync, no error message.

10. Outlook uses `delta_list` with the Microsoft Graph delta link as cursor; IMAP uses `(uidValidity, highestUid)` per folder. Both follow the same enqueue → import pipeline.

### Sync — messages import

11. After list fetch, the CRM dispatches a `messages-import` job (or re-uses the same cron tick within the 4-minute budget) that:
    - Atomically claims a batch of up to 10 IDs from `email_import_queue` (`DELETE … RETURNING`), so two concurrent workers cannot process the same id.
    - Calls `batch_get_emails` on the Gmail integration with `format: "full"`.
    - For each parsed message: upserts into `email_threads` (key: `external_id`), upserts into `emails` (key: `header_message_id`), upserts into `email_channel_associations` (key: `user_id, provider, external_id`), inserts `email_participants` (key: `email_id, address, role`).
    - Enqueues failed IDs back into `email_import_queue` so they retry on the next cycle.

12. Up to 6 batches (60 messages) run per cron tick or until the 4-minute job budget is exhausted, whichever comes first. If the queue still has items at the end, the CRM dispatches a follow-up job immediately so import does not wait 5 minutes per batch.

13. After parsing, the CRM stores three body variants on `emails`:
    - `body_html`: raw HTML from the integration (empty if not present).
    - `body_text_raw`: raw text/plain from the integration, or fallback derived from HTML if only HTML is present.
    - `body`: `body_text_raw` with quoted-reply chains stripped via the `planer` library (`extractFrom` for text, `extractFromHtml` for HTML→text). This is what the timeline shows by default.

14. `subject` is truncated to 1000 characters before storage. `body_html`, `body_text_raw`, and `body` are each truncated to 2 MB. Truncation is silent; the original is still retrievable from Gmail via `get_email`.

15. Direction on `email_channel_associations` is computed as:
    - If `from === sync_state.handle` or `from ∈ sync_state.handle_aliases` → `outgoing`.
    - Else if any of `to/cc/bcc/delivered_to` contains `sync_state.handle` or an alias → `incoming`.
    - Else `incoming` (default).
    
    Direction is **per-association**, not per-email. The same `emails` row may have `outgoing` for Alice's association and `incoming` for Bob's.

16. Messages received without a `header_message_id` are stored with a synthetic key `fallback-<user_id>-<external_id>` and flagged `header_message_id_synthetic: true`. Cross-mailbox deduplication does not apply to synthetic rows.

17. `email_participants.contact_id` is populated by matching lowercased `email_participants.address` against `contacts.email` at insert time. Unmatched participants stay with `contact_id = null` and are re-matched by a job on `contacts.created` / `contacts.updated` events.

### Throttle and error handling

18. `sync_state` carries `throttle_failure_count: int` (default 0) and `throttle_retry_after: timestamptz | null`.

19. When the integration returns `TEMPORARY_ERROR`, the CRM increments `throttle_failure_count`, sets `throttle_retry_after = max(error.retryAfter, now + 2^count * 60s)` with `count` capped at 5, and exits the current job. The cron picks up the next tick and skips the job if `now < throttle_retry_after`. After **5** consecutive failures the status flips to `failed_permanent` and the cron stops attempting; a manual "Sync now" resets the count.

20. When the integration returns `INSUFFICIENT_PERMISSIONS`, the CRM sets `status = needs_reauth` immediately. The "Connect" button reappears. On successful reconnection, the count and status reset.

21. When the integration returns `SYNC_CURSOR_ERROR` (only possible during `history_list`), the CRM clears `cursor`, sets `sync_stage = "list_fetch"`, and continues without incrementing `throttle_failure_count`.

22. On success, `throttle_failure_count` is reset to 0 and `throttle_retry_after` is set to `null`.

### Send

23. From a contact's email tab, "Compose" opens a form with To (prefilled with contact email), Cc, Bcc, Subject, body (rich-text editor with bold/italic/link/lists), and an attachment uploader (multiple files, each up to 25 MB, total up to 30 MB so headroom remains under Gmail's 35 MB MIME limit).

24. The user can pick the "From" address from a dropdown listing their primary handle plus aliases (`sync_state.handle_aliases`). The default is the primary handle.

25. "Reply" / "Reply all" / "Forward" buttons on a message open the same compose form, prefilled:
    - **Reply**: To = original `from`, Subject = `Re: <original subject>` (no double "Re:"), body starts with editor's reply-quote section pointing at the original `body_html`. Hidden state carries `reply_to_message_id`.
    - **Reply all**: same as Reply, plus original `to` and `cc` (excluding the current user's handle) in To/Cc.
    - **Forward**: To empty, Subject = `Fwd: <original subject>`, body contains the original headers + body as a forwarded block. Hidden state carries `reply_to_message_id` but **does not** propagate `inReplyTo`/`References` to the integration (forwards start a new thread).

26. On send, the CRM:
    - Resolves `reply_to_message_id` (if set) to fetch `header_message_id`, `references`, and `thread_external_id` from `emails`.
    - Uploads attachments to RootCX storage, then re-fetches their bytes as base64 to pass to the integration. (v0.2 ships base64 inline; future iteration may pass `file_id` once the integration supports it.)
    - Calls `gmail.send_email` with `to/cc/bcc/subject/text/html/attachments/from`, plus `inReplyTo: header_message_id`, `references: [...existing references, header_message_id]`, `threadId: thread_external_id` for replies (not for forwards).
    - On `{ok: true}`: shows "Sent" toast, closes form. Immediately persists the sent message via the same `messages-import` code path with `direction: outgoing` so the user sees their reply in the thread without waiting for the next sync.
    - On `{ok: false, error}`: maps error code to UI:
      - `INSUFFICIENT_PERMISSIONS` → "Reconnect Gmail" prompt.
      - `TEMPORARY_ERROR` → "Gmail is rate-limiting. Retry in N seconds." (uses `retryAfter` if present).
      - `MISCONFIGURED` (e.g. message too large) → inline form error.
      - any other → generic error toast with "View details" expander.

27. "Save as draft" calls `create_draft` instead. Drafts are not synced back to the CRM in v0.2; the user is told the draft is available in Gmail's drafts folder.

### Thread view

28. On a contact's email tab, messages are grouped by `email_threads.id`. Each thread displays:
    - A collapsed header showing the most recent message's subject, participant chip list, last received time, message count, and unread indicator (any association where `is_read = false`).
    - On click, the thread expands to show messages in chronological order (oldest first).
    - Each message has a compact header (avatar, sender name + address, date), a body, and per-message actions: Reply, Reply all, Forward, Open in Gmail (deep link `https://mail.google.com/mail/u/0/#all/<external_id>`).
    - Quoted-reply chains in the body are collapsed by default behind a "Show trimmed content" expander. The expander reveals `body_text_raw` (or rendered HTML for Gmail).

29. Bodies render HTML if `body_html` is present. Rendering happens in a sandboxed iframe with `sandbox="allow-popups allow-popups-to-escape-sandbox"` (no `allow-scripts`, no `allow-same-origin`) and the HTML is passed through DOMPurify before injection. Links open in a new tab with `rel="noopener noreferrer"`. External images are blocked by default behind a per-thread "Show images" toggle that the user can flip; the toggle is sticky per thread for the session.

30. If `body_html` is empty, the CRM renders `body` (cleaned plain text) with auto-linkification of URLs.

31. Attachments listed on the message show as chips with filename, size, and MIME type icon. Clicking a chip triggers `gmail.get_attachment` and serves the bytes as a download. Inline images (`isInline: true`, `contentId: cid:...`) are rendered inside the HTML body via a runtime substitution of `cid:` references with `data:` URLs, gated by the same "Show images" toggle.

### Multi-user behavior

32. When two workspace members are both in the To/Cc of the same external email, only one `emails` row exists (deduplicated by `header_message_id`), but each member has their own `email_channel_associations` row with their own `direction` (typically `incoming` for both unless one is the sender via an alias).

33. The contact's email tab shows the union of emails associated with **any** workspace member, filtered by participant address matching the contact. Two users looking at the same contact see the same thread list. If a member disconnects, their associations are removed but the emails remain visible to others.

34. `emails.read` and `emails.write` permissions are workspace-wide. There is no per-user filtering of which emails are visible (out of scope, see Non-goals).

### Failure recovery

35. A "Reset email data" action in Email Settings deletes all `emails`, `email_channel_associations`, `email_participants`, `email_threads`, `email_import_queue`, and `sync_state` rows after a typed confirmation. The user must reconnect after reset. This action does not delete attachments stored in RootCX platform storage.

36. A workspace admin can trigger a forced full resync on a single user's connection from a settings sub-action: it clears `cursor` and `sync_stage = "list_fetch"` without touching credentials. The next cron tick restarts from the beginning.

37. If a `messages-import` job dies mid-batch (worker crash), the unprocessed queue items are already deleted (claimed). They are re-enqueued by an hourly `stale-cleanup` cron that finds any `sync_state.status = "syncing"` older than 30 minutes, resets it to `idle`, and re-enqueues IDs that have not produced an `email_channel_associations` row from the last claimed batch (tracked via a `last_claimed_ids` JSON column on `sync_state`).

### Audit

38. Every `sync_state` transition (status change, cursor advance, reset, reauth, throttle) emits an event to RootCX's audit_log table with `app_id: "crm"`, `entity: "sync_state"`, `action: <transition>`, `user_id`, `details: {old, new}`. Send actions emit `entity: "emails", action: "sent", details: {to, subject, threadId, headerMessageId}`. This is the only place the CRM intentionally writes to the platform audit log; record CRUD already audits via the runtime.

## Open questions

- **Open question (invariant 26):** Should the CRM enforce a per-day quota on outbound sends per user (e.g. 100/day) as a guardrail against accidental loops in scripted workflows? Proposed answer: no in v2 — Gmail's own quota is the backstop; revisit after first incident.

- **Open question (invariant 29):** Should "Show images" be remembered per contact, per workspace member, or globally per session? Proposed answer: per session, per thread (most conservative — minimizes tracking-pixel surface without nagging the user repeatedly in one work session).

- **Open question (invariant 33):** When a contact has no email but `email_participants.address` matches via a different identifier (e.g. the contact has a personal email recorded but is corresponding from a work address), should the thread surface? Proposed answer: no in v2. Match strictly by `contacts.email`.
