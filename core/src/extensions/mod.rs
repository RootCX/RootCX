pub mod agents;
mod audit;
pub mod auth;
pub mod browser;
pub mod logs;
pub mod rbac;

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

    async fn on_app_installed(&self, _pool: &PgPool, _manifest: &AppManifest, _installed_by: Uuid, _tool_names: &[String]) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        None
    }
}

/// Build all built-in extensions in correct bootstrap order.
/// Auth must come before RBAC (rbac_assignments references users table).
pub fn builtin_extensions(
    auth_config: Arc<AuthConfig>,
    browser_queue: Arc<browser::queue::BrowserQueue>,
) -> Vec<Box<dyn RuntimeExtension>> {
    vec![
        Box::new(audit::AuditExtension),
        Box::new(logs::LogsExtension),
        Box::new(auth::AuthExtension { config: auth_config }),
        Box::new(rbac::RbacExtension),
        Box::new(agents::AgentExtension),
        Box::new(browser::BrowserExtension::with_queue(browser_queue)),
    ]
}
