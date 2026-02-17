use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;

/// Bootstrap the `rootcx_system` schema and its seed tables.
///
/// Idempotent — safe to call on every boot.
pub async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    info!("bootstrapping rootcx_system schema");

    sqlx::query("CREATE SCHEMA IF NOT EXISTS rootcx_system")
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS rootcx_system.apps (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            version     TEXT NOT NULL DEFAULT '0.0.1',
            status      TEXT NOT NULL DEFAULT 'installed',
            manifest    JSONB,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
        )
        "#,
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    // Audit log table (append-only)
    sqlx::query(
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
        )
        "#,
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_audit_ts ON rootcx_system.audit_log (changed_at DESC)")
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_audit_table ON rootcx_system.audit_log (table_schema, table_name)")
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;

    // Generic audit trigger function
    sqlx::query(
        r#"
        CREATE OR REPLACE FUNCTION rootcx_system.audit_trigger_fn()
        RETURNS TRIGGER AS $$
        DECLARE
            rec_id TEXT;
        BEGIN
            IF TG_OP = 'DELETE' THEN
                rec_id := OLD.id::TEXT;
            ELSE
                rec_id := NEW.id::TEXT;
            END IF;

            INSERT INTO rootcx_system.audit_log
                (table_schema, table_name, record_id, operation, old_record, new_record)
            VALUES (
                TG_TABLE_SCHEMA,
                TG_TABLE_NAME,
                rec_id,
                TG_OP,
                CASE WHEN TG_OP IN ('UPDATE','DELETE') THEN to_jsonb(OLD) ELSE NULL END,
                CASE WHEN TG_OP IN ('INSERT','UPDATE') THEN to_jsonb(NEW) ELSE NULL END
            );

            IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
        END;
        $$ LANGUAGE plpgsql SECURITY DEFINER
        "#,
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    // Helper: attach audit trigger to a table
    sqlx::query(
        r#"
        CREATE OR REPLACE FUNCTION rootcx_system.enable_tracking(target_table REGCLASS)
        RETURNS VOID AS $$
        DECLARE
            trigger_name TEXT := 'audit_' || target_table::TEXT;
        BEGIN
            trigger_name := regexp_replace(trigger_name, '[^a-zA-Z0-9_]', '_', 'g');
            EXECUTE format(
                'CREATE OR REPLACE TRIGGER %I
                 AFTER INSERT OR UPDATE OR DELETE ON %s
                 FOR EACH ROW EXECUTE FUNCTION rootcx_system.audit_trigger_fn()',
                trigger_name, target_table::TEXT
            );
        END;
        $$ LANGUAGE plpgsql
        "#,
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    // Helper: detach audit trigger from a table
    sqlx::query(
        r#"
        CREATE OR REPLACE FUNCTION rootcx_system.disable_tracking(target_table REGCLASS)
        RETURNS VOID AS $$
        DECLARE
            trigger_name TEXT := 'audit_' || target_table::TEXT;
        BEGIN
            trigger_name := regexp_replace(trigger_name, '[^a-zA-Z0-9_]', '_', 'g');
            EXECUTE format('DROP TRIGGER IF EXISTS %I ON %s', trigger_name, target_table::TEXT);
        END;
        $$ LANGUAGE plpgsql
        "#,
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    info!("rootcx_system schema ready");
    Ok(())
}
