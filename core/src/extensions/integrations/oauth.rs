//! Core-owned access token renewal for integration connections.
//!
//! A provider that rotates refresh tokens issues a new one on every renewal and
//! consumes the old. Handing the refresh token to a worker therefore loses it:
//! the worker renews, the provider rotates, and the new token dies with the call.
//! The Core renews instead and the worker only ever sees a short-lived access
//! token.
//!
//! Opt-in is `tokenUrl` in the integration's provider config, which only an admin
//! can populate (`save_platform_config`). The destination is deliberately not
//! taken from the manifest: app code authors manifests, so a manifest-supplied
//! URL would let the Core be told where to post its own secrets.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;

use crate::secrets::SecretManager;

/// Credential fields the Core reads and writes. `refreshToken` and `accessToken`
/// are already the convention across the bundled integrations, so nothing has to be
/// declared for the Core to find them.
///
/// Expiry is deliberately NOT `expiresAt`: an integration may already keep its own
/// under that name and in another unit (the bundled Outlook one uses milliseconds,
/// `outlook/index.ts:66`). Reading a foreign unit would make the Core judge a token
/// live for centuries and never renew it. The `_` prefix marks Core-owned, as
/// `_conn.` and `_delegate` already do elsewhere.
const F_REFRESH: &str = "refreshToken";
const F_ACCESS: &str = "accessToken";
const F_EXPIRES: &str = "_expiresAt";

/// Renew this far ahead of expiry rather than on the rejection it would cause.
const MARGIN_SECS: i64 = 300;
const TIMEOUT_MS: u64 = 15_000;

/// Whether a renewal failure means the grant is gone or merely that now is a bad
/// time. Only the first may kill the connection.
#[derive(Debug)]
pub(crate) enum RenewError {
    Rejected(String),
    Transient(String),
}

impl RenewError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Rejected(m) | Self::Transient(m) => m,
        }
    }

    /// The worker-facing envelope, so a renewal failure reads like any other
    /// credential failure to callers that already classify one.
    pub(crate) fn envelope(&self) -> JsonValue {
        let code = match self {
            Self::Rejected(_) => "unauthorized",
            Self::Transient(_) => "TEMPORARY_ERROR",
        };
        json!({ "ok": false, "error": { "code": code, "message": self.message() } })
    }
}

/// One in-flight renewal per connection. The Core is the only renewer, so an
/// in-process guard covers every worker; correctness does not rest on it, the
/// read-back fence below does.
static IN_FLIGHT: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

