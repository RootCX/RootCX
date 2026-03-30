use std::collections::HashMap;

use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::manifest::quote_ident;
use crate::routes::{self, SharedRuntime};

use super::RuntimeExtension;

async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

pub struct HooksExtension;

#[async_trait]
impl RuntimeExtension for HooksExtension {
    fn name(&self) -> &str {
        "hooks"
    }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping hooks extension");

        // Config table — stores hook definitions
        exec(
            pool,
            r#"
            CREATE TABLE IF NOT EXISTS rootcx_system.entity_hooks (
                id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                app_id       TEXT NOT NULL,
                entity       TEXT NOT NULL,
                operation    TEXT NOT NULL CHECK (operation IN ('INSERT', 'UPDATE', 'DELETE')),
                action_type  TEXT NOT NULL CHECK (action_type IN ('job', 'agent')),
                action_config JSONB NOT NULL DEFAULT '{}',
                active       BOOLEAN NOT NULL DEFAULT true,
                created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
            )"#,
        )
        .await?;

        exec(
            pool,
            "CREATE INDEX IF NOT EXISTS idx_hooks_lookup ON rootcx_system.entity_hooks (app_id, entity, operation) WHERE active = true",
        )
        .await?;

        // Trigger function — checks entity_hooks config, enqueues to pgmq if match
        exec(
            pool,
            r#"
            CREATE OR REPLACE FUNCTION rootcx_system.hooks_trigger_fn()
            RETURNS TRIGGER AS $$
            DECLARE
                hook RECORD;
                rec_id TEXT;
                record_data JSONB;
                old_data JSONB;
            BEGIN
                rec_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.id::TEXT ELSE NEW.id::TEXT END;
                record_data := CASE WHEN TG_OP IN ('INSERT', 'UPDATE') THEN to_jsonb(NEW) ELSE NULL END;
                old_data := CASE WHEN TG_OP IN ('UPDATE', 'DELETE') THEN to_jsonb(OLD) ELSE NULL END;

                FOR hook IN
                    SELECT id, action_type, action_config
                    FROM rootcx_system.entity_hooks
                    WHERE app_id = TG_TABLE_SCHEMA
                      AND entity = TG_TABLE_NAME
                      AND operation = TG_OP
                      AND active = true
                LOOP
                    PERFORM pgmq.send('jobs', jsonb_build_object(
                        'app_id', TG_TABLE_SCHEMA,
                        'payload', jsonb_build_object(
                            '_hook', true,
                            'hook_id', hook.id,
                            'entity', TG_TABLE_NAME,
                            'operation', TG_OP,
                            'record_id', rec_id,
                            'record', record_data,
                            'old_record', old_data,
                            'action_type', hook.action_type,
                            'action_config', hook.action_config
                        )
                    ));
                END LOOP;

                RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
            END;
            $$ LANGUAGE plpgsql SECURITY DEFINER"#,
        )
        .await?;

        // Helper to enable hooks on a table
        exec(
            pool,
            r#"
            CREATE OR REPLACE FUNCTION rootcx_system.enable_hooks(target_table REGCLASS)
            RETURNS VOID AS $$
            DECLARE trigger_name TEXT;
            BEGIN
                trigger_name := regexp_replace('hooks_' || target_table::TEXT, '[^a-zA-Z0-9_]', '_', 'g');
                EXECUTE format(
                    'CREATE OR REPLACE TRIGGER %I
                     AFTER INSERT OR UPDATE OR DELETE ON %s
                     FOR EACH ROW EXECUTE FUNCTION rootcx_system.hooks_trigger_fn()',
                    trigger_name, target_table::TEXT);
            END;
            $$ LANGUAGE plpgsql"#,
        )
        .await?;

