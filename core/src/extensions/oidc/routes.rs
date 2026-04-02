use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use openidconnect::core::{CoreIdToken, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    NonceVerifier, PkceCodeChallenge, PkceCodeVerifier, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::auth::{AuthConfig, jwt};
use crate::extensions::rbac::policy::require_admin;
use crate::routes::{self, SharedRuntime};
use crate::secrets::SecretManager;

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("failed to build reqwest client")
}

// ── Provider CRUD ──────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderPublic {
    id: String,
    display_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpsertProviderRequest {
    id: String,
    display_name: String,
    issuer_url: String,
    client_id: String,
    client_secret: Option<String>,
    #[serde(default = "default_scopes")]
    scopes: Vec<String>,
    #[serde(default = "default_true")]
    auto_register: bool,
    #[serde(default = "default_role")]
    default_role: String,
    role_claim: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_scopes() -> Vec<String> { vec!["openid".into(), "email".into(), "profile".into()] }
fn default_true() -> bool { true }
fn default_role() -> String { "admin".into() }

/// GET /api/v1/auth/oidc/providers — public, for login screen
pub(crate) async fn list_providers(
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<ProviderPublic>>, ApiError> {
    let pool = routes::pool(&rt);
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, display_name FROM rootcx_system.oidc_providers WHERE enabled = true ORDER BY id",
    )
    .fetch_all(&pool)
    .await?;

    Ok(Json(rows.into_iter().map(|(id, display_name)| ProviderPublic { id, display_name }).collect()))
}

/// POST /api/v1/auth/oidc/providers — admin only
pub(crate) async fn upsert_provider(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Json(req): Json<UpsertProviderRequest>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    require_admin(&pool, identity.user_id).await?;

    if req.id.is_empty() { return Err(ApiError::BadRequest("id required".into())); }
    if req.issuer_url.is_empty() { return Err(ApiError::BadRequest("issuer_url required".into())); }
    if req.client_id.is_empty() { return Err(ApiError::BadRequest("client_id required".into())); }

    // Validate issuer URL format and enforce HTTPS (except localhost)
    let parsed = url::Url::parse(&req.issuer_url)
        .map_err(|_| ApiError::BadRequest("invalid issuer_url".into()))?;
    let host = parsed.host_str().unwrap_or("");
    if parsed.scheme() != "https" && host != "localhost" && host != "127.0.0.1" {
        return Err(ApiError::BadRequest("issuer_url must be HTTPS (except localhost)".into()));
    }

    // Try to fetch discovery document to validate
    let issuer = IssuerUrl::new(req.issuer_url.clone())
        .map_err(|e| ApiError::BadRequest(format!("invalid issuer_url: {e}")))?;
    let client = http_client();
    CoreProviderMetadata::discover_async(issuer, &client)
        .await
        .map_err(|e| ApiError::BadRequest(format!("failed to fetch discovery document: {e}")))?;

    // Encrypt client_secret via vault if provided
    if let Some(ref secret) = req.client_secret {
        secrets.set(&pool, &format!("oidc:{}", req.id), "client_secret", secret).await
            .map_err(|e| ApiError::Internal(format!("failed to encrypt client_secret: {e}")))?;
    }

    let scopes_ref: Vec<&str> = req.scopes.iter().map(|s| s.as_str()).collect();

    // Store NULL for client_secret in the row — actual secret is in the vault
    sqlx::query(
        "INSERT INTO rootcx_system.oidc_providers
            (id, display_name, issuer_url, client_id, client_secret, scopes, auto_register, default_role, role_claim, enabled)
         VALUES ($1, $2, $3, $4, NULL, $5, $6, $7, $8, $9)
         ON CONFLICT (id) DO UPDATE SET
            display_name = EXCLUDED.display_name,
            issuer_url = EXCLUDED.issuer_url,
            client_id = EXCLUDED.client_id,
            scopes = EXCLUDED.scopes,
            auto_register = EXCLUDED.auto_register,
            default_role = EXCLUDED.default_role,
            role_claim = EXCLUDED.role_claim,
            enabled = EXCLUDED.enabled",
    )
    .bind(&req.id)
    .bind(&req.display_name)
    .bind(&req.issuer_url)
    .bind(&req.client_id)
    .bind(&scopes_ref)
    .bind(req.auto_register)
    .bind(&req.default_role)
    .bind(&req.role_claim)
    .bind(req.enabled)
    .execute(&pool)
    .await?;

    Ok((StatusCode::OK, Json(json!({ "message": format!("provider '{}' saved", req.id) }))))
}

/// DELETE /api/v1/auth/oidc/providers/{id} — admin only
pub(crate) async fn delete_provider(
    State(rt): State<SharedRuntime>,
    axum::extract::Path(id): axum::extract::Path<String>,
    identity: Identity,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    require_admin(&pool, identity.user_id).await?;

    let force = params.get("force").is_some_and(|v| v == "true");

    if !force {
        let (linked,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM rootcx_system.users WHERE oidc_provider = $1",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await?;

        if linked > 0 {
            return Err(ApiError::BadRequest(format!(
                "{linked} user(s) are linked to this provider. Use ?force=true to delete anyway."
            )));
        }
    }

    let r = sqlx::query("DELETE FROM rootcx_system.oidc_providers WHERE id = $1")
        .bind(&id)
        .execute(&pool)
        .await?;

    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("provider '{id}' not found")));
    }

    // Clean up encrypted secret from vault
    let _ = secrets.delete(&pool, &format!("oidc:{id}"), "client_secret").await;

    Ok(Json(json!({ "message": format!("provider '{id}' deleted") })))
}

