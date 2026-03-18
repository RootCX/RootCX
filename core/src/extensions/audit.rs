use std::collections::HashMap;

use async_trait::async_trait;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;
use crate::manifest::quote_ident;
use crate::routes::{self, SharedRuntime, query_params};

use super::RuntimeExtension;

async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

pub struct AuditExtension;

#[async_trait]
impl RuntimeExtension for AuditExtension {
    fn name(&self) -> &str {
        "audit"
    }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping audit extension");

        exec(
            pool,
            r#"
            CREATE TABLE IF NOT EXISTS rootcx_system.audit_log (
                id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                table_schema TEXT NOT NULL,
                table_name   TEXT NOT NULL,
                record_id    TEXT,
                operation    TEXT NOT NULL,
                old_record   JSONB,
                new_record   JSONB,
                changed_at   TIMESTAMPTZ NOT NULL DEFAULT now()
            )"#,
        )
        .await?;

        exec(pool, "CREATE INDEX IF NOT EXISTS idx_audit_id_desc ON rootcx_system.audit_log (id DESC)").await?;
        exec(pool, "CREATE INDEX IF NOT EXISTS idx_audit_ts ON rootcx_system.audit_log (changed_at DESC)").await?;
        exec(pool, "CREATE INDEX IF NOT EXISTS idx_audit_table ON rootcx_system.audit_log (table_schema, table_name)")
            .await?;

        exec(
            pool,
            r#"
            CREATE OR REPLACE FUNCTION rootcx_system.audit_trigger_fn()
            RETURNS TRIGGER AS $$
            DECLARE rec_id TEXT;
            BEGIN
                rec_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.id::TEXT ELSE NEW.id::TEXT END;
                INSERT INTO rootcx_system.audit_log
                    (table_schema, table_name, record_id, operation, old_record, new_record)
                VALUES (
                    TG_TABLE_SCHEMA, TG_TABLE_NAME, rec_id, TG_OP,
                    CASE WHEN TG_OP IN ('UPDATE','DELETE') THEN to_jsonb(OLD) ELSE NULL END,
                    CASE WHEN TG_OP IN ('INSERT','UPDATE') THEN to_jsonb(NEW) ELSE NULL END
                );
                RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
            END;
            $$ LANGUAGE plpgsql SECURITY DEFINER"#,
        )
        .await?;

        exec(
            pool,
            r#"
            CREATE OR REPLACE FUNCTION rootcx_system.enable_tracking(target_table REGCLASS)
            RETURNS VOID AS $$
            DECLARE trigger_name TEXT;
            BEGIN
                trigger_name := regexp_replace('audit_' || target_table::TEXT, '[^a-zA-Z0-9_]', '_', 'g');
                EXECUTE format(
                    'CREATE OR REPLACE TRIGGER %I
                     AFTER INSERT OR UPDATE OR DELETE ON %s
                     FOR EACH ROW EXECUTE FUNCTION rootcx_system.audit_trigger_fn()',
                    trigger_name, target_table::TEXT);
            END;
            $$ LANGUAGE plpgsql"#,
        )
        .await?;

        exec(
            pool,
            r#"
            CREATE OR REPLACE FUNCTION rootcx_system.disable_tracking(target_table REGCLASS)
            RETURNS VOID AS $$
            DECLARE trigger_name TEXT;
            BEGIN
                trigger_name := regexp_replace('audit_' || target_table::TEXT, '[^a-zA-Z0-9_]', '_', 'g');
                EXECUTE format('DROP TRIGGER IF EXISTS %I ON %s', trigger_name, target_table::TEXT);
            END;
            $$ LANGUAGE plpgsql"#,
        )
        .await?;

        info!("audit extension ready");
        Ok(())
    }

    async fn on_table_created(
        &self,
        pool: &PgPool,
        _manifest: &rootcx_types::AppManifest,
        schema: &str,
        table: &str,
    ) -> Result<(), RuntimeError> {
        let sql =
            format!("SELECT rootcx_system.enable_tracking('{}.{}'::regclass)", quote_ident(schema), quote_ident(table));
        exec(pool, &sql).await
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(Router::new().route("/api/v1/audit", get(list_audit_events)))
    }
}

const AUDIT_COLS: query_params::ColumnDefs = &[
    ("table_schema", "TEXT"),
    ("table_name", "TEXT"),
    ("record_id", "TEXT"),
    ("operation", "TEXT"),
    ("changed_at", "TIMESTAMPTZ"),
];

const AUDIT_ALIASES: &[(&str, &str)] = &[
    ("app_id", "table_schema"),
    ("entity", "table_name"),
];

async fn list_audit_events(
    _identity: crate::auth::identity::Identity,
    State(rt): State<SharedRuntime>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<JsonValue>>, crate::api_error::ApiError> {
    let pool = routes::pool(&rt).await?;
    let q = query_params::parse(&params, AUDIT_COLS, AUDIT_ALIASES, 100, "id");
    let wc = q.where_clause();

    let sql = format!(
        "SELECT to_jsonb(a.*) AS row FROM rootcx_system.audit_log a {wc} ORDER BY {} {} LIMIT {}",
        q.order_col, q.order_dir, q.limit
    );

    let mut query = sqlx::query_as::<_, (JsonValue,)>(&sql);
    for val in &q.binds { query = query.bind(val); }

    let rows: Vec<(JsonValue,)> = query.fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(r,)| r).collect()))
}
