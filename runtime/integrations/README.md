# RootCX integration plugins

RootCX owns this runtime and its provider adapters. They call vendor APIs directly;
no hosted integration broker or paid connector service is required. This is a
first-party implementation, with MIT validation dependencies listed in
`THIRD_PARTY_NOTICES.md`. Vendor accounts, API entitlements, quotas and OAuth approval
remain the vendor's responsibility.

**Release status: implemented and locally tested; live provider qualification pending.**
No test accounts or OAuth applications were available for the implementation session.
Every generated manifest says `requires-live-verification`. Neither a fixture test nor
successfully saving a connection is a complete production qualification.

## 1.1.0 qualification candidate — 15 October 2026

Ten rebuilt plugins, 62 actions including ten account checks. Local evidence:
77 plugin/contract/artifact tests, TypeScript and artifact integrity checks passed;
26 Core integration library and 17 HTTP connection tests passed against disposable
PostgreSQL. No live provider accounts were available for the nine non-Drive services.
No production deployment or provider qualification is claimed for this candidate.

Hardening covers OAuth client-vs-grant failures, Slack HTTP-200 token errors,
extreme expiration values, malformed OAuth grants, bounded cooldowns, uncertain
writes and POST read semantics. Provider changes add paginated Notion property
reads, numeric HubSpot search offsets, read-only Salesforce queries, folded Gmail
headers, strict Calendar dates, Slack commercial history limits and Stripe metadata
validation. Sheets RAW behavior was checked; no adapter change was necessary.

Migration notes: re-save existing HubSpot OAuth configuration with
`https://api.hubspot.com/oauth/v3/token`; persisted v1 URLs are not migrated by the
manifest. Calendar dateTime requires an explicit UTC offset even with timeZone.
Slack history limit is now at most 15. Stripe metadata values must be strings.
Local disconnect removes stored credentials atomically; it does not revoke the
remote provider consent. Operator-managed Google OAuth still covers Drive only.
The SHAPP operator setup and per-provider live recipe are recorded in
`rootcx-website/docs/shapp/integrations.md` (15 October section).

## Build and verification

From this directory, using Bun 1.3.10:

```sh
bun install --frozen-lockfile --ignore-scripts
bun run typecheck
bun run build
bun test test
bun build.ts --check
```

`build.ts` compiles one self-contained worker per provider into
`core/resources/integrations/<id>`. The registry is a **build-time** registry: workers
load only their provider, not the whole catalog. Each release has protocol/version
metadata and SHA-256 integrity checks. `--check` detects stale generated artifacts.
Bun/Ajv are pinned; standalone connectors need no dependency download when installed.
Gmail/Calendar retain their existing synchronization workers/dependencies and expose
new live actions under `live_*`; their existing action names and entities survive.

The Core tests are run with the repository's PostgreSQL image (`pgmq`, `pg_cron`):

```sh
make core-unit FILTER=extensions::integrations
make core-test TEST=connections_integration
```

## Supported operations

Each plugin also has `verify_connection`. These are deliberately bounded capabilities,
not a claim to implement every endpoint of each SaaS.

| Plugin | Implemented live operations | Authentication |
| --- | --- | --- |
| Notion | Discover data sources; schema; query/read/create/update pages | Internal token or OAuth |
| Stripe | List/read/create/update customers; list subscriptions | Secret/restricted key |
| HubSpot | List/search/read/create/update contacts; property schema | Private token or OAuth |
| Salesforce | Contact/Opportunity describe, SOQL query, read/create/update | OAuth on login.salesforce.com |
| Airtable | Discover bases/schema; list/read/create/update records | Personal token or OAuth + PKCE |
| Gmail | Search/read mail; create draft; send plain text | OAuth + PKCE |
| Google Calendar | List calendars/events; read/create/update events | OAuth + PKCE |
| Google Sheets | Discover spreadsheets/worksheets; read, append, update values | OAuth + PKCE |
| Google Drive | List/read metadata; download/export; multipart upload | OAuth + PKCE |
| Slack | List channels; history; post message/thread reply | Bot token or OAuth |

Notion is a complete replacement using data sources and API version `2025-09-03`.
No legacy `apiToken`, `databaseId` or normalized lossy property conversion is used.
Writes accept native property values and reject unknown/computed fields before sending.
Stripe pins `2025-09-30.clover`; Salesforce uses REST v61.0. Other API versions are
explicit in provider paths. Salesforce sandbox/My Domain authorization endpoints are
not yet exposed in this first release; the returned API instance must be an HTTPS
Salesforce domain. This restriction must be extended with reviewed endpoints, not
arbitrary user-supplied hosts.

