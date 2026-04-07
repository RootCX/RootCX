# Core REST API — Integrations

Bind — base `/api/v1/apps/{app_id}/integrations`:

| Method | Path | Body | Response |
|--------|------|------|----------|
| GET | `/` | — | bindings list |
| POST | `/` | `{integrationId, config?}` | bind |
| PATCH | `/{integration_id}` | `{config}` | update config |
| DELETE | `/{integration_id}` | — | unbind |

Actions + auth — base `/api/v1/integrations`:

| Method | Path | Body | Response |
|--------|------|------|----------|
| POST | `/{integration_id}/actions/{action_id}` | action input | action result |
| GET | `/{integration_id}/auth` | — | `{connected,type}` |
| POST | `/{integration_id}/auth/start` | — | OAuth url or credential schema |
| POST | `/{integration_id}/auth/credentials` | `{field:value,...}` | — |
| DELETE | `/{integration_id}/auth` | — | disconnect |

**From a worker:** `POST {runtime_url}/api/v1/integrations/{integration_id}/actions/{action_id}` with `Authorization: Bearer {authToken}`, body = action input.
