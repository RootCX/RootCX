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

    Ok(())
}

/// Backfill delegations for existing triggers. Must run AFTER extension bootstrap
/// has added `created_by` columns to crons/hooks/webhooks tables.
pub async fn migrate_existing_triggers(pool: &PgPool) -> Result<(), RuntimeError> {
    let fallback_admin = resolve_primary_admin(pool).await;
    migrate_existing_crons(pool, fallback_admin).await?;
    migrate_existing_hooks(pool, fallback_admin).await?;
    migrate_existing_webhooks(pool, fallback_admin).await?;
    Ok(())
}

async fn resolve_primary_admin(pool: &PgPool) -> Option<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM rootcx_system.rbac_assignments \
         WHERE role = 'admin' \
         ORDER BY assigned_at ASC LIMIT 1"
    ).fetch_optional(pool).await.ok().flatten()
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

async fn migrate_existing_crons(pool: &PgPool, fallback_admin: Option<Uuid>) -> Result<(), RuntimeError> {
    // Backfill created_by for orphan crons (pre-upgrade rows)
    if let Some(admin) = fallback_admin {
        sqlx::query("UPDATE rootcx_system.cron_schedules SET created_by = $1 WHERE created_by IS NULL")
            .bind(admin).execute(pool).await.map_err(err)?;
    }

    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT cs.id, cs.app_id FROM rootcx_system.cron_schedules cs \
         WHERE cs.created_by IS NOT NULL \
           AND EXISTS(SELECT 1 FROM rootcx_system.agents WHERE app_id = cs.app_id) \
           AND NOT EXISTS( \
               SELECT 1 FROM rootcx_system.delegations d \
               WHERE d.trigger_type = 'cron' AND d.trigger_ref = cs.id AND d.revoked_at IS NULL)"
    ).fetch_all(pool).await.map_err(err)?;

    for (cron_id, app_id) in rows {
        let agent_uid = crate::extensions::agents::agent_user_id(&app_id);
        let delegator: Option<Uuid> = sqlx::query_scalar(
            "SELECT created_by FROM rootcx_system.cron_schedules WHERE id = $1"
        ).bind(cron_id).fetch_optional(pool).await.map_err(err)?.flatten();
        if let Some(d) = delegator {
            let _ = create(pool, d, agent_uid, "cron", Some(cron_id)).await;
        }
    }
    Ok(())
}

async fn migrate_existing_hooks(pool: &PgPool, fallback_admin: Option<Uuid>) -> Result<(), RuntimeError> {
    // Backfill created_by for orphan hooks
    if let Some(admin) = fallback_admin {
        sqlx::query("UPDATE rootcx_system.entity_hooks SET created_by = $1 WHERE created_by IS NULL AND action_type = 'agent'")
            .bind(admin).execute(pool).await.map_err(err)?;
    }

    let rows: Vec<(Uuid, String, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT h.id, h.app_id, h.action_config FROM rootcx_system.entity_hooks h \
         WHERE h.created_by IS NOT NULL AND h.action_type = 'agent' \
           AND NOT EXISTS( \
               SELECT 1 FROM rootcx_system.delegations d \
               WHERE d.trigger_type = 'hook' AND d.trigger_ref = h.id AND d.revoked_at IS NULL)"
    ).fetch_all(pool).await.map_err(err)?;

    for (hook_id, app_id, config) in rows {
        let target = config.as_ref()
            .and_then(|c| c.get("app_id")).and_then(|v| v.as_str())
            .unwrap_or(&app_id);
        let agent_uid = crate::extensions::agents::agent_user_id(target);
        let delegator: Option<Uuid> = sqlx::query_scalar(
            "SELECT created_by FROM rootcx_system.entity_hooks WHERE id = $1"
        ).bind(hook_id).fetch_optional(pool).await.map_err(err)?.flatten();
        if let Some(d) = delegator {
            let _ = create(pool, d, agent_uid, "hook", Some(hook_id)).await;
        }
    }
    Ok(())
}

async fn migrate_existing_webhooks(pool: &PgPool, fallback_admin: Option<Uuid>) -> Result<(), RuntimeError> {
    // Backfill created_by for orphan webhooks on agent apps
    if let Some(admin) = fallback_admin {
        sqlx::query(
            "UPDATE rootcx_system.webhooks SET created_by = $1 \
             WHERE created_by IS NULL \
               AND EXISTS(SELECT 1 FROM rootcx_system.agents WHERE app_id = rootcx_system.webhooks.app_id)"
        ).bind(admin).execute(pool).await.map_err(err)?;
    }

    // Migrate legacy webhooks on agent apps to the "agent" method convention.
    // The old code routed ALL webhooks on agent-apps to the agent; preserve that behavior.
    sqlx::query(
        "UPDATE rootcx_system.webhooks SET method = 'agent' \
         WHERE method != 'agent' \
           AND EXISTS(SELECT 1 FROM rootcx_system.agents WHERE app_id = rootcx_system.webhooks.app_id)"
    ).execute(pool).await.map_err(err)?;

    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT w.id, w.app_id FROM rootcx_system.webhooks w \
         WHERE w.created_by IS NOT NULL \
           AND EXISTS(SELECT 1 FROM rootcx_system.agents WHERE app_id = w.app_id) \
           AND NOT EXISTS( \
               SELECT 1 FROM rootcx_system.delegations d \
               WHERE d.trigger_type = 'webhook' AND d.trigger_ref = w.id AND d.revoked_at IS NULL)"
    ).fetch_all(pool).await.map_err(err)?;

    for (wh_id, app_id) in rows {
        let agent_uid = crate::extensions::agents::agent_user_id(&app_id);
        let delegator: Option<Uuid> = sqlx::query_scalar(
            "SELECT created_by FROM rootcx_system.webhooks WHERE id = $1"
        ).bind(wh_id).fetch_optional(pool).await.map_err(err)?.flatten();
        if let Some(d) = delegator {
            let _ = create(pool, d, agent_uid, "webhook", Some(wh_id)).await;
        }
    }
    Ok(())
}