## Adding a provider without changing Core

1. Add a typed module in `src/providers` exporting `Plugin`, then register it in
   `src/registry.ts`. There is no runtime switch statement to edit in Core or Shappy.
2. Declare schemas, native operations, effects, provider origins and service aliases.
   `action.run` is ordinary TypeScript: use vendor-specific pagination, multi-request
   flows, GraphQL JSON bodies, multipart or binary requests when required.
3. The default authentication header is Bearer. A reviewed `authorize` hook can set
   API-key headers, Basic auth or request signatures; `credentialSchema` can validate
   extra credential fields. OAuth currently handles code flow, optional PKCE,
   form/JSON exchanges, body/Basic client authentication, scope separators and
   provider account verification. New auth protocols (device flow, OAuth 1, SAML,
   arbitrary provider refresh algorithms) require a versioned SDK extension; they
   are not silently forced through OAuth 2.
4. Declare optional discovery metadata. Shappy connects accounts, verifies access,
   binds the selected account to the current app, and lists sources from this
   metadata. The current token UI accepts one access token; providers requiring
   extra credential fields need a corresponding form extension.
5. Write wire-contract and failure tests, build, and run the live qualification
   checklist below before enabling customer use.

Plugins are **trusted, reviewed code shipped with Core**, running in the existing
worker sandbox and principal boundary. Uploading untrusted third-party executable
plugins or runtime package installation from a public registry is not supported.
Hashes detect release corruption, not a trusted publisher's malicious code.

## Reliability and security boundaries

- Core encrypts credentials and OAuth state. State is hashed, single-use, expires
  after 10 minutes and persists across process restarts/replicas. PKCE verifiers
  remain server-side.
- Refresh tokens are renewed in Core under a PostgreSQL transaction advisory lock.
  Rotated tokens persist before worker use. Compare-and-swap prevents resurrection
  after disconnect/reconnect. Unknown token lifetime forces renewal before reuse
  rather than assuming an hour of validity (notably Salesforce).
- Permission checks and app/user/account bindings remain in Core. Explicit account
  selection uses `x-integration-connection` and must belong to the caller, even for
  admins. It cannot be combined with an app binding header.
- Action schemas reject unknown parameters. HTTP origins are reviewed, redirects
  are denied, responses/requests are bounded to 10 MiB, HTTP timeouts are 20 seconds.
  Drive upload is limited to 1 MiB. OAuth exchanges time out after 15 seconds.
- Local token buckets and four in-flight requests per connection limit a worker.
  `Retry-After` is honored after 429. These budgets are **per worker**, not a global
  tenant/provider quota service; provider throttling remains authoritative across
  replicas. There is no automatic write retry. Network/5xx/invalid successful write
  responses report `outcome: unknown`; reconcile against the provider first.
- Stripe creation requires a stable idempotency key. Calendar creation requires a
  stable event ID. Other APIs cannot promise exactly-once creation: no local copy
  or supposed universal upsert hides that limitation. Stable provider IDs are used
  for updates; Sheets coordinates are not record identities.
- Stateless plugins start on demand and share Core's bounded worker pool. Existing
  Gmail/Calendar lifecycle workers remain. Thousands of active concurrent accounts
  still require capacity planning/load tests; catalog size is not proof of throughput.
- No new webhook ingestion, background sync, event queue, cursor persistence or
  bulk import engine is claimed here. Existing Gmail/Calendar synchronization remains
  separate from the new live actions and may persist its own mail/calendar data.
- Error envelopes omit raw upstream bodies and secrets. API output may contain
  customer data by design and must not be indiscriminately logged by applications.

## Configure and qualify a deployment

1. Build/release Core with the generated resources and the updated Shappy bundle.
   Installing a catalog version is an explicit admin action; do not assume existing
   tenant installations upgrade themselves. Back up existing packages/config first.
   The old Notion connector is intentionally incompatible; reconnect it and update
   old app action names/inputs. No legacy Notion data migration is required here.
2. Install each provider from the catalog. For OAuth, create your own vendor OAuth
   app and register the deployment's `OAUTH_CALLBACK_URL` exactly. In hosted SHAPP,
   use the central router URL, not the UI origin. Configure the same tenant/router
   HMAC settings already required by Core. Save client ID/secret in Shappy, which
   also saves the declared token URL/client authentication/encoding in Core.
3. Grant the scopes listed in `manifest.json`. Google may require consent-screen
   verification and restricted-scope review. Gmail/Calendar scopes include the
   existing synchronization actions (Gmail settings/modify, full Calendar access).
   Slack requires the bot to be allowed in each private channel. Notion/Airtable
   require explicit resource sharing. HubSpot account verification requires `oauth`.
