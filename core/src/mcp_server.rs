//! Official inbound MCP interface for RootCX.
//!
//! MCP exposes authenticated workspace context and governed RootCX actions.
//! Application source code stays in the user's local workspace; the RootCX
//! CLI owns scaffolding, builds, tests, and deployment.

use std::collections::{HashMap, HashSet};

use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{Request, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json as AxumJson, Router};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, MetaObject, ServerCapabilities, ServerInfo};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{Json, ServerHandler, tool, tool_handler, tool_router};
use rootcx_types::AppManifest;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use crate::api_error::ApiError;
use crate::auth::identity::{Identity, authenticate_parts, authenticate_parts_for_audience};
use crate::routes::SharedRuntime;

const MAX_MCP_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
struct GrantedScopes(HashSet<String>);

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManifestInput {
    /// A complete RootCX manifest.json object.
    pub manifest: JsonValue,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppIdInput {
    pub app_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateRecordsInput {
    pub app_id: String,
    pub entity: String,
    pub records: Vec<JsonValue>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutput {
    // schemars describes a bare `serde_json::Value` as the boolean schema
    // `true`. That is valid JSON Schema, but strict clients (Claude Code,
    // Claude Cowork) reject a boolean where a subschema object is expected and
    // then drop every tool. Every tool result is a JSON object, so say so.
    #[schemars(with = "serde_json::Map<String, JsonValue>")]
    pub result: JsonValue,
}

fn oauth_tool_meta(scopes: &[&str]) -> MetaObject {
    let mut meta = MetaObject::new();
    meta.insert(
        "securitySchemes".to_string(),
        json!([{ "type": "oauth2", "scopes": scopes }]),
    );
    meta
}

#[derive(Clone)]
pub struct RootCxMcp {
    runtime: SharedRuntime,
    tool_router: ToolRouter<Self>,
}

#[tool_router(router = tool_router)]
impl RootCxMcp {
    fn new(runtime: SharedRuntime) -> Self {
        Self {
            runtime,
            tool_router: Self::tool_router(),
        }
    }

    /// Use this when starting work in RootCX or when the current workspace,
    /// user, permissions, installed applications, or onboarding state is unknown.
    #[tool(
        name = "get_project_context",
        annotations(
            title = "Get RootCX project context",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        ),
        meta = oauth_tool_meta(&["mcp:read"])
    )]
    async fn get_project_context(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ToolOutput>, String> {
        require_scopes(&context, &["mcp:read"])?;
        let identity = identity_from_context(&context)?;
        let apps = crate::routes::list_apps(
            identity.clone(),
            State(self.runtime.clone()),
            Query(HashMap::new()),
        )
        .await
        .map_err(api_error_message)?
        .0;
        let permissions = crate::governance::authority::resolve_effective_permissions(
            self.runtime.pool(),
            &identity,
        )
        .await
        .map_err(api_error_message)?;
        let onboarding =
            crate::extensions::onboarding::read_status(self.runtime.pool(), identity.user_id)
            .await
            .map_err(api_error_message)?;
        let workspace_url = std::env::var("ROOTCX_PUBLIC_URL")
            .unwrap_or_else(|_| "http://localhost:9100".into())
            .trim_end_matches('/')
            .to_string();

        Ok(Json(ToolOutput {
            result: json!({
                "runtime": self.runtime.status(),
                "workspace": { "url": workspace_url },
                "permissions": permissions,
                "apps": apps,
                "onboarding": onboarding,
                "activationTarget": "Deploy and open the first useful application"
            }),
        }))
    }

    /// Use this when the user wants to inspect or modify an existing RootCX
    /// application. Returns the installed app and its data contract.
    #[tool(
        name = "get_app",
        annotations(
            title = "Get a RootCX app",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        ),
        meta = oauth_tool_meta(&["mcp:read"])
    )]
    async fn get_app(
        &self,
        Parameters(input): Parameters<AppIdInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ToolOutput>, String> {
        require_scopes(&context, &["mcp:read"])?;
        let identity = identity_from_context(&context)?;
        let app = crate::routes::get_app(
            identity,
            State(self.runtime.clone()),
            AxumPath(input.app_id),
        )
        .await
        .map_err(api_error_message)?
        .0;
        Ok(Json(ToolOutput { result: app }))
    }

    /// Use this to validate a complete RootCX manifest against the current
    /// workspace without changing the workspace.
    #[tool(
        name = "validate_manifest",
        annotations(
            title = "Validate a RootCX manifest",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        ),
        meta = oauth_tool_meta(&["mcp:read"])
    )]
    async fn validate_manifest(
        &self,
        Parameters(input): Parameters<ManifestInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ToolOutput>, String> {
        require_scopes(&context, &["mcp:read"])?;
        let identity = identity_from_context(&context)?;
        let manifest = parse_manifest(input.manifest)?;
        crate::manifest::validate_manifest(&manifest).map_err(|error| error.to_string())?;
        let schema = crate::routes::verify_schema(
            identity,
            State(self.runtime.clone()),
            AxumJson(manifest.clone()),
        )
        .await
        .map_err(api_error_message)?
        .0;

        Ok(Json(ToolOutput {
            result: json!({
                "valid": true,
                "appId": manifest.app_id,
                "schema": schema
            }),
        }))
    }

    /// Use this when the user has approved creating initial records in an
    /// existing RootCX entity. Creates up to 1000 records through RootCX RLS
    /// and auditing. Never fabricate real customer or company data.
    #[tool(
        name = "create_records",
        annotations(
            title = "Create RootCX records",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        ),
        meta = oauth_tool_meta(&["mcp:read", "mcp:write"])
    )]
    async fn create_records(
        &self,
        Parameters(input): Parameters<CreateRecordsInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ToolOutput>, String> {
        require_scopes(&context, &["mcp:read", "mcp:write"])?;
        let identity = identity_from_context(&context)?;
        let (_, records) = crate::routes::bulk_create_records(
            State(self.runtime.clone()),
            AxumPath((input.app_id, input.entity)),
            identity,
            AxumJson(JsonValue::Array(input.records)),
        )
        .await
        .map_err(api_error_message)?;
        Ok(Json(ToolOutput {
            result: json!({ "records": records.0 }),
        }))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RootCxMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("RootCX", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Start with get_project_context. Use MCP for authenticated RootCX context, validation, and governed data actions. For application work, keep source code in the local workspace and use the RootCX CLI to scaffold, build, test, and deploy. Never send application source code through MCP and never invent manifest fields."
                    .to_string(),
            )
    }
}