// ── Token Exchange (server-to-server) ──────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TokenExchangeRequest {
    provider_id: String,
    id_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TokenExchangeResponse {
    access_token: String,
    expires_in: i64,
}

/// Nonce verifier that skips nonce validation (for server-to-server token exchange).
struct SkipNonce;
impl NonceVerifier for SkipNonce {
    fn verify(self, _nonce: Option<&Nonce>) -> Result<(), String> {
        Ok(())
    }
}

/// POST /api/v1/auth/oidc/token-exchange
pub(crate) async fn token_exchange(
    State(rt): State<SharedRuntime>,
    axum::Extension(auth_config): axum::Extension<Arc<AuthConfig>>,
    Json(req): Json<TokenExchangeRequest>,
) -> Result<Json<TokenExchangeResponse>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    let provider = load_provider(&pool, &secrets, &req.provider_id).await?;

    let issuer = IssuerUrl::new(provider.issuer_url.clone())
        .map_err(|e| ApiError::Internal(format!("invalid issuer_url: {e}")))?;
    let client = http_client();
    let metadata = CoreProviderMetadata::discover_async(issuer, &client)
        .await
        .map_err(|e| ApiError::Internal(format!("discovery failed: {e}")))?;

    let oidc_client = build_oidc_client(&provider, metadata);
    let verifier = oidc_client.id_token_verifier();

    // Parse the raw id_token JWT
    let id_token: CoreIdToken = req.id_token.parse()
        .map_err(|e: serde_json::Error| ApiError::Unauthorized(format!("invalid id_token: {e}")))?;

    // Validate (no nonce check for server-to-server)
    let claims = id_token
        .claims(&verifier, SkipNonce)
        .map_err(|e| ApiError::Unauthorized(format!("id_token validation failed: {e}")))?;

    let sub = claims.subject().to_string();
    let email = claims
        .email()
        .map(|e: &openidconnect::EndUserEmail| e.to_string())
        .ok_or_else(|| ApiError::Unauthorized("id_token missing email claim".into()))?;
    let name = claims
        .preferred_username()
        .map(|n: &openidconnect::EndUserUsername| n.to_string())
        .or_else(|| {
            claims.name()
                .and_then(|n: &openidconnect::LocalizedClaim<openidconnect::EndUserName>| {
                    n.get(None).map(|v: &openidconnect::EndUserName| v.to_string())
                })
        });

    let role = extract_role_claim(&req.id_token, &provider)?;
    let user_id = find_or_create_user(&pool, &provider, &sub, &email, name.as_deref(), role.as_deref()).await?;

    let access_token = jwt::encode_access(&auth_config, user_id, &email)?;

    Ok(Json(TokenExchangeResponse {
        access_token,
        expires_in: auth_config.access_ttl.as_secs() as i64,
    }))
}