4. Connect dedicated test accounts. Token connections are verified against the real
   account endpoint **before** storage. OAuth connections are verified after exchange.
   Select the account, verify it, bind it to a test application with user scope.
5. For each read path, test empty and multi-page results, missing permissions and
   inaccessible resources. For writes, create one uniquely labeled fixture, read it
   independently, update **the same provider ID**, and read it again. Test Unicode,
   optional/null values and native types. Sheets: append, read exact coordinates,
   update those coordinates. Gmail: draft and send to your own test recipient.
   Slack: post to a dedicated private test channel. Drive: upload/download binary
   and export a real Workspace document. Do not use customer production records.
6. Exercise expiration/rotation, revocation/reconnection, 429, uncertain writes and
   simultaneous account access through Core. Confirm the app cannot borrow another
   user's connection. Record provider ID, granted scopes, artifact hashes, UTC
   timestamp, operation and result; exclude tokens, secrets and customer contents.
7. Remove only test objects you created. Keep the qualification evidence and approve
   the exact release artifacts before production rollout.

`bun live-check.ts <provider> <connection-id>` performs a real read-only account probe
through Core, taking `ROOTCX_CORE_URL` and `ROOTCX_ACCESS_TOKEN` from the environment.
It never creates test accounts or writes provider data. Successful probe ≠ full recipe.

## Official API references

- [Notion data sources](https://developers.notion.com/reference/query-a-data-source)
- [Stripe customers](https://docs.stripe.com/api/customers)
- [HubSpot account scopes](https://developers.hubspot.com/docs/api-reference/legacy/account/account-information/guide)
- [Salesforce REST](https://developer.salesforce.com/docs/atlas.en-us.api_rest.meta/api_rest/intro_rest.htm)
- [Airtable OAuth](https://airtable.com/developers/web/api/oauth-reference)
- [Gmail](https://developers.google.com/workspace/gmail/api/reference/rest)
- [Calendar](https://developers.google.com/workspace/calendar/api/v3/reference)
- [Sheets](https://developers.google.com/workspace/sheets/api/reference/rest)
- [Drive](https://developers.google.com/workspace/drive/api/reference/rest)
- [Slack Web API](https://docs.slack.dev/reference/methods/)


### Operator-managed Google OAuth

Core optionally supports `ROOTCX_MANAGED_GOOGLE_OAUTH=true` with
`ROOTCX_GOOGLE_CLIENT_ID` and `ROOTCX_GOOGLE_CLIENT_SECRET` supplied by the operator.
Keep `OAUTH_CALLBACK_URL` fixed to the trusted deployment callback. This is used
by SHAPP; standalone deployments keep their existing configuration by default.

In this mode Core owns Google's authorization URL, PKCE exchange, Drive permission
check and account probe. Tenant API writes cannot replace the operator client or
submit manual credentials. The shared client secret stays in Core, not plugin
RPC payloads. Renewal resolves the operator config before stripping refresh tokens
and config from worker calls. Failure to resolve configuration fails closed.

`POST integrations/google_drive/auth/start` also returns `attemptId`. Poll
`GET integrations/google_drive/auth/attempts/{attemptId}` using the initiating
user's session. A successful receipt contains `connectionId`; pending, cancelled
and failed receipts never expose one. Receipts expire after ten minutes.
`POST integrations/google_drive/connections/{connectionId}/verify` verifies only
the caller's own account, without requiring a company administrator role.
The normal user-scoped app-binding API remains the explicit consent boundary.

SHAPP setup, exact Google Console steps, local import and production secret wiring:
`rootcx-website/docs/shapp/google-drive-oauth.md`. No live Google qualification has
been performed without operator credentials; the production Drive scope remains
restricted and needs the applicable Google review.

## LinkedIn · API Builderz 0.1.0 — 30 October 2026

`linkedin_api_builderz` adds five read actions and an access probe through
`realtime-linkedin-bulk-data.p.rapidapi.com`, using each customer's subscription/key.
See [the action contract and qualification evidence](LINKEDIN-API-BUILDERZ.md).

Eight playground calls exercised all five endpoints and pagination; seven succeeded,
one returned provider HTTP 502. The last response reported 43/50 requests remaining.
Live evidence corrected distinct interaction URNs and profile pagination tokens.
154 runtime tests, TypeScript, artifact integrity and the Core catalog tamper test
passed. No tenant connection, deployment or change to existing provider artifacts.