pub(crate) fn router(runtime: SharedRuntime) -> Router<SharedRuntime> {
    let handler = RootCxMcp::new(runtime.clone());
    let service: StreamableHttpService<RootCxMcp, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Default::default(),
        mcp_config(),
    );

    let auth_runtime = runtime.clone();
    let protected = Router::new()
        .nest_service("/mcp", service)
        .route_layer(middleware::from_fn(move |request, next| {
            let runtime = auth_runtime.clone();
            authenticate_mcp_request(runtime, request, next)
        }));
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource_metadata),
        )
        .merge(protected)
}

fn mcp_resource_url() -> String {
    format!(
        "{}/mcp",
        std::env::var("ROOTCX_PUBLIC_URL")
            .unwrap_or_else(|_| "http://localhost:9100".into())
            .trim_end_matches('/')
    )
}

async fn protected_resource_metadata() -> impl IntoResponse {
    let authorization_servers = std::env::var("ROOTCX_OIDC_ISSUER")
        .ok()
        .filter(|value| !value.is_empty())
        .into_iter()
        .collect::<Vec<_>>();
    AxumJson(json!({
        "resource": mcp_resource_url(),
        "authorization_servers": authorization_servers,
        "scopes_supported": ["mcp:read", "mcp:write"],
        "bearer_methods_supported": ["header"]
    }))
}

fn mcp_config() -> StreamableHttpServerConfig {
    let mut hosts = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    let mut origins = Vec::new();

    if let Ok(public_url) = std::env::var("ROOTCX_PUBLIC_URL")
        && let Ok(url) = url::Url::parse(&public_url)
    {
        if let Some(host) = url.host_str() {
            hosts.push(host.to_string());
            if let Some(port) = url.port() {
                hosts.push(format!("{host}:{port}"));
            }
        }
        origins.push(url.origin().ascii_serialization());
    }
    if let Ok(extra) = std::env::var("ROOTCX_MCP_ALLOWED_HOSTS") {
        hosts.extend(
            extra
                .split(',')
                .map(str::trim)
                .filter(|host| !host.is_empty())
                .map(str::to_string),
        );
    }
    hosts.sort();
    hosts.dedup();

    let mut config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(hosts)
        .with_allowed_origins(origins);
    config.max_request_body_bytes = MAX_MCP_BODY_BYTES;
    config
}