// ── Browser Flow (authorize + callback) ────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct AuthorizeParams {
    redirect_uri: Option<String>,
}

/// GET /api/v1/auth/oidc/{provider_id}/authorize
pub(crate) async fn authorize(
    State(rt): State<SharedRuntime>,
    axum::extract::Path(provider_id): axum::extract::Path<String>,
    Query(params): Query<AuthorizeParams>,
) -> Result<Response, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    let provider = load_provider(&pool, &secrets, &provider_id).await?;

    let redirect_uri = params.redirect_uri.unwrap_or_default();

    let issuer = IssuerUrl::new(provider.issuer_url.clone())
        .map_err(|e| ApiError::Internal(format!("invalid issuer_url: {e}")))?;
    let client = http_client();
    let metadata = CoreProviderMetadata::discover_async(issuer, &client)
        .await
        .map_err(|e| ApiError::Internal(format!("discovery failed: {e}")))?;

    let oidc_client = build_oidc_client(&provider, metadata);

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let mut auth_req = oidc_client.authorize_url(
        openidconnect::AuthenticationFlow::<openidconnect::core::CoreResponseType>::AuthorizationCode,
        CsrfToken::new_random,
        Nonce::new_random,
    );

    for scope in &provider.scopes {
        if scope != "openid" {
            auth_req = auth_req.add_scope(Scope::new(scope.clone()));
        }
    }

    let (auth_url, state, nonce) = auth_req
        .set_pkce_challenge(pkce_challenge)
        .url();

    sqlx::query(
        "INSERT INTO rootcx_system.oidc_state (state, provider_id, nonce, pkce_verifier, redirect_uri)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(state.secret())
    .bind(&provider_id)
    .bind(nonce.secret())
    .bind(pkce_verifier.secret())
    .bind(&redirect_uri)
    .execute(&pool)
    .await?;

    Ok(Redirect::temporary(auth_url.as_str()).into_response())
}

#[derive(Deserialize)]
pub(crate) struct CallbackParams {
    code: String,
    state: String,
}