fn guard_for(connection_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut map = IN_FLIGHT.lock().unwrap();
    map.retain(|_, g| Arc::strong_count(g) > 1);
    map.entry(connection_id.to_string()).or_default().clone()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn str_field<'a>(v: &'a JsonValue, key: &str) -> Option<&'a str> {
    v.get(key).and_then(JsonValue::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// Present and not within the renewal margin.
fn is_live(blob: &JsonValue, now: i64) -> bool {
    str_field(blob, F_ACCESS).is_some()
        && blob.get(F_EXPIRES).and_then(JsonValue::as_i64).is_some_and(|exp| now < exp - MARGIN_SECS)
}

/// Config fields the Core acts on itself, so no worker may set them through
/// `__bind`'s `mergeConfig`. Only an admin writes these
/// (`save_platform_config`), which is what keeps the renewal destination out of
/// reach of application code.
pub(crate) const CORE_OWNED_CONFIG: [&str; 3] = ["tokenUrl", "clientId", "clientSecret"];

/// The Core renews for this connection only when an admin configured a token
/// endpoint and the credential actually carries a refresh token.
pub(crate) fn manages(config: &JsonValue, blob: &JsonValue) -> bool {
    str_field(config, "tokenUrl").is_some() && str_field(blob, F_REFRESH).is_some()
}

/// The credential as a worker may see it: everything except the refresh token.
///
/// Private on purpose. Renewing and stripping must not be separate steps a caller
/// can order wrongly or forget: `credentials_for_worker` is the only way in.
fn for_worker(blob: &JsonValue, config: &JsonValue) -> JsonValue {
    let mut out = blob.clone();
    if manages(config, blob) {
        out.as_object_mut().map(|map| map.remove(F_REFRESH));
    }
    out
}

/// The credential to hand a worker: a live access token, renewed if needed, and
/// never the refresh token.
///
/// Persists before returning, so a rotated refresh token is durable before the
/// access token is used. A failed write fails the call: continuing would spend an
/// access token whose refresh token the provider has already consumed and we did
/// not keep.
pub(crate) async fn credentials_for_worker(
    pool: &PgPool,
    secrets: &SecretManager,
    integration_id: &str,
    connection_id: &str,
    config: &JsonValue,
    blob: JsonValue,
) -> Result<JsonValue, RenewError> {
    if !manages(config, &blob) || is_live(&blob, now_secs()) {
        return Ok(for_worker(&blob, config));
    }

    let guard = guard_for(connection_id);
    let _held = guard.lock().await;

    // Read-back fence: whoever held the guard before us may have renewed already,
    // and presenting a consumed refresh token can cost the whole grant.
    //
    // Fails closed. Falling back to the caller's pre-guard snapshot on a read error
    // would present a token the previous holder already rotated, and the provider's
    // rejection would then flag a perfectly healthy connection dead.
    let blob = match reread(pool, secrets, integration_id, connection_id).await {
        Ok(Some(current)) => current,
        Ok(None) => return Err(RenewError::Transient("the credential is gone".into())),
        Err(e) => return Err(e),
    };
    if is_live(&blob, now_secs()) {
        return Ok(for_worker(&blob, config));
    }

    let refresh = str_field(&blob, F_REFRESH)
        .ok_or_else(|| RenewError::Rejected("connection has no refresh token".into()))?
        .to_string();
    // Flagged here, not at each caller: this is the only layer that knows both the
    // connection and whether the provider's answer was terminal.
    let renewed = match exchange(config, &refresh).await {
        Ok(renewed) => renewed,
        Err(e) => {
            if matches!(e, RenewError::Rejected(_)) {
                super::connections::flag_if_auth_failed(
                    pool, integration_id, Some(connection_id), &e.envelope(),
                ).await;
            }
            return Err(e);
        }
    };

    // Neither the reconnect nor the disconnect paths take our guard. If either
    // landed while we were at the provider, it wins: writing now would revert a
    // fresh credential, or resurrect one the owner just revoked. Read directly
    // rather than through `reread`, which cannot tell a vanished credential from a
    // failed read, and only the first means "revoked".
    let unchanged = reread(pool, secrets, integration_id, connection_id)
        .await?
        .is_some_and(|current| str_field(&current, F_REFRESH) == Some(&refresh));
    if !unchanged {
        return Err(RenewError::Transient("the credential changed while renewing".into()));
    }

    let merged = merge(&blob, renewed);

    secrets
        .set(pool, integration_id, &super::connections::credential_key(connection_id), &merged.to_string())
        .await
        .map_err(|e| RenewError::Transient(format!("persisting the credential failed: {e}")))?;

    Ok(for_worker(&merged, config))
}

/// The stored credential. `Ok(None)` means absent, which is revoked; an `Err`
/// means we could not tell, which is not the same thing and must not be conflated.
async fn reread(
    pool: &PgPool, secrets: &SecretManager, integration_id: &str, connection_id: &str,
) -> Result<Option<JsonValue>, RenewError> {
    let raw = secrets
        .get(pool, integration_id, &super::connections::credential_key(connection_id))
        .await
        .map_err(|e| RenewError::Transient(format!("re-reading the credential failed: {e}")))?;
    Ok(raw.and_then(|raw| serde_json::from_str(&raw).ok()))
}

struct Renewed {
    access: String,
    refresh: Option<String>,
    expires_at: i64,
}

/// Keep the previous refresh token when the provider omits one: a non-rotating
/// provider returns only an access token, and dropping the old one would
/// disconnect it.
fn merge(blob: &JsonValue, renewed: Renewed) -> JsonValue {
    let mut out = blob.clone();
    let map = match out.as_object_mut() {
        Some(m) => m,
        None => return out,
    };
    map.insert(F_ACCESS.into(), json!(renewed.access));
    map.insert(F_EXPIRES.into(), json!(renewed.expires_at));
    if let Some(r) = renewed.refresh {
        map.insert(F_REFRESH.into(), json!(r));
    }
    out
}

async fn exchange(config: &JsonValue, refresh_token: &str) -> Result<Renewed, RenewError> {
    let raw = str_field(config, "tokenUrl").ok_or_else(|| RenewError::Transient("no tokenUrl".into()))?;
    let url = reqwest::Url::parse(raw)
        .map_err(|e| RenewError::Transient(format!("invalid tokenUrl: {e}")))?;
    // A refresh token and a client secret go out in this body; cleartext is not an
    // option a deployment gets to choose. Relaxed only for the loopback test stub.
    #[cfg(not(test))]
    if url.scheme() != "https" {
        return Err(RenewError::Transient("tokenUrl must be https".into()));
    }
    let client = client_for(&url).await?;

    let mut form = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
    ];
    for key in ["clientId", "clientSecret"] {
        if let Some(v) = str_field(config, key) {
            let wire = if key == "clientId" { "client_id" } else { "client_secret" };
            form.push((wire, v.to_string()));
        }
    }

    let response = client
        .post(url)
        .form(&form)
        .send()
        .await
        .map_err(|e| RenewError::Transient(format!("token endpoint unreachable: {e}")))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    interpret(status.as_u16(), &body)
}