async fn authenticate_mcp_request(
    runtime: SharedRuntime,
    request: Request<Body>,
    next: Next,
) -> Response {
    let (mut parts, body) = request.into_parts();
    let resource = mcp_resource_url();
    let require_audience = url::Url::parse(&resource).is_ok_and(|url| url.scheme() == "https")
        && std::env::var("ROOTCX_MCP_ALLOW_LEGACY_BEARER").as_deref() != Ok("true");
    let authenticated = match authenticate_parts_for_audience(&parts, &runtime, &resource).await {
        Ok(authenticated) => Ok((
            authenticated.identity,
            GrantedScopes(authenticated.scopes),
        )),
        Err(_) if !require_audience => authenticate_parts(&parts, &runtime)
            .await
            .map(|identity| {
                (
                    identity,
                    GrantedScopes(HashSet::from(["mcp:read".into(), "mcp:write".into()])),
                )
            }),
        Err(error) => Err(error),
    };
    match authenticated {
        Ok((identity, scopes)) => {
            parts.extensions.insert(identity);
            parts.extensions.insert(scopes);
            next.run(Request::from_parts(parts, body)).await
        }
        Err(error) => {
            let unauthorized = matches!(error, ApiError::Unauthorized(_));
            let mut response = error.into_response();
            if unauthorized {
                let metadata = format!(
                    "{}/.well-known/oauth-protected-resource/mcp",
                    resource.trim_end_matches("/mcp")
                );
                let value = format!("Bearer resource_metadata=\"{metadata}\", scope=\"mcp:read\"");
                if let Ok(value) = header::HeaderValue::from_str(&value) {
                    response
                        .headers_mut()
                        .insert(header::WWW_AUTHENTICATE, value);
                }
            }
            response
        }
    }
}

fn require_scopes(context: &RequestContext<RoleServer>, required: &[&str]) -> Result<(), String> {
    let parts = context
        .extensions
        .get::<axum::http::request::Parts>()
        .ok_or_else(|| "authenticated HTTP request context is missing".to_string())?;
    let granted = parts
        .extensions
        .get::<GrantedScopes>()
        .ok_or_else(|| "MCP OAuth scopes are missing".to_string())?;
    if has_scopes(granted, required) {
        Ok(())
    } else {
        Err(format!(
            "missing required OAuth scope: {}",
            required.join(" ")
        ))
    }
}

fn has_scopes(granted: &GrantedScopes, required: &[&str]) -> bool {
    required.iter().all(|scope| granted.0.contains(*scope))
}

fn identity_from_context(context: &RequestContext<RoleServer>) -> Result<Identity, String> {
    let parts = context
        .extensions
        .get::<axum::http::request::Parts>()
        .ok_or_else(|| "authenticated HTTP request context is missing".to_string())?;
    parts
        .extensions
        .get::<Identity>()
        .cloned()
        .ok_or_else(|| "authenticated RootCX identity is missing".to_string())
}

fn parse_manifest(value: JsonValue) -> Result<AppManifest, String> {
    serde_json::from_value(value).map_err(|error| format!("invalid RootCX manifest: {error}"))
}

fn api_error_message(error: ApiError) -> String {
    match error {
        ApiError::NotFound(message)
        | ApiError::BadRequest(message)
        | ApiError::Unauthorized(message)
        | ApiError::Forbidden(message)
        | ApiError::Conflict(message)
        | ApiError::Unavailable(message)
        | ApiError::Internal(message) => message,
        ApiError::NotReady => "runtime not ready".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_tools_advertise_exact_safety_and_oauth_metadata() {
        let tools = [
            (
                RootCxMcp::get_project_context_tool_attr(),
                true,
                false,
                None,
                json!([{ "type": "oauth2", "scopes": ["mcp:read"] }]),
            ),
            (
                RootCxMcp::get_app_tool_attr(),
                true,
                false,
                None,
                json!([{ "type": "oauth2", "scopes": ["mcp:read"] }]),
            ),
            (
                RootCxMcp::validate_manifest_tool_attr(),
                true,
                false,
                None,
                json!([{ "type": "oauth2", "scopes": ["mcp:read"] }]),
            ),
            (
                RootCxMcp::create_records_tool_attr(),
                false,
                false,
                Some(false),
                json!([{
                    "type": "oauth2", "scopes": ["mcp:read", "mcp:write"]
                }]),
            ),
        ];

        for (tool, read_only, destructive, idempotent, security_schemes) in tools {
            let annotations = tool.annotations.expect("tool safety annotations");
            assert_eq!(annotations.read_only_hint, Some(read_only));
            assert_eq!(annotations.destructive_hint, Some(destructive));
            assert_eq!(annotations.idempotent_hint, idempotent);
            assert_eq!(annotations.open_world_hint, Some(false));
            assert_eq!(
                tool.meta
                    .as_ref()
                    .and_then(|meta| meta.get("securitySchemes")),
                Some(&security_schemes),
                "wrong OAuth scopes for {}",
                tool.name,
            );
        }

        let read = GrantedScopes(HashSet::from(["mcp:read".into()]));
        assert!(has_scopes(&read, &["mcp:read"]));
        assert!(!has_scopes(&read, &["mcp:read", "mcp:write"]));
    }
}