/// GET /api/v1/auth/oidc/callback
pub(crate) async fn callback(
    State(rt): State<SharedRuntime>,
    axum::Extension(auth_config): axum::Extension<Arc<AuthConfig>>,
    Query(params): Query<CallbackParams>,
) -> Result<Response, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);

    // Look up and delete the state (single-use)
    let state_row: Option<(String, String, String, String, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "DELETE FROM rootcx_system.oidc_state
             WHERE state = $1
             RETURNING provider_id, nonce, pkce_verifier, redirect_uri, created_at",
        )
        .bind(&params.state)
        .fetch_optional(&pool)
        .await?;

    let (provider_id, nonce_str, pkce_verifier_str, client_redirect_uri, created_at) =
        state_row.ok_or_else(|| ApiError::Unauthorized("invalid or expired state".into()))?;

    if chrono::Utc::now() - created_at > chrono::Duration::minutes(10) {
        return Err(ApiError::Unauthorized("state expired".into()));
    }

    let provider = load_provider(&pool, &secrets, &provider_id).await?;

    let issuer = IssuerUrl::new(provider.issuer_url.clone())
        .map_err(|e| ApiError::Internal(format!("invalid issuer_url: {e}")))?;
    let client = http_client();
    let metadata = CoreProviderMetadata::discover_async(issuer, &client)
        .await
        .map_err(|e| ApiError::Internal(format!("discovery failed: {e}")))?;

    let oidc_client = build_oidc_client(&provider, metadata);

    // Exchange code for tokens
    let token_response = oidc_client
        .exchange_code(AuthorizationCode::new(params.code))
        .unwrap_or_else(|_| unreachable!()) // token endpoint is set from provider metadata
        .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier_str))
        .request_async(&client)
        .await
        .map_err(|e| ApiError::Unauthorized(format!("token exchange failed: {e}")))?;

    // Verify id_token
    let id_token = token_response
        .id_token()
        .ok_or_else(|| ApiError::Unauthorized("no id_token in response".into()))?;

    let nonce = Nonce::new(nonce_str);
    let verifier = oidc_client.id_token_verifier();
    let claims = id_token
        .claims(&verifier, &nonce)
        .map_err(|e| ApiError::Unauthorized(format!("id_token validation failed: {e}")))?;

    let sub = claims.subject().to_string();
    let email = claims
        .email()
        .map(|e: &openidconnect::EndUserEmail| e.to_string())
        .ok_or_else(|| ApiError::Unauthorized("id_token missing email claim".into()))?;
    let name = claims
        .preferred_username()
        .map(|n: &openidconnect::EndUserUsername| n.to_string())
        .or_else(|| {
            claims.name()
                .and_then(|n: &openidconnect::LocalizedClaim<openidconnect::EndUserName>| {
                    n.get(None).map(|v: &openidconnect::EndUserName| v.to_string())
                })
        });

    let raw_token = id_token.to_string();
    let role = extract_role_claim(&raw_token, &provider)?;

    let user_id = find_or_create_user(&pool, &provider, &sub, &email, name.as_deref(), role.as_deref()).await?;

    // Issue Core JWT (access + refresh)
    let session_id = Uuid::new_v4();
    let expires_at = chrono::Utc::now() + auth_config.refresh_ttl;
    sqlx::query("INSERT INTO rootcx_system.sessions (id, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(session_id)
        .bind(user_id)
        .bind(expires_at)
        .execute(&pool)
        .await?;

    let access_token = jwt::encode_access(&auth_config, user_id, &email)?;
    let refresh_token = jwt::encode_refresh(&auth_config, user_id, session_id)?;

    if client_redirect_uri.is_empty() {
        return Ok(Json(json!({
            "accessToken": access_token,
            "refreshToken": refresh_token,
            "expiresIn": auth_config.access_ttl.as_secs(),
        }))
        .into_response());
    }

    let mut redirect_url = url::Url::parse(&client_redirect_uri)
        .map_err(|_| ApiError::Internal("invalid redirect_uri".into()))?;
    redirect_url.query_pairs_mut()
        .append_pair("access_token", &access_token)
        .append_pair("refresh_token", &refresh_token)
        .append_pair("expires_in", &auth_config.access_ttl.as_secs().to_string());

    Ok(Redirect::temporary(redirect_url.as_str()).into_response())
}

// ── Helpers ────────────────────────────────────────────────────────────────

struct ProviderRow {
    id: String,
    issuer_url: String,
    client_id: String,
    client_secret: Option<String>,
    scopes: Vec<String>,
    auto_register: bool,
    default_role: String,
    role_claim: Option<String>,
}