/// An HTTP client pinned to the addresses the destination resolved to, refusing
/// private, loopback and metadata networks. The Core runs in the tenant pod, so an
/// unvalidated destination here would reach further than app code ever can.
#[cfg(not(test))]
async fn client_for(url: &reqwest::Url) -> Result<reqwest::Client, RenewError> {
    let addrs = crate::tools::http_request::resolve_allowed_url(url)
        .await
        .map_err(RenewError::Transient)?;
    crate::tools::http_request::build_client(TIMEOUT_MS, url, &addrs).map_err(RenewError::Transient)
}

/// Tests drive a loopback stub, which the guard above exists to refuse. The bypass
/// is compiled out of every non-test build.
#[cfg(test)]
async fn client_for(_url: &reqwest::Url) -> Result<reqwest::Client, RenewError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(TIMEOUT_MS))
        .build()
        .map_err(|e| RenewError::Transient(e.to_string()))
}

/// The provider's answer, classified. Split from the network call so the rules that
/// decide "this grant is gone" can be tested without one.
fn interpret(status: u16, body: &str) -> Result<Renewed, RenewError> {
    if !(200..300).contains(&status) {
        let detail = describe(body);
        // A rejected grant will keep being rejected; anything else may not.
        return Err(if status == 401 || (status == 400 && detail.contains("invalid_grant")) {
            RenewError::Rejected(format!("the provider rejected the refresh token: {detail}"))
        } else {
            RenewError::Transient(format!("token endpoint returned {status}: {detail}"))
        });
    }

    let parsed: JsonValue = serde_json::from_str(body)
        .map_err(|e| RenewError::Transient(format!("token endpoint returned non-JSON: {e}")))?;
    let access = str_field(&parsed, "access_token")
        .ok_or_else(|| RenewError::Transient("token endpoint returned no access_token".into()))?
        .to_string();
    let lifetime = parsed.get("expires_in").and_then(JsonValue::as_i64).filter(|s| *s > 0).unwrap_or(3600);

    Ok(Renewed {
        access,
        refresh: str_field(&parsed, "refresh_token").map(str::to_string),
        expires_at: now_secs() + lifetime,
    })
}

/// The provider's own error wording, never the whole body: a token endpoint can
/// echo credentials back in an error.
fn describe(body: &str) -> String {
    serde_json::from_str::<JsonValue>(body)
        .ok()
        .and_then(|v| {
            ["error", "error_description", "ErrorType", "Message"]
                .iter()
                .filter_map(|k| str_field(&v, k).map(str::to_string))
                .reduce(|a, b| format!("{a}: {b}"))
        })
        .unwrap_or_else(|| "no detail".into())
}

#[cfg(test)]
mod stub {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub struct Provider {
        pub url: String,
        /// How many times the token endpoint was actually hit, which is the property
        /// the concurrency test asserts on.
        pub exchanges: Arc<AtomicUsize>,
    }

    /// A token endpoint that answers with `reply`, given the number of exchanges so
    /// far so a rotating provider can hand back a distinct pair each time.
    async fn serve(delay_ms: u64, reply: fn(usize) -> (u16, String)) -> Provider {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let exchanges = Arc::new(AtomicUsize::new(0));
        let counter = exchanges.clone();

        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let counter = counter.clone();
                tokio::spawn(async move {
                    let mut request = [0u8; 2048];
                    let _ = sock.read(&mut request).await;
                    let nth = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    // Held open so concurrent callers genuinely overlap.
                    if delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    let (status, body) = reply(nth);
                    let reason = if status == 200 { "OK" } else { "Error" };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body,
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });

        Provider { url: format!("http://127.0.0.1:{port}/token"), exchanges }
    }

