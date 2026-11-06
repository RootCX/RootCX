//! Official inbound MCP interface for RootCX.
//!
//! MCP exposes authenticated workspace context and governed RootCX actions.
//! Application source code stays in the user's local workspace; the RootCX
//! CLI owns scaffolding, builds, tests, and deployment.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{Request, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json as AxumJson, Router};
use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, MetaObject, ServerCapabilities, ServerInfo};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{Json, ServerHandler, tool, tool_handler, tool_router};
use rootcx_types::AppManifest;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use crate::api_error::ApiError;
use crate::governance::action_invocation::{Declaration, authorize, dispatch};
use crate::governance::authority::{require_perm, resolve_permissions};
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
pub struct CallActionInput {
    /// Installed app ID, from get_project_context.
    pub app: String,
    /// Action ID listed by describe_app for this app.
    pub action: String,
    /// Action input matching the action's inputSchema.
    #[serde(default)]
    pub input: serde_json::Map<String, JsonValue>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CreateRecordInput {
    /// Installed app ID, from get_project_context.
    pub app: String,
    /// Entity name listed by describe_app.
    pub entity: String,
    /// Field values by field name. Array fields take JSON arrays.
    pub data: serde_json::Map<String, JsonValue>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct UpdateRecordInput {
    /// Installed app ID, from get_project_context.
    pub app: String,
    /// Entity name listed by describe_app.
    pub entity: String,
    /// The record id, from query_data or an action result.
    pub id: String,
    /// The fields to overwrite, by field name.
    pub data: serde_json::Map<String, JsonValue>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadFileInput {
    /// Installed app ID that receives the file.
    pub app: String,
    /// The file the person attached, when the host passes it by reference.
    #[serde(default)]
    pub file: Option<crate::mcp_files::AttachedFile>,
    /// Otherwise: the file name with its extension.
    #[serde(default)]
    pub name: Option<String>,
    /// Otherwise: its MIME type, for example application/pdf or image/png.
    #[serde(default)]
    pub content_type: Option<String>,
    /// Otherwise: its content, base64 encoded.
    #[serde(default)]
    pub content_base64: Option<String>,
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

/// Audit and system identities an external assistant has no use for; `updated_at` stays
/// because apps use it to detect concurrent changes.
const INTERNAL_FIELDS: [&str; 12] = [
    "created_at", "created_by", "updated_by", "deleted_by", "owner_id", "user_id",
    "tenant_id", "project_id", "installation_id", "request_id", "trace_id", "session_id",
];

/// Results as an assistant receives them: without internal fields, at any depth.
fn minimized(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Array(items) => JsonValue::Array(items.into_iter().map(minimized).collect()),
        JsonValue::Object(fields) => JsonValue::Object(
            fields.into_iter().filter(|(key, _)| !INTERNAL_FIELDS.contains(&key.as_str())).map(|(key, item)| (key, minimized(item))).collect(),
        ),
        other => other,
    }
}

/// Read-only work needs `mcp:read`; anything else also `mcp:write`.
fn scopes(read_only: bool) -> &'static [&'static str] {
    if read_only { &["mcp:read"] } else { &["mcp:read", "mcp:write"] }
}

/// The caller-safe message; faults are logged here, since MCP errors bypass the HTTP response.
fn tool_error(error: ApiError) -> String {
    if matches!(error, ApiError::Internal(_) | ApiError::Unavailable(_) | ApiError::NotReady) {
        tracing::error!(%error, "MCP tool failed");
    }
    error.to_string()
}

/// Hosts that support it pass an attached file by reference in `file`.
fn upload_file_meta() -> MetaObject {
    let mut meta = oauth_tool_meta(scopes(false));
    meta.insert("openai/fileParams".to_string(), json!(["file"]));
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
        let mut tool_router = Self::tool_router();
        for name in REGISTRY_TOOLS {
            if let Some(route) = registry_route(&runtime, name) {
                tool_router.add_route(route);
            }
        }
        Self { runtime, tool_router }
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
        .map_err(|error| error.to_string())?
        .0;
        let permissions = crate::governance::authority::resolve_effective_permissions(
            self.runtime.pool(),
            &identity,
        )
        .await
        .map_err(|error| error.to_string())?;
        let onboarding =
            crate::extensions::onboarding::read_status(self.runtime.pool(), identity.user_id)
            .await
            .map_err(|error| error.to_string())?;
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
        .map_err(|error| error.to_string())?
        .0;

        Ok(Json(ToolOutput {
            result: json!({
                "valid": true,
                "appId": manifest.app_id,
                "schema": schema
            }),
        }))
    }

    /// Use this to run one action an app declares, listed by describe_app,
    /// for the person. Actions apply the app's business rules. Actions marked
    /// for people only are refused.
    #[tool(
        name = "call_action",
        annotations(
            title = "Run an app action",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        ),
        meta = oauth_tool_meta(&["mcp:read", "mcp:write"])
    )]
    async fn call_action(
        &self,
        Parameters(input): Parameters<CallActionInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ToolOutput>, String> {
        let identity = identity_from_context(&context)?;
        let pool = self.runtime.pool();
        let declaration = Declaration::load(pool, &input.app, &input.action).await.map_err(tool_error)?;
        let (_, permissions) = resolve_permissions(pool, identity.user_id).await.map_err(tool_error)?;
        // Grants first: a caller without them learns nothing about the declaration.
        authorize(&input.app, &input.action, declaration.as_ref(), &permissions, true).map_err(tool_error)?;
        require_scopes(&context, scopes(declaration.as_ref().is_some_and(|d| d.read_only)))?;
        let caller = crate::ipc::RpcCaller::person(identity.user_id, &identity.email);
        let dispatcher = self.runtime.worker_manager().action_caller();
        let result = dispatch(dispatcher.as_ref(), &input.app, &input.action, declaration.as_ref(), JsonValue::Object(input.input), caller)
            .await.map_err(tool_error)?;
        Ok(Json(ToolOutput { result: minimized(result) }))
    }

    /// Use this to create one record in an app entity when no app action fits:
    /// actions apply the app's rules. Creates a single record, nothing else.
    #[tool(
        name = "create_record",
        annotations(
            title = "Create a record",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        ),
        meta = oauth_tool_meta(scopes(false))
    )]
    async fn create_record(
        &self,
        Parameters(input): Parameters<CreateRecordInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ToolOutput>, String> {
        let args = json!({"app": input.app, "entity": input.entity, "action": "create", "data": input.data});
        self.mutate(&context, input.app, args).await
    }

    /// Use this to change fields of one existing record when no app action fits:
    /// actions apply the app's rules. Overwrites only the given fields.
    #[tool(
        name = "update_record",
        annotations(
            title = "Update a record",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        ),
        meta = oauth_tool_meta(scopes(false))
    )]
    async fn update_record(
        &self,
        Parameters(input): Parameters<UpdateRecordInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ToolOutput>, String> {
        let args = json!({"app": input.app, "entity": input.entity, "action": "update", "id": input.id, "data": input.data});
        self.mutate(&context, input.app, args).await
    }

    /// Use this to store a file the person gave you (a PDF or an image) in an
    /// app, then pass the returned file_id to the action input that expects it.
    #[tool(
        name = "upload_file",
        annotations(
            title = "Add a file to an app",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        ),
        meta = upload_file_meta()
    )]
    async fn upload_file(
        &self,
        Parameters(input): Parameters<UploadFileInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ToolOutput>, String> {
        require_scopes(&context, scopes(false))?;
        let identity = identity_from_context(&context)?;
        // Checked before anything is downloaded or decoded.
        require_perm(self.runtime.pool(), identity.user_id, &format!("app:{}:storage.write", input.app)).await.map_err(tool_error)?;
        let file = match input.file {
            Some(file) => crate::mcp_files::fetch(&file, crate::extensions::storage::max_file_bytes()).await?,
            None => {
                let (Some(name), Some(content_type), Some(encoded)) = (input.name, input.content_type, input.content_base64) else {
                    return Err("pass the attached file as file, or name, contentType and contentBase64".into());
                };
                let content = base64::engine::Engine::decode(&base64::engine::general_purpose::STANDARD, &encoded)
                    .map_err(|_| "contentBase64 is not valid base64".to_string())?;
                crate::mcp_files::Fetched { name, content_type, content }
            }
        };
        let chunks = futures::stream::once(async { Ok(axum::body::Bytes::from(file.content)) });
        let result = crate::extensions::storage::store_person_file(
            self.runtime.pool(), identity.user_id, &input.app, &file.name, &file.content_type, chunks,
        ).await.map_err(tool_error)?;
        Ok(Json(ToolOutput { result }))
    }
}

