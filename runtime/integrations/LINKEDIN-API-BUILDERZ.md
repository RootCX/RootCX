# LinkedIn · API Builderz 0.1.0

Generic read connector `linkedin_api_builderz`. Provider: API Builderz, distributed
through RapidAPI, independent of LinkedIn. Customer-owned subscription and key;
no shared operator credentials. No sales workflow or application-specific behavior.

## Action contract

| Action | Inputs | Provider route | Result |
| --- | --- | --- | --- |
| `list_company_posts` | `companyUrl`, optional `page` | GET `/company_updates` | Page of `posts` |
| `list_profile_posts` | `profileUrl`, optional `page`, `paginationToken` | GET `/profile_updates` | Page of `posts` |
| `list_post_reactions` | `reactionsUrn`, optional `page` | GET `/post_reactions` | Page of `reactions` |
| `list_post_comments` | `commentsUrn`, optional `page` | GET `/post_comments` | Page of `comments` |
| `get_person` | `profileUrl` | POST `/person`, JSON `{link}` | `{person}` |
| `verify_connection` | none | GET `/company_updates`, public LinkedIn company page | Access proof only |

All actions are read-only. `get_person` is a read despite the provider's POST method.
No messages, invites, contact writes, scheduler or automatic retries.

Use the exact `reactionsUrn` and `commentsUrn` returned in each post. A post's `urn`
identifies the displayed activity and can differ from the underlying UGC post.
Reconstructing interaction IDs from it can select the wrong resource. The earlier,
unreleased `postUrn` input was removed; no deployed action contract is migrated.

Page envelope: `{items, page, nextPage, nextPaginationToken, hasMore, truncated}`.
Items preserve provider fields. Next profile pages require **both** `nextPage` and
`nextPaginationToken` (passed as input `paginationToken`); page numbers alone are
rejected before HTTP. A nonempty token takes precedence over the provider's total,
which was observed as zero despite real results. If continuation is required but
its token is missing, `truncated` is true and `nextPage` is null. Unknown coverage
is `hasMore: null`, never a false completed collection. Pages are bounded at 10,000.
Other paginated endpoints use explicit page numbers. Stop when `hasMore` is false.
Collections are mutable: totals and membership can change between calls; consumers
must deduplicate and checkpoint rather than assuming a stable snapshot.

Fixed HTTPS origin; validated LinkedIn profile/company URLs and URNs; RapidAPI
headers only. No generic URL fetch. The shared transport supplies timeouts, size
limits, rate limiting and safe errors. 401/403/429/5xx and malformed HTTP-200 bodies
remain failures. No provider body or key is included in error envelopes.

Verification may consume one request. The existing credential upsert maintains one
key slot per user/provider (`API Builderz [rapidapi]`); reconnecting rotates it.
This is not LinkedIn identity authentication. No changes to shared auth, secrets,
OAuth, permissions or default-account resolution. Each action uses native Core RBAC.

## Live evidence — 30 October 2026

The authenticated RapidAPI playground was used with the user's existing BASIC plan,
within the authorized test scope. Eight requests were launched sequentially, no
subscription change or retry. No API key was extracted or persisted locally. No
raw personal profile or commenter dataset was saved in the repository.

| Request | Observed result |
| --- | --- |
| Company Updates, LinkedIn company, page 1 | HTTP 200, success, 10 posts; pagination start 0/count 10/total 232 |
| Reactions of first returned post, page 1 | HTTP 200, success; start 0/count 10/total 58 |
| Comments of same post, page 1 | HTTP 200, success; start 0/count 10/total 6 |
| Person Updates, provider's example profile, page 1 | HTTP 200, success; start 0/count 20/total 0 plus paginationToken |
| Same profile, page 2 with returned token | HTTP 200, success; start 20/count 20/total 0 plus a new token |
| Person Data, provider's example profile | HTTP 200, success; data object with firstName/lastName/fullName/headline |
| Comments, page 2 beyond reported final page | HTTP 502; gateway reported provider unreachable; not an empty success |
| Reactions, page 2 | HTTP 200, success; start 10/count 10/total 60 |

The final response headers reported a 50-request limit and **43 remaining**. Seven
successful requests were reflected in the quota; eight total requests were launched.
Do not assume future failed requests are free. Pricing page showed BASIC $0/month,
50 requests/month with hard limit; separate bandwidth overage applies above 10,240 MB.
These are observations at test time, not hardcoded product promises.

Source: https://rapidapi.com/apibuilderz/api/realtime-linkedin-bulk-data and its
playground endpoints. Initial route discovery came from the reference sales machine;
live tests corrected its interaction-ID assumptions and missing profile continuation.

## Validation and release boundary

154 runtime tests passed, including all existing providers, compiled worker dispatch,
wire headers/paths, continuation and invalid input/error cases. TypeScript passed;
generated artifact hashes passed `bun run check:artifacts`. Core's native catalog
integrity/tamper test passed with this provider included.

Live results above qualify sampled provider contracts through RapidAPI's playground.
They are not a live end-to-end connection through a running Core tenant. The generic
manifest still says `requires-live-verification`; tenant installation, secret storage,
revocation and user/agent permissions must be exercised in the target environment
before a production rollout. No release, tenant change, commit, push or deployment.
