mod audit;
pub mod logs;

use async_trait::async_trait;
use axum::Router;
use sqlx::PgPool;

use crate::routes::SharedRuntime;
use crate::RuntimeError;

#[async_trait]
pub trait RuntimeExtension: Send + Sync {
    fn name(&self) -> &str;
    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError>;

    async fn on_table_created(&self, _pool: &PgPool, _schema: &str, _table: &str) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        None
    }
}

pub fn builtin_extensions() -> Vec<Box<dyn RuntimeExtension>> {
    vec![Box::new(audit::AuditExtension), Box::new(logs::LogsExtension)]
}
