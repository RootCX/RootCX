pub(crate) mod events;
pub(crate) mod executor;
pub(crate) mod expr;
pub(crate) mod items;
pub(crate) mod routes;
pub(crate) mod runner;
pub(crate) mod validate;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, post};
use sqlx::PgPool;
use tracing::info;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;

pub struct WorkflowExtension;

#[async_trait]
impl RuntimeExtension for WorkflowExtension {
    fn name(&self) -> &str { "workflows" }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping workflows extension");
        for ddl in [
            "CREATE TABLE IF NOT EXISTS rootcx_system.workflows (
                id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                app_id      TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
                name        TEXT NOT NULL UNIQUE,
                graph       JSONB NOT NULL DEFAULT '{\"nodes\":[],\"edges\":[]}'::jsonb,
                enabled     BOOLEAN NOT NULL DEFAULT false,
                version     INT NOT NULL DEFAULT 1,
                created_by  UUID,
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system.workflow_versions (
                workflow_id UUID NOT NULL REFERENCES rootcx_system.workflows(id) ON DELETE CASCADE,
                version     INT NOT NULL,
                graph       JSONB NOT NULL,
                published_by UUID,
                published_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (workflow_id, version)
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system.workflow_executions (
                id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                workflow_id     UUID NOT NULL REFERENCES rootcx_system.workflows(id) ON DELETE CASCADE,
                app_id          TEXT NOT NULL,
                status          TEXT NOT NULL DEFAULT 'queued',
                trigger_data    JSONB,
                run_as_user_id  UUID NOT NULL,
                error           TEXT,
                started_at      TIMESTAMPTZ,
                finished_at     TIMESTAMPTZ,
                created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE INDEX IF NOT EXISTS idx_workflow_executions_workflow
                ON rootcx_system.workflow_executions (workflow_id, created_at DESC)",
            "CREATE TABLE IF NOT EXISTS rootcx_system.workflow_node_runs (
                id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                execution_id    UUID NOT NULL REFERENCES rootcx_system.workflow_executions(id) ON DELETE CASCADE,
                node_id         TEXT NOT NULL,
                status          TEXT NOT NULL DEFAULT 'pending',
                input           JSONB,
                output          JSONB,
                attempts        INT NOT NULL DEFAULT 0,
                error           TEXT,
                started_at      TIMESTAMPTZ,
                finished_at     TIMESTAMPTZ
            )",
            "CREATE INDEX IF NOT EXISTS idx_workflow_node_runs_exec
                ON rootcx_system.workflow_node_runs (execution_id)",
            // Durable runner: graph snapshot (stable across edits/redelivery),
            // pgmq lease mapping for crash-resume, and the attempt counter.
            "ALTER TABLE rootcx_system.workflow_executions
                ADD COLUMN IF NOT EXISTS graph JSONB,
                ADD COLUMN IF NOT EXISTS lease_msg_id BIGINT,
                ADD COLUMN IF NOT EXISTS attempts INT NOT NULL DEFAULT 0",
            // One row per (execution, node): upsert on retry/resume instead of
            // appending duplicates.
            "CREATE UNIQUE INDEX IF NOT EXISTS uq_workflow_node_runs_exec_node
                ON rootcx_system.workflow_node_runs (execution_id, node_id)",
            // Resume lookup: find the in-flight execution for a redelivered lease.
            "CREATE INDEX IF NOT EXISTS idx_workflow_executions_lease
                ON rootcx_system.workflow_executions (lease_msg_id)
                WHERE lease_msg_id IS NOT NULL",
        ] {
            sqlx::query(ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
        }

        sqlx::query(
            "DO $$ BEGIN
                ALTER TABLE rootcx_system.entity_hooks
                    DROP CONSTRAINT IF EXISTS entity_hooks_action_type_check;
                ALTER TABLE rootcx_system.entity_hooks
                    ADD CONSTRAINT entity_hooks_action_type_check
                    CHECK (action_type IN ('job', 'agent', 'workflow'));
            EXCEPTION WHEN others THEN NULL;
            END $$"
        ).execute(pool).await.map_err(RuntimeError::Schema)?;

        info!("workflows tables ready");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route("/api/v1/workflows", get(routes::list_workflows).post(routes::create_workflow))
                .route("/api/v1/workflows/{workflow_id}", get(routes::get_workflow).put(routes::update_workflow).delete(routes::delete_workflow))
                .route("/api/v1/workflows/{workflow_id}/run", post(routes::run_workflow))
                .route("/api/v1/workflows/{workflow_id}/executions", get(routes::list_executions))
                .route("/api/v1/workflows/{workflow_id}/executions/{execution_id}/stream", get(routes::stream_execution))
                .route("/api/v1/workflows/nodes", get(routes::list_nodes))
        )
    }
}
