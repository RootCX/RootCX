use sqlx::PgPool;
use uuid::Uuid;

use crate::RuntimeError;

fn err(e: sqlx::Error) -> RuntimeError { RuntimeError::Schema(e) }

pub async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::query("CREATE EXTENSION IF NOT EXISTS \"uuid-ossp\"")
        .execute(pool).await.map_err(err)?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS rootcx_system.delegations (
            id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            delegator_uid UUID NOT NULL,
            agent_uid     UUID NOT NULL,
            trigger_type  TEXT NOT NULL CHECK (trigger_type IN ('cron', 'hook', 'webhook', 'manual')),
            trigger_ref   UUID,
            created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
            expires_at    TIMESTAMPTZ,
            revoked_at    TIMESTAMPTZ
        )
    "#).execute(pool).await.map_err(err)?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_delegations_lookup \
         ON rootcx_system.delegations (delegator_uid, agent_uid) WHERE revoked_at IS NULL"
    ).execute(pool).await.map_err(err)?;

    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_delegations_trigger \
         ON rootcx_system.delegations (trigger_type, trigger_ref) WHERE revoked_at IS NULL AND trigger_ref IS NOT NULL"
    ).execute(pool).await.map_err(err)?;

    migrate_existing_crons(pool).await?;
    migrate_existing_hooks(pool).await?;

    Ok(())
}

pub async fn is_valid(pool: &PgPool, delegator: Uuid, agent: Uuid) -> Result<bool, RuntimeError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.delegations \
         WHERE delegator_uid = $1 AND agent_uid = $2 \
         AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now()))"
    ).bind(delegator).bind(agent).fetch_one(pool).await.map_err(err)
}

pub async fn create(
    pool: &PgPool, delegator: Uuid, agent: Uuid, trigger_type: &str, trigger_ref: Option<Uuid>,
) -> Result<Uuid, RuntimeError> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO rootcx_system.delegations (delegator_uid, agent_uid, trigger_type, trigger_ref) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (trigger_type, trigger_ref) WHERE revoked_at IS NULL AND trigger_ref IS NOT NULL \
         DO UPDATE SET delegator_uid = EXCLUDED.delegator_uid \
         RETURNING id"
    ).bind(delegator).bind(agent).bind(trigger_type).bind(trigger_ref)
    .fetch_one(pool).await.map_err(err)?;
    Ok(id)
}

#[allow(dead_code)]
pub async fn revoke(pool: &PgPool, delegation_id: Uuid) -> Result<(), RuntimeError> {
    sqlx::query("UPDATE rootcx_system.delegations SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(delegation_id).execute(pool).await.map_err(err)?;
    Ok(())
}

#[allow(dead_code)]
pub async fn revoke_by_trigger(pool: &PgPool, trigger_type: &str, trigger_ref: Uuid) -> Result<(), RuntimeError> {
    sqlx::query(
        "UPDATE rootcx_system.delegations SET revoked_at = now() \
         WHERE trigger_type = $1 AND trigger_ref = $2 AND revoked_at IS NULL"
    ).bind(trigger_type).bind(trigger_ref).execute(pool).await.map_err(err)?;
    Ok(())
}

async fn migrate_existing_crons(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::query(r#"
        INSERT INTO rootcx_system.delegations (delegator_uid, agent_uid, trigger_type, trigger_ref)
        SELECT cs.created_by,
               uuid_generate_v5('9a3b4c5d-6e7f-4001-8293-a4b5c6d7e8f9'::uuid, 'agent:' || cs.app_id),
               'cron', cs.id
        FROM rootcx_system.cron_schedules cs
        WHERE cs.created_by IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM rootcx_system.delegations d
              WHERE d.trigger_type = 'cron' AND d.trigger_ref = cs.id AND d.revoked_at IS NULL
          )
    "#).execute(pool).await.map_err(err)?;
    Ok(())
}

async fn migrate_existing_hooks(pool: &PgPool) -> Result<(), RuntimeError> {
    let has_column: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM information_schema.columns \
         WHERE table_schema = 'rootcx_system' AND table_name = 'entity_hooks' AND column_name = 'created_by')"
    ).fetch_one(pool).await.map_err(err)?;
    if !has_column { return Ok(()); }

    sqlx::query(r#"
        INSERT INTO rootcx_system.delegations (delegator_uid, agent_uid, trigger_type, trigger_ref)
        SELECT h.created_by,
               uuid_generate_v5('9a3b4c5d-6e7f-4001-8293-a4b5c6d7e8f9'::uuid,
                   'agent:' || COALESCE(h.action_config->>'app_id', h.app_id)),
               'hook', h.id
        FROM rootcx_system.entity_hooks h
        WHERE h.created_by IS NOT NULL
          AND h.action_type = 'agent'
          AND NOT EXISTS (
              SELECT 1 FROM rootcx_system.delegations d
              WHERE d.trigger_type = 'hook' AND d.trigger_ref = h.id AND d.revoked_at IS NULL
          )
    "#).execute(pool).await.map_err(err)?;
    Ok(())
}
