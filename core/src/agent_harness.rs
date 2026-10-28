//! Harnesses own reasoning; Core owns identity, persistence and business capabilities.
use crate::{governance::execution::AgentAuthority, RuntimeError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use std::path::PathBuf;

pub(crate) fn entry_point() -> Result<PathBuf, RuntimeError> {
    let path = std::env::var_os("ROOTCX_AGENT_HARNESS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("ROOTCX_RESOURCES")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"))
                .join("agent-harness/index.js")
        });
    if !path.is_file() {
        return Err(RuntimeError::Config(
            "managed agent harness is not installed".into(),
        ));
    }
    path.canonicalize()
        .map_err(|e| RuntimeError::Config(e.to_string()))
}

pub(crate) async fn admit_session(
    pool: &PgPool,
    authority: &AgentAuthority,
    session: &str,
) -> Result<PgConnection, RuntimeError> {
    let sid = session
        .parse::<uuid::Uuid>()
        .map_err(|_| RuntimeError::Invalid("invalid agent session ID".into()))?;
    crate::extensions::agents::persistence::ensure_session(
        pool,
        session,
        authority.app(),
        authority.human(),
    )
    .await
    .map_err(RuntimeError::Database)?;
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.agent_sessions s WHERE s.id=$1 AND s.app_id=$2 AND
         (s.user_id=$3 OR (s.user_id=$4 AND $5 AND EXISTS(
           SELECT 1 FROM rootcx_system.channel_sessions cs JOIN rootcx_system.channel_identities ci
           ON ci.channel_id=cs.channel_id AND ci.external_chat_id=cs.external_chat_id
           WHERE cs.session_id=s.id AND cs.app_id=s.app_id AND ci.user_id=$3))))"
    ).bind(sid).bind(authority.app()).bind(authority.human())
        .bind(crate::extensions::agents::agent_user_id(authority.app()))
        .bind(authority.owner().origin.is_some()).fetch_one(pool).await.map_err(RuntimeError::Database)?;
    if !owned {
        return Err(RuntimeError::PermissionDenied(
            "agent session belongs to another principal".into(),
        ));
    }
    // Detach the connection so long model calls cannot exhaust the SQL pool.
    // Closing it releases the advisory lock, including on cancellation/crash.
    let mut connection = pool
        .acquire()
        .await
        .map_err(RuntimeError::Database)?
        .detach();
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
        .bind(format!("agent-session:{sid}"))
        .fetch_one(&mut connection)
        .await
        .map_err(RuntimeError::Database)?;
    if !locked {
        return Err(RuntimeError::Conflict(
            "agent session already has an active turn".into(),
        ));
    }
    Ok(connection)
}

pub(crate) async fn state_op(
    pool: &PgPool,
    authority: &AgentAuthority,
    session: &str,
    op: &str,
    namespace: &str,
    key: &str,
    value: Value,
) -> Result<Value, String> {
    let mut permissions = authority.revalidate(pool).await?;
    permissions.sort();
    // Narrower task scopes and revoked grants cannot recover broader saved context.
    let partition = hex::encode(Sha256::digest(
        serde_json::to_vec(&(authority.partition_key(), permissions)).map_err(|e| e.to_string())?,
    ));
    if !matches!(namespace, "checkpoints" | "files" | "memory")
        || key.len() > 1024
        || value.to_string().len() > 600_000
    {
        return Err("invalid harness state request".into());
    }
    let scope = if namespace == "memory" { "" } else { session };
    let base = "app_id=$1 AND user_id=$2 AND authority=$3 AND session_id=$4 AND namespace=$5";
    let query = match op {
        "get" => format!("SELECT value FROM rootcx_system.agent_harness_state WHERE {base} AND key=$6"),
        "list" => format!("SELECT COALESCE(jsonb_agg(key ORDER BY key), '[]'::jsonb) FROM (SELECT key FROM rootcx_system.agent_harness_state WHERE {base} AND left(key, length($6))=$6 AND key > $7 ORDER BY key LIMIT 500) page"),
        "put" => "INSERT INTO rootcx_system.agent_harness_state(app_id,user_id,authority,session_id,namespace,key,value) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(app_id,user_id,authority,session_id,namespace,key) DO UPDATE SET value=EXCLUDED.value, updated_at=now() RETURNING 'true'::jsonb".into(),
        "delete" => format!("DELETE FROM rootcx_system.agent_harness_state WHERE {base} AND key=$6 RETURNING 'true'::jsonb"),
        _ => return Err("unsupported harness state operation".into()),
    };
    let mut q = sqlx::query_scalar::<_, Value>(&query)
        .bind(authority.app())
        .bind(authority.human())
        .bind(partition)
        .bind(scope)
        .bind(namespace)
        .bind(key);
    if op == "put" {
        q = q.bind(value);
    } else if op == "list" {
        q = q.bind(value.as_str().unwrap_or(""));
    }
    q.fetch_optional(pool)
        .await
        .map(|v| v.unwrap_or(json!(null)))
        .map_err(|e| e.to_string())
}