impl RootCxMcp {
    /// One operation per tool (hosts must never choose it through an argument),
    /// each run by the registry's `mutate_data` with the person's permissions.
    async fn mutate(&self, context: &RequestContext<RoleServer>, app: String, args: JsonValue) -> Result<Json<ToolOutput>, String> {
        require_scopes(context, scopes(false))?;
        let identity = identity_from_context(context)?;
        let result = crate::tools::routes::execute_as_person(&self.runtime, &identity, "mutate_data", app, args, true)
            .await.map_err(tool_error)?;
        Ok(Json(ToolOutput { result: minimized(result) }))
    }
}

/// Core's governed read tools, served from the tool registry as registered:
/// one implementation and one descriptor for agents, HTTP and MCP (ADR 0015).
const REGISTRY_TOOLS: [&str; 2] = ["describe_app", "query_data"];

fn registry_route(runtime: &SharedRuntime, name: &'static str) -> Option<ToolRoute<RootCxMcp>> {
    let (descriptor, annotations) = runtime.tool_registry().http_descriptors().into_iter().find(|(d, _)| d.name == name)?;
    let scopes = scopes(annotations.as_ref().is_some_and(|hints| hints.read_only_hint == Some(true)));
    let schema = descriptor.input_schema.as_object()?.clone();
    let mut tool = rmcp::model::Tool::new(name, descriptor.description, Arc::new(schema)).with_meta(oauth_tool_meta(scopes));
    if let Some(hints) = annotations {
        tool = tool.with_annotations(
            rmcp::model::ToolAnnotations::new()
                .read_only(hints.read_only_hint.unwrap_or(false))
                .destructive(hints.destructive_hint.unwrap_or(true))
                .idempotent(hints.idempotent_hint.unwrap_or(false))
                .open_world(hints.open_world_hint.unwrap_or(true)),
        );
    }
    Some(ToolRoute::new_dyn(tool, move |call: ToolCallContext<'_, RootCxMcp>| {
        Box::pin(async move {
            let outcome = run_registry_tool(&call, name, scopes).await;
            Ok(match outcome {
                Ok(result) => CallToolResult::structured(json!({ "result": minimized(result) })),
                Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
            }.into())
        })
    }))
}