    /// Single-use refresh tokens: every renewal returns a new pair.
    pub async fn rotating(delay_ms: u64) -> Provider {
        serve(delay_ms, |nth| {
            (200, format!(r#"{{"access_token":"access-{nth}","refresh_token":"rotated-{nth}","expires_in":3600}}"#))
        }).await
    }

    /// A revoked grant: what a rotating provider says once a token is reused.
    pub async fn rejecting() -> Provider {
        serve(0, |_| (400, r#"{"error":"invalid_grant"}"#.to_string())).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> JsonValue { json!({ "tokenUrl": "https://p.example.com/token" }) }

    #[test]
    fn manages_requires_both_an_endpoint_and_a_refresh_token() {
        let with_token = json!({ "refreshToken": "r1" });
        assert!(manages(&cfg(), &with_token), "configured endpoint plus a refresh token");
        assert!(!manages(&json!({}), &with_token), "no endpoint configured: leave it to the worker");
        assert!(!manages(&cfg(), &json!({ "username": "u" })), "nothing to renew with");
        assert!(!manages(&cfg(), &json!({ "refreshToken": "  " })), "blank is absent");
    }

    /// The Core must not read an integration's own expiry field. The bundled Outlook
    /// integration keeps one named `expiresAt` in milliseconds, which read as seconds
    /// would place the token centuries ahead and suppress renewal forever.
    #[test]
    fn an_integrations_own_expiry_field_is_never_read_as_the_cores() {
        let now = 1_000_000;
        let foreign_millis = json!({ "accessToken": "a", "expiresAt": now as i64 * 1000 });
        assert!(!is_live(&foreign_millis, now),
            "a foreign expiry must leave the token due for renewal, not live forever");
    }

    #[test]
    fn liveness_respects_the_renewal_margin() {
        let now = 1_000_000;
        let live = json!({ "accessToken": "a", "_expiresAt": now + MARGIN_SECS + 1 });
        assert!(is_live(&live, now));
        assert!(!is_live(&json!({ "accessToken": "a", "_expiresAt": now + MARGIN_SECS }), now),
            "inside the margin counts as expired, so renewal never races the provider");
        assert!(!is_live(&json!({ "accessToken": "a" }), now), "no expiry known");
        assert!(!is_live(&json!({ "_expiresAt": now + 99_999 }), now), "no token");
    }

    #[test]
    fn merge_keeps_the_stored_refresh_token_unless_the_provider_rotated() {
        // (case, what the provider returned, which token must be stored after)
        let cases: &[(&str, Option<&str>, &str)] = &[
            ("a rotating provider replaces it", Some("r2"), "r2"),
            ("a non-rotating provider must not be disconnected", None, "r1"),
            ("a blank rotation is not a rotation", Some("   "), "r1"),
        ];
        for (case, returned, expected) in cases {
            let blob = json!({ "refreshToken": "r1", "apiBase": "https://x.example.com" });
            let renewed = Renewed {
                access: "a2".into(),
                refresh: returned.map(str::to_string).filter(|r| !r.trim().is_empty()),
                expires_at: 42,
            };
            let out = merge(&blob, renewed);
            assert_eq!(out["refreshToken"], *expected, "case: {case}");
            assert_eq!(out["accessToken"], "a2", "case: {case}");
            assert_eq!(out["_expiresAt"], 42, "case: {case}");
            assert_eq!(out["apiBase"], "https://x.example.com", "unrelated fields survive: {case}");
        }
    }

    #[test]
    fn the_worker_never_receives_the_refresh_token() {
        let blob = json!({ "refreshToken": "r1", "accessToken": "a1", "apiBase": "https://x" });
        let out = for_worker(&blob, &cfg());
        assert!(out.get("refreshToken").is_none(), "the field the Core exists to withhold");
        assert_eq!(out["accessToken"], "a1");
        assert_eq!(out["apiBase"], "https://x");
    }

    #[test]
    fn an_unmanaged_connection_is_passed_through_untouched() {
        // No configured endpoint: the integration still renews for itself, so
        // withholding the refresh token would break it.
        let blob = json!({ "refreshToken": "r1" });
        assert_eq!(for_worker(&blob, &json!({})), blob);
    }

    /// Asserted against the real classifier, not against the literal codes: the
    /// contract is "a rejected grant reaches dead-flagging", and a rename on either
    /// side would otherwise pass both files' tests while silently breaking it.
    #[test]
    fn only_a_rejected_grant_reaches_the_dead_flagging_classifier() {
        use super::super::connections::auth_failure_message;

        let rejected = RenewError::Rejected("gone".into()).envelope();
        assert_eq!(auth_failure_message(&rejected).as_deref(), Some("gone"));

        let transient = RenewError::Transient("later".into()).envelope();
        assert_eq!(auth_failure_message(&transient), None, "a bad moment must not kill the connection");
    }

    #[test]
    fn a_successful_response_is_read_field_by_field() {
        // (case, body, expected rotated token, expected minimum lifetime)
        let cases: &[(&str, &str, Option<&str>, i64)] = &[
            ("a rotating provider returns a new pair",
                r#"{"access_token":"a2","refresh_token":"r2","expires_in":3600}"#, Some("r2"), 3600),
            ("a non-rotating provider returns only an access token",
                r#"{"access_token":"a2","expires_in":60}"#, None, 60),
            ("a missing lifetime falls back to an hour",
                r#"{"access_token":"a2"}"#, None, 3600),
            ("a zero lifetime would renew on every call, so it falls back too",
                r#"{"access_token":"a2","expires_in":0}"#, None, 3600),
        ];
        for (case, body, rotated, min_lifetime) in cases {
            let r = interpret(200, body).unwrap_or_else(|e| panic!("case: {case}: {e:?}"));
            assert_eq!(r.access, "a2", "case: {case}");
            assert_eq!(r.refresh.as_deref(), *rotated, "case: {case}");
            // Absolute, not relative: a restart must still be able to judge it.
            assert!(r.expires_at >= now_secs() + min_lifetime, "case: {case}");
        }
    }

    /// The distinction the whole design rests on: a dead grant must not be retried
    /// forever, and a provider outage must not disconnect a working mailbox.
    #[test]
    fn only_a_rejected_grant_is_terminal() {
        let cases: &[(&str, u16, &str, bool)] = &[
            ("invalid_grant on 400 is terminal", 400, r#"{"error":"invalid_grant"}"#, true),
            ("401 is terminal whatever the body", 401, "", true),
            ("another 400 is not terminal", 400, r#"{"error":"invalid_request"}"#, false),
            ("429 is not terminal", 429, "", false),
            ("500 is not terminal", 500, "", false),
            ("200 with no access token is not terminal", 200, r#"{"scope":"x"}"#, false),
            ("200 with a non-JSON body is not terminal", 200, "<html/>", false),
        ];
        for (case, status, body, terminal) in cases {
            let err = interpret(*status, body).err().expect(case);
            assert_eq!(matches!(err, RenewError::Rejected(_)), *terminal, "case: {case}");
        }
    }

    #[tokio::test]
    async fn one_guard_per_connection_and_the_map_does_not_grow_unbounded() {
        let a = guard_for("conn-a");
        assert!(Arc::ptr_eq(&a, &guard_for("conn-a")), "concurrent callers must share one guard");
        assert!(!Arc::ptr_eq(&a, &guard_for("conn-b")), "distinct connections must not serialise");
        drop(a);
        let _ = guard_for("conn-c");
        assert!(!IN_FLIGHT.lock().unwrap().contains_key("conn-a"), "released guards are reaped");
    }

    mod against_a_provider {
        use super::super::super::connections;
        use super::super::stub;
        use super::*;
        use std::sync::atomic::Ordering;

        const APP: &str = "_oauth_test";
        const OWNER: &str = "00000000-0000-0000-0000-000000000001";

        /// Bootstrapped once per test binary. Running it per test made several
        /// bootstraps race on the same catalog rows ("tuple concurrently updated"),
        /// which is a harness problem, not a product one.
        static SCHEMA: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

        /// A connection as the Core finds one: a live row, a stored credential, and
        /// the provider config that decides whether the Core renews at all.
        struct Connected {
            pool: PgPool,
            secrets: SecretManager,
            id: String,
            config: JsonValue,
        }

        impl Connected {
            async fn with(config: JsonValue, credential: JsonValue) -> Self {
                let url = std::env::var("TEST_DATABASE_URL")
                    .unwrap_or_else(|_| "postgres://rootcx:rootcx@localhost:5480/rootcx".into());
                let pool = PgPool::connect(&url).await.expect("connect to test DB");
                SCHEMA.get_or_init(|| async {
                    sqlx::query("CREATE SCHEMA IF NOT EXISTS rootcx_system").execute(&pool).await.unwrap();
                    crate::secrets::bootstrap_secrets_schema(&pool).await.unwrap();
                    connections::bootstrap(&pool).await.unwrap();
                }).await;

                let secrets = SecretManager::with_key(&[0x5A; 32]);
                let id = uuid::Uuid::new_v4().to_string();
                sqlx::query(
                    "INSERT INTO rootcx_system.integration_connections (id, integration_id, user_id, kind, status)
                     VALUES ($1, $2, $3, 'direct', 'active')",
                ).bind(&id).bind(APP).bind(OWNER).execute(&pool).await.unwrap();
                secrets.set(&pool, APP, &connections::credential_key(&id), &credential.to_string())
                    .await.unwrap();

                Self { pool, secrets, id, config }
            }

            /// The stored credential, read the way a fresh process reads it.
            async fn credential(&self) -> JsonValue {
                reread(&self.pool, &self.secrets, APP, &self.id).await.unwrap().expect("credential must be stored")
            }

            /// Exactly `credentials_for_worker`, over the stored credential.
            async fn resolve(&self) -> Result<JsonValue, RenewError> {
                let credential = self.credential().await;
                credentials_for_worker(&self.pool, &self.secrets, APP, &self.id, &self.config, credential).await
            }

            async fn status(&self) -> String {
                sqlx::query_scalar("SELECT status FROM rootcx_system.integration_connections WHERE id = $1")
                    .bind(&self.id).fetch_one(&self.pool).await.unwrap()
            }
        }

        fn config_for(provider: &stub::Provider) -> JsonValue {
            json!({ "tokenUrl": provider.url, "clientId": "cid", "clientSecret": "csec" })
        }

        /// The defect this module exists for: the provider consumes the old refresh
        /// token, so the new one must be durable before the call proceeds.
        #[tokio::test]
        async fn a_rotated_refresh_token_is_persisted_before_use() {
            let provider = stub::rotating(0).await;
            let c = Connected::with(
                config_for(&provider),
                json!({ "refreshToken": "original", "apiBase": "https://x" }),
            ).await;

            let out = c.resolve().await.unwrap();

            assert_eq!(c.credential().await["refreshToken"], "rotated-1",
                "the store holds the new token, which is what survives the call");
            assert_eq!(out["accessToken"], "access-1");
            assert_eq!(out["apiBase"], "https://x", "unrelated credential fields are preserved");
            assert!(out.get("refreshToken").is_none(),
                "the worker sees the access token, never the refresh token");
        }

        /// A second call must not spend the token the first one already rotated, and
        /// re-reading from the store is exactly what a fresh process does.
        #[tokio::test]
        async fn a_live_access_token_is_reused_and_survives_a_restart() {
            let provider = stub::rotating(0).await;
            let c = Connected::with(config_for(&provider), json!({ "refreshToken": "original" })).await;

            c.resolve().await.unwrap();
            assert_eq!(provider.exchanges.load(Ordering::SeqCst), 1);

            let out = c.resolve().await.unwrap();

            assert_eq!(provider.exchanges.load(Ordering::SeqCst), 1,
                "a still-live access token must not cost another rotation");
            assert_eq!(out["accessToken"], "access-1");
        }

        /// Two callers racing to renew would each present the same single-use token;
        /// one wins and the other burns the grant.
        #[tokio::test]
        async fn concurrent_calls_on_one_connection_cause_exactly_one_exchange() {
            let provider = stub::rotating(120).await;
            let c = Connected::with(config_for(&provider), json!({ "refreshToken": "original" })).await;

            let mut set = tokio::task::JoinSet::new();
            for _ in 0..8 {
                let (pool, secrets, id, config) =
                    (c.pool.clone(), SecretManager::with_key(&[0x5A; 32]), c.id.clone(), c.config.clone());
                set.spawn(async move {
                    let credential = reread(&pool, &secrets, APP, &id).await.unwrap().unwrap();
                    credentials_for_worker(&pool, &secrets, APP, &id, &config, credential)
                        .await
                        .map(|out| out["accessToken"].clone())
                });
            }
            let mut tokens = Vec::new();
            while let Some(joined) = set.join_next().await {
                tokens.push(joined.unwrap().expect("every concurrent caller must succeed"));
            }

            assert_eq!(provider.exchanges.load(Ordering::SeqCst), 1,
                "the guard plus the read-back fence must collapse the race to one rotation");
            assert!(tokens.windows(2).all(|w| w[0] == w[1]), "all callers share one token: {tokens:?}");
            assert_eq!(c.credential().await["refreshToken"], "rotated-1");
        }

        /// The path the feature turns on when a grant dies. Classification is
        /// unit-tested elsewhere; this proves the state transition it drives happens.
        #[tokio::test]
        async fn a_rejected_grant_flags_the_connection_dead() {
            let provider = stub::rejecting().await;
            let c = Connected::with(config_for(&provider), json!({ "refreshToken": "already-consumed" })).await;

            let err = c.resolve().await.expect_err("a revoked grant must not resolve");

            assert!(matches!(err, RenewError::Rejected(_)), "got: {err:?}");
            assert_eq!(c.status().await, "dead",
                "a revoked connection that still reads 'active' keeps being auto-selected");
        }

        /// The mirror of the above, and why `forbidden` is excluded from the
        /// dead-flagging set: an outage must not disconnect a mailbox that works.
        #[tokio::test]
        async fn a_transient_failure_leaves_the_connection_active() {
            // Nothing listens on port 1, so the exchange cannot connect.
            let c = Connected::with(
                json!({ "tokenUrl": "http://127.0.0.1:1/token" }),
                json!({ "refreshToken": "still-good" }),
            ).await;

            let err = c.resolve().await.expect_err("an unreachable provider must fail the call");

            assert!(matches!(err, RenewError::Transient(_)), "got: {err:?}");
            assert_eq!(c.status().await, "active", "an outage must not cost a reconnection");
        }

        /// A read that fails is not a read that found nothing. Falling back to the
        /// caller's pre-guard snapshot would present a token a concurrent renewal had
        /// already rotated, and the provider's rejection would kill a healthy
        /// connection.
        #[tokio::test]
        async fn a_failed_read_does_not_fall_back_to_a_stale_snapshot() {
            let provider = stub::rotating(0).await;
            let c = Connected::with(config_for(&provider), json!({ "refreshToken": "original" })).await;
            let stale = c.credential().await;

            c.pool.close().await;
            let err = credentials_for_worker(&c.pool, &c.secrets, APP, &c.id, &c.config, stale)
                .await
                .expect_err("an unreadable store must fail the call");

            assert!(matches!(err, RenewError::Transient(_)), "got: {err:?}");
            assert_eq!(provider.exchanges.load(Ordering::SeqCst), 0,
                "no token may be spent on a snapshot we could not confirm");
        }

        /// A disconnect landing mid-renewal must not be undone. Writing the merged
        /// credential back would recreate access the owner just revoked, and every
        /// later renewal would keep it alive.
        #[tokio::test]
        async fn a_credential_revoked_mid_renewal_is_not_resurrected() {
            let provider = stub::rotating(150).await;
            let c = Connected::with(config_for(&provider), json!({ "refreshToken": "original" })).await;
            let credential = c.credential().await;

            let renewing = {
                let (pool, secrets, id, config) = (
                    c.pool.clone(), SecretManager::with_key(&[0x5A; 32]), c.id.clone(), c.config.clone(),
                );
                tokio::spawn(async move {
                    credentials_for_worker(&pool, &secrets, APP, &id, &config, credential).await
                })
            };
            // Delete only once the exchange is provably in flight; a fixed sleep would
            // race the renewal on a loaded machine and pass for the wrong reason.
            while provider.exchanges.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            c.secrets.delete(&c.pool, APP, &connections::credential_key(&c.id)).await.unwrap();

            let err = renewing.await.unwrap().expect_err("a revoked credential must not resolve");

            assert!(matches!(err, RenewError::Transient(_)), "got: {err:?}");
            assert!(reread(&c.pool, &c.secrets, APP, &c.id).await.unwrap().is_none(),
                "the revoked credential must stay gone");
        }
    }

    #[test]
    fn describe_reports_the_provider_wording_and_never_the_raw_body() {
        assert!(describe(r#"{"error":"invalid_grant"}"#).contains("invalid_grant"));
        assert_eq!(describe("<html>secret=abc</html>"), "no detail");
        assert_eq!(describe(r#"{"unknown":1}"#), "no detail");
    }
}
