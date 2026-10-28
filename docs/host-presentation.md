# Host-owned presentation

Core owns authorization, OAuth exchanges, approval signatures, durable operations and source history. A host may supply presentation through additive contracts; standalone Core needs no product service.

## Builder

Run responses (including SSE, conversation history and activity) expose optional `errorCode` alongside `message`. Known failures have a stable code and a safe generic fallback. Raw diagnostics stay server-side. Hosts should translate recognized codes and display `message` for absent or unknown codes. Existing runs are not rewritten. The nullable `error_code` column is added idempotently at bootstrap. Publication retries clear it; failed recovery sets `PUBLICATION_RECOVERY_REQUIRED` again.

New source commits use `RootCX Builder <builder@rootcx.local>` and `Application change <run-id>`. No existing commit, source bundle or base revision is rewritten.

## OAuth

`ROOTCX_OAUTH_RESULT_URL` optionally selects a host result page. It must be an absolute HTTP(S) URL on the same origin as the operator's `OAUTH_CALLBACK_URL`, without credentials, query or fragment. `OAUTH_CALLBACK_HMAC_KEY` and `ROOTCX_TENANT_REF` must also be set. Invalid/missing configuration uses the neutral Core page.

The validated destination is saved in the encrypted pending OAuth state at start. Older pending states without this field complete on Core's neutral page. The provider callback URL, PKCE verifier, one-use state, credential verification/storage, attempt ownership and polling endpoints are unchanged.

After persisting the outcome, Core returns 303 to the saved destination with only `receipt=<payload>.<signature>`:

- `payload`: unpadded base64url JSON with `tenant`, `provider`, `status` (`connected`, `cancelled`, `failed`) and integer `issuedAt` (Unix seconds).
- `signature`: lowercase hex HMAC-SHA256 over `oauth-result-v1:<payload>` with the existing tenant callback HMAC key.
- No OAuth authorization code, access/refresh token, diagnostic or user ID is included.

The receipt authorizes **display only**. It must never grant access, approve a tool or replace the private attempt-status endpoint. Hosts verify the signature before displaying success and bound its lifetime (the portal uses 120 seconds, 30 seconds future tolerance). Replaying a receipt can only redisplay a result. Core responses and host pages use `no-store` and `no-referrer`. Without a usable signing key at callback time, Core renders its neutral result.

## Channels

Managed text messages can include `notice: { code }` in addition to their existing `kind: "text"` and `text` fallback. Codes: `ACCOUNT_LINK_REQUIRED`, `REQUEST_FAILED`, `APPROVAL_DETAILS_TOO_LARGE`. Old relays can ignore the new field. Native providers inherit fallback text handling.

The relay may translate known notices **after** checking the signature and live grant. Approval text, full arguments, signed button IDs, identity binding and expiry are not notices and must remain unchanged. Oversized arguments still cancel the approval; a notice does not authorize execution.

`ROOTCX_CHANNEL_DISPLAY_NAME` is an optional prefix for newly inserted managed channel names. Existing names, deterministic IDs, links, delegations and secrets remain unchanged. Without a prefix, Core uses the provider's name.

## Legacy companion bundles

`ROOTCX_FRONTEND_LEGACY_COMPANION_PREFIXES` is a JSON array of at most 16 absolute path prefixes, optionally containing `{app}`. Expanded prefixes are at most 256 bytes, end with `.`, contain only ASCII alphanumeric characters or `/ _ - .`, and cannot contain `..` or start with `//`.

Only exact empty module script tags whose URL is a configured prefix followed by a nonempty hexadecimal hash and `.js` are stripped from HTTP responses. Other scripts and stored files remain intact. Public share responses strip these tags without injecting a companion.

For older hosts that only supply `ROOTCX_FRONTEND_COMPANION_URL`, Core derives legacy prefixes from the loader's parent path segment: `/namespace/companion.` and `/apps/{app}/namespace/companion.`. An explicit array overrides this fallback; `[]` disables stripping. Invalid explicit configuration does not authorize stripping. Standalone deployments with legacy bundles and no loader must supply their aliases explicitly.