        info!("hooks extension ready");
        Ok(())
    }

    async fn on_table_created(
        &self,
        pool: &PgPool,
        _manifest: &rootcx_types::AppManifest,
        schema: &str,
        table: &str,
    ) -> Result<(), RuntimeError> {
        let sql = format!(
            "SELECT rootcx_system.enable_hooks('{}.{}'::regclass)",
            quote_ident(schema),
            quote_ident(table)
        );
        exec(pool, &sql).await
    }

    async fn on_app_installed(
        &self,
        pool: &PgPool,
        manifest: &rootcx_types::AppManifest,
        _installed_by: uuid::Uuid,
    ) -> Result<(), RuntimeError> {
        let trigger = match &manifest.trigger {
            Some(t) => t,
            None => return Ok(()),
        };

        for op in &trigger.on {
            let operation = op.to_uppercase();
            if !["INSERT", "UPDATE", "DELETE"].contains(&operation.as_str()) {
                return Err(RuntimeError::Schema(sqlx::Error::Protocol(format!(
                    "invalid trigger operation: '{op}'"
                ))));
            }

            let config = serde_json::json!({ "app_id": manifest.app_id });
            sqlx::query(
                r#"
                INSERT INTO rootcx_system.entity_hooks (app_id, entity, operation, action_type, action_config)
                VALUES ($1, $2, $3, 'agent', $4)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(&trigger.app_id)
            .bind(&trigger.entity)
            .bind(&operation)
            .bind(&config)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;

            info!(
                app_id = %manifest.app_id,
                entity = %trigger.entity,
                operation = %operation,
                "trigger hook registered from manifest"
            );
        }

        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route("/api/v1/apps/{app_id}/hooks", get(list_hooks).post(create_hook))
                .route("/api/v1/apps/{app_id}/hooks/{hook_id}", get(get_hook).delete(delete_hook)),
        )
    }
}

// ── API types ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateHookRequest {
    entity: String,
    operation: String,
    action_type: String,
    action_config: Option<JsonValue>,
}

// ── Route handlers ───────────────────────────────────────────────────────

async fn list_hooks(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = routes::pool(&rt);

    let entity_filter = params.get("entity");
    let op_filter = params.get("operation");

    let mut sql = "SELECT to_jsonb(h.*) AS row FROM rootcx_system.entity_hooks h WHERE app_id = $1".to_string();
    let mut binds: Vec<String> = vec![app_id];

    if let Some(entity) = entity_filter {
        binds.push(entity.clone());
        sql.push_str(&format!(" AND entity = ${}", binds.len()));
    }
    if let Some(op) = op_filter {
        binds.push(op.to_uppercase());
        sql.push_str(&format!(" AND operation = ${}", binds.len()));
    }

    sql.push_str(" ORDER BY created_at DESC");

    let mut query = sqlx::query_as::<_, (JsonValue,)>(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    let rows: Vec<(JsonValue,)> = query.fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(r,)| r).collect()))
}

async fn create_hook(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<CreateHookRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);

    let operation = body.operation.to_uppercase();
    if !["INSERT", "UPDATE", "DELETE"].contains(&operation.as_str()) {
        return Err(ApiError::BadRequest("operation must be INSERT, UPDATE, or DELETE".into()));
    }
    if !["job", "agent"].contains(&body.action_type.as_str()) {
        return Err(ApiError::BadRequest("action_type must be 'job' or 'agent'".into()));
    }

    let config = body.action_config.unwrap_or(serde_json::json!({}));

    let (row,): (JsonValue,) = sqlx::query_as(
        r#"
        INSERT INTO rootcx_system.entity_hooks (app_id, entity, operation, action_type, action_config)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING to_jsonb(rootcx_system.entity_hooks.*)
        "#,
    )
    .bind(&app_id)
    .bind(&body.entity)
    .bind(&operation)
    .bind(&body.action_type)
    .bind(&config)
    .fetch_one(&pool)
    .await?;

    Ok(Json(row))
}

async fn get_hook(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, hook_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);

    let row: Option<(JsonValue,)> = sqlx::query_as(
        "SELECT to_jsonb(h.*) FROM rootcx_system.entity_hooks h WHERE id = $1::uuid AND app_id = $2",
    )
    .bind(&hook_id)
    .bind(&app_id)
    .fetch_optional(&pool)
    .await?;

    match row {
        Some((r,)) => Ok(Json(r)),
        None => Err(ApiError::NotFound("hook not found".into())),
    }
}

async fn delete_hook(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, hook_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);

    let result = sqlx::query("DELETE FROM rootcx_system.entity_hooks WHERE id = $1::uuid AND app_id = $2")
        .bind(&hook_id)
        .bind(&app_id)
        .execute(&pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("hook not found".into()));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}
