pub mod agents;
pub(crate) mod audit;
pub mod auth;
pub mod channels;
pub mod hooks;
pub mod integrations;
pub mod logs;
pub mod magic_link;
pub mod mcp;
pub mod oidc;
pub mod onboarding;
pub mod rbac;
pub mod service_accounts;
pub mod sharing;
pub mod platform_storage;
pub mod storage;
pub mod workflows;

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use sqlx::PgPool;
use uuid::Uuid;

use crate::RuntimeError;
use crate::auth::AuthConfig;
use crate::routes::SharedRuntime;
use rootcx_types::AppManifest;

#[async_trait]
pub trait RuntimeExtension: Send + Sync {
    fn name(&self) -> &str;
    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError>;

    async fn on_table_created(
        &self,
        _pool: &PgPool,
        _manifest: &AppManifest,
        _schema: &str,
        _table: &str,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }

    async fn on_app_installed(&self, _pool: &PgPool, _manifest: &AppManifest, _installed_by: Uuid) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        None
    }
}

/// Build all built-in extensions in correct bootstrap order.
///
/// The order is load-bearing; `bootstrap_order_satisfies_declared_dependencies`
/// enforces it. Current constraints:
/// * audit before hooks — audit creates `sensitive_fields`, read by the hooks trigger
/// * auth before rbac — `rbac_assignments` references the users table
/// * rbac before service_accounts — they register keys into `rbac_permissions`
pub fn builtin_extensions(auth_config: Arc<AuthConfig>) -> Vec<Box<dyn RuntimeExtension>> {
    vec![
        Box::new(audit::AuditExtension),
        Box::new(hooks::HooksExtension),
        Box::new(logs::LogsExtension),
        Box::new(auth::AuthExtension { config: Arc::clone(&auth_config) }),
        Box::new(rbac::RbacExtension),
        // After RBAC: registers core permission keys into rbac_permissions.
        Box::new(service_accounts::ServiceAccountExtension { config: auth_config }),
        Box::new(sharing::SharingExtension),
        Box::new(oidc::OidcExtension),
        Box::new(magic_link::MagicLinkExtension),
        Box::new(agents::AgentExtension),
        Box::new(integrations::IntegrationsExtension),
        Box::new(mcp::McpExtension),
        Box::new(onboarding::OnboardingExtension),
        Box::new(channels::ChannelExtension),
        Box::new(storage::StorageExtension),
        Box::new(platform_storage::PlatformStorageExtension),
        Box::new(workflows::WorkflowExtension),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `builtin_extensions` order is load-bearing: each bootstrap runs in
    /// sequence, and several create objects a later one depends on. Nothing else
    /// enforces it — a reorder compiles fine and the failure surfaces late (a
    /// plpgsql function resolves its tables at call time, not at creation, so a
    /// misordered trigger dependency breaks the first write rather than boot).
    ///
    /// One test for all the ordering constraints, keyed on `name()` so it does
    /// not encode positions and survives inserting an unrelated extension.
    #[test]
    fn bootstrap_order_satisfies_declared_dependencies() {
        let secret = b"test-secret-key-for-unit-tests!!";
        let config = Arc::new(AuthConfig {
            encoding_key: jsonwebtoken::EncodingKey::from_secret(secret),
            decoding_key: jsonwebtoken::DecodingKey::from_secret(secret),
            access_ttl: std::time::Duration::from_secs(900),
            refresh_ttl: std::time::Duration::from_secs(86400),
        });
        let extensions = builtin_extensions(config);
        let names: Vec<&str> = extensions.iter().map(|e| e.name()).collect();

        let position = |name: &str| {
            names
                .iter()
                .position(|n| *n == name)
                .unwrap_or_else(|| panic!("extension '{name}' is missing from builtin_extensions"))
        };

        for (earlier, later, why) in [
            (
                "audit",
                "hooks",
                "audit creates rootcx_system.sensitive_fields, which the hooks trigger reads",
            ),
            (
                "auth",
                "rbac",
                "rbac_assignments references the users table",
            ),
            (
                "rbac",
                "service_accounts",
                "service accounts register permission keys into rbac_permissions",
            ),
        ] {
            assert!(
                position(earlier) < position(later),
                "'{earlier}' must bootstrap before '{later}': {why} (order: {names:?})"
            );
        }
    }
}
