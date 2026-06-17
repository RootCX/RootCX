//! Resolving a worker's RLS identity (`RpcCaller`) from a user id.
//!
//! One source of truth for "given a user, what identity does the core pose for
//! a unit of work run on their behalf". Disabled (or unknown) principals
//! resolve to `None` — deny-by-default: a caller must refuse rather than fall
//! back to an identity-less worker, which RLS treats as anonymous (every row
//! denied).

use sqlx::PgPool;
use uuid::Uuid;

use crate::ipc::RpcCaller;

/// Resolve `user_id` to a direct (non-delegated) caller acting with their own
/// full authority. `None` if the principal is disabled or unknown.
pub async fn resolve_caller(pool: &PgPool, user_id: Uuid) -> Option<RpcCaller> {
    resolve_caller_inheriting(pool, user_id, None).await
}

/// As [`resolve_caller`], but `inherit` carries a delegated context's frozen
/// effective permissions down to the target: authority is monotone — a
/// delegated caller never re-widens to the user's full set. `None` → direct.
pub async fn resolve_caller_inheriting(
    pool: &PgPool, user_id: Uuid, inherit: Option<&[String]>,
) -> Option<RpcCaller> {
    let (email,): (String,) = sqlx::query_as(
        "SELECT email FROM rootcx_system.users WHERE id = $1 AND disabled_at IS NULL")
        .bind(user_id).fetch_optional(pool).await.ok()??;
    Some(RpcCaller { user_id: user_id.to_string(), email, effective_perms: inherit.map(<[String]>::to_vec) })
}
