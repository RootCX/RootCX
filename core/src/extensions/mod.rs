pub mod agents;
mod audit;
pub mod auth;
pub mod logs;
pub mod rbac;

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use sqlx::PgPool;

use crate::RuntimeError;
use crate::auth::AuthConfig;
use crate::routes::SharedRuntime;
use rootcx_shared_types::AppManifest;

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

    async fn on_app_installed(&self, _pool: &PgPool, _manifest: &AppManifest) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        None
    }
}

/// Build all built-in extensions in correct bootstrap order.
/// Auth must come before RBAC (rbac_assignments references users table).
pub fn builtin_extensions_with_cache(
    auth_config: Arc<AuthConfig>,
    rbac_cache: Arc<rbac::PolicyCache>,
) -> Vec<Box<dyn RuntimeExtension>> {
    vec![
        Box::new(audit::AuditExtension),
        Box::new(logs::LogsExtension),
        Box::new(auth::AuthExtension { config: auth_config }),
        Box::new(rbac::RbacExtension::with_cache(Arc::clone(&rbac_cache))),
        Box::new(agents::AgentExtension::with_cache(rbac_cache)),
    ]
}