async fn load_provider(pool: &sqlx::PgPool, secrets: &SecretManager, id: &str) -> Result<ProviderRow, ApiError> {
    let row: Option<(String, String, String, Vec<String>, bool, String, Option<String>)> =
        sqlx::query_as(
            "SELECT id, issuer_url, client_id, scopes, auto_register, default_role, role_claim
             FROM rootcx_system.oidc_providers WHERE id = $1 AND enabled = true",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;

    let (id, issuer_url, client_id, scopes, auto_register, default_role, role_claim) =
        row.ok_or_else(|| ApiError::NotFound(format!("OIDC provider '{id}' not found or disabled")))?;

    // Decrypt client_secret from vault
    let client_secret = secrets.get(pool, &format!("oidc:{id}"), "client_secret").await
        .map_err(|e| ApiError::Internal(format!("failed to decrypt client_secret: {e}")))?;

    Ok(ProviderRow { id, issuer_url, client_id, client_secret, scopes, auto_register, default_role, role_claim })
}

fn core_public_url() -> String {
    std::env::var("ROOTCX_PUBLIC_URL")
        .or_else(|_| std::env::var("ROOTCX_URL"))
        .unwrap_or_else(|_| "http://localhost:9100".to_string())
}

fn build_oidc_client(
    provider: &ProviderRow,
    metadata: CoreProviderMetadata,
) -> openidconnect::core::CoreClient<
    openidconnect::EndpointSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointMaybeSet,
    openidconnect::EndpointMaybeSet,
> {
    let client_id = ClientId::new(provider.client_id.clone());
    let client_secret = provider.client_secret.as_ref().map(|s| ClientSecret::new(s.clone()));
    let callback_url = format!("{}/api/v1/auth/oidc/callback", core_public_url());
    openidconnect::core::CoreClient::from_provider_metadata(metadata, client_id, client_secret)
        .set_redirect_uri(openidconnect::RedirectUrl::new(callback_url).expect("invalid callback URL"))
}

/// Extract a custom role claim from the id_token JWT payload.
fn extract_role_claim(raw_token: &str, provider: &ProviderRow) -> Result<Option<String>, ApiError> {
    let claim_name = match &provider.role_claim {
        Some(c) if !c.is_empty() => c,
        _ => return Ok(None),
    };

    let parts: Vec<&str> = raw_token.split('.').collect();
    if parts.len() < 2 {
        return Ok(None);
    }

    use base64::Engine;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| ApiError::Internal("failed to decode id_token payload".into()))?;

    let claims: serde_json::Value = serde_json::from_slice(&payload)
        .map_err(|_| ApiError::Internal("failed to parse id_token payload".into()))?;

    Ok(claims.get(claim_name).and_then(|v| v.as_str()).map(|s| s.to_string()))
}