async fn run_registry_tool(call: &ToolCallContext<'_, RootCxMcp>, name: &str, scopes: &[&str]) -> Result<JsonValue, String> {
    require_scopes(&call.request_context, scopes)?;
    let identity = identity_from_context(&call.request_context)?;
    let args = JsonValue::Object(call.arguments.clone().unwrap_or_default());
    let app = args.get("app").and_then(JsonValue::as_str).ok_or("missing: app")?.to_string();
    crate::tools::routes::execute_as_person(&call.service.runtime, &identity, name, app, args, true)
        .await.map_err(tool_error)
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RootCxMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("RootCX", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Start with get_project_context, then describe_app for the app you need: it lists the records you may read and the actions you may run. Prefer an app action (call_action) to raw record changes: actions apply the app's rules. Use query_data to read, create_record to add and update_record to change a record. You cannot delete records or run actions reserved for people; say the person does it in the app. Never invent names, amounts or terms. For application work, keep source code in the local workspace and use the RootCX CLI to scaffold, build, test, and deploy. Never send application source code through MCP and never invent manifest fields."
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
                RootCxMcp::validate_manifest_tool_attr(),
                true,
                false,
                None,
                json!([{ "type": "oauth2", "scopes": ["mcp:read"] }]),
            ),
            (
                RootCxMcp::create_record_tool_attr(),
                false,
                false,
                Some(false),
                json!([{
                    "type": "oauth2", "scopes": ["mcp:read", "mcp:write"]
                }]),
            ),
            (
                RootCxMcp::update_record_tool_attr(),
                false,
                true,
                Some(true),
                json!([{
                    "type": "oauth2", "scopes": ["mcp:read", "mcp:write"]
                }]),
            ),
            (
                RootCxMcp::upload_file_tool_attr(),
                false,
                false,
                Some(false),
                json!([{
                    "type": "oauth2", "scopes": ["mcp:read", "mcp:write"]
                }]),
            ),
        ];
        let call_action = RootCxMcp::call_action_tool_attr();
        let hints = call_action.annotations.expect("call_action hints");
        assert_eq!(
            (hints.read_only_hint, hints.destructive_hint, hints.open_world_hint),
            (Some(false), Some(true), Some(true)),
            "one tool runs any declared action, so it claims the worst case",
        );

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

    #[test]
    fn assistants_never_receive_internal_identities() {
        let record = json!({"id": "r1", "updated_at": "t", "owner_id": "u", "lines": [{"created_by": "u", "qty": 2}]});
        assert_eq!(minimized(record), json!({"id": "r1", "updated_at": "t", "lines": [{"qty": 2}]}));
    }
}