/// Find or create a Core user from OIDC claims.
async fn find_or_create_user(
    pool: &sqlx::PgPool,
    provider: &ProviderRow,
    sub: &str,
    email: &str,
    display_name: Option<&str>,
    role: Option<&str>,
) -> Result<Uuid, ApiError> {
    // 1. Try by oidc_provider + oidc_sub (stable identifier)
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM rootcx_system.users WHERE oidc_provider = $1 AND oidc_sub = $2",
    )
    .bind(&provider.id)
    .bind(sub)
    .fetch_optional(pool)
    .await?;

    if let Some((user_id,)) = existing {
        sqlx::query(
            "UPDATE rootcx_system.users SET email = $1, display_name = COALESCE($2, display_name), updated_at = now()
             WHERE id = $3",
        )
        .bind(email)
        .bind(display_name)
        .bind(user_id)
        .execute(pool)
        .await?;

        return Ok(user_id);
    }

    // 2. Try linking existing user by email (one-time migration from password to OIDC)
    let by_email: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM rootcx_system.users WHERE email = $1 AND oidc_provider IS NULL",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?;

    if let Some((user_id,)) = by_email {
        sqlx::query(
            "UPDATE rootcx_system.users SET oidc_provider = $1, oidc_sub = $2, updated_at = now() WHERE id = $3",
        )
        .bind(&provider.id)
        .bind(sub)
        .bind(user_id)
        .execute(pool)
        .await?;

        return Ok(user_id);
    }

    // 3. Auto-register if enabled
    if !provider.auto_register {
        return Err(ApiError::Forbidden("user not provisioned".into()));
    }

    let (user_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO rootcx_system.users (email, display_name, oidc_provider, oidc_sub)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(email)
    .bind(display_name)
    .bind(&provider.id)
    .bind(sub)
    .fetch_one(pool)
    .await
    .map_err(|e| {
        if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
            ApiError::BadRequest(format!("email '{email}' already taken"))
        } else {
            ApiError::Internal(e.to_string())
        }
    })?;

    // Assign role
    let assigned_role = role.unwrap_or(&provider.default_role);
    let role_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.rbac_roles WHERE name = $1)",
    )
    .bind(assigned_role)
    .fetch_one(pool)
    .await?;

    let final_role = if role_exists { assigned_role } else { &provider.default_role };

    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(final_role)
    .execute(pool)
    .await?;

    tracing::info!(user_id = %user_id, email = %email, role = %final_role, provider = %provider.id, "OIDC user auto-registered");

    Ok(user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(role_claim: Option<&str>) -> ProviderRow {
        ProviderRow {
            id: "test".into(),
            issuer_url: "https://example.com".into(),
            client_id: "cid".into(),
            client_secret: None,
            scopes: vec!["openid".into()],
            auto_register: true,
            default_role: "admin".into(),
            role_claim: role_claim.map(String::from),
        }
    }

    fn fake_jwt(payload_json: &str) -> String {
        use base64::Engine;
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"alg":"RS256"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(payload_json);
        format!("{header}.{payload}.fakesig")
    }

    // ── extract_role_claim ─────────────────────────────────────────────

    #[test]
    fn extracts_role_from_custom_claim() {
        let token = fake_jwt(r#"{"sub":"u1","role":"editor"}"#);
        let result = extract_role_claim(&token, &provider(Some("role"))).unwrap();
        assert_eq!(result.as_deref(), Some("editor"), "should extract the role claim");
    }

    #[test]
    fn returns_none_when_no_role_claim_configured() {
        let token = fake_jwt(r#"{"sub":"u1","role":"editor"}"#);
        for claim in [None, Some("")] {
            let result = extract_role_claim(&token, &provider(claim)).unwrap();
            assert!(result.is_none(), "no role_claim configured → None");
        }
    }

    #[test]
    fn returns_none_when_claim_absent_from_token() {
        let token = fake_jwt(r#"{"sub":"u1"}"#);
        let result = extract_role_claim(&token, &provider(Some("role"))).unwrap();
        assert!(result.is_none(), "missing claim in token → None");
    }

    #[test]
    fn returns_none_when_claim_is_not_string() {
        let token = fake_jwt(r#"{"sub":"u1","role":42}"#);
        let result = extract_role_claim(&token, &provider(Some("role"))).unwrap();
        assert!(result.is_none(), "non-string claim → None");
    }

    #[test]
    fn handles_nested_claim_name_literally() {
        // "org.role" is treated as a top-level key, not a path
        let token = fake_jwt(r#"{"sub":"u1","org.role":"viewer"}"#);
        let result = extract_role_claim(&token, &provider(Some("org.role"))).unwrap();
        assert_eq!(result.as_deref(), Some("viewer"));
    }

    #[test]
    fn returns_none_for_malformed_jwt() {
        // No dots at all
        assert!(extract_role_claim("nodots", &provider(Some("role"))).unwrap().is_none());
        // Only header, no payload
        assert!(extract_role_claim("header", &provider(Some("role"))).unwrap().is_none());
    }

    #[test]
    fn errors_on_invalid_base64_payload() {
        let result = extract_role_claim("header.!!!invalid!!!.sig", &provider(Some("role")));
        assert!(result.is_err(), "invalid base64 should return Err");
    }

    #[test]
    fn errors_on_invalid_json_payload() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("not json");
        let token = format!("hdr.{payload}.sig");
        let result = extract_role_claim(&token, &provider(Some("role")));
        assert!(result.is_err(), "invalid JSON should return Err");
    }

    // ── password_login_disabled ────────────────────────────────────────

    #[test]
    fn password_login_disabled_checks_env() {
        use crate::routes::auth::password_login_disabled;

        unsafe {
            // Baseline: unset → enabled
            std::env::remove_var("ROOTCX_DISABLE_PASSWORD_LOGIN");
            assert!(!password_login_disabled());

            for (val, expected) in [
                ("true", true),
                ("1", true),
                ("false", false),
                ("0", false),
                ("yes", false),
                ("", false),
            ] {
                std::env::set_var("ROOTCX_DISABLE_PASSWORD_LOGIN", val);
                assert_eq!(
                    password_login_disabled(), expected,
                    "ROOTCX_DISABLE_PASSWORD_LOGIN={val:?} should be {expected}"
                );
            }

            std::env::remove_var("ROOTCX_DISABLE_PASSWORD_LOGIN");
        }
    }
}
