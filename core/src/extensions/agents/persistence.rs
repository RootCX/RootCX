use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) struct PersistCtx {
    pub pool: PgPool,
    pub app_id: String,
    pub session_id: String,
    pub user_id: Uuid,
    pub user_message: String,
}

pub(crate) async fn ensure_session(pool: &PgPool, session_id: &str, app_id: &str, user_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO rootcx_system.agent_sessions (id, app_id, user_id)
         VALUES ($1::uuid, $2, $3)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(session_id).bind(app_id).bind(user_id)
    .execute(pool).await?;
    Ok(())
}

pub(crate) async fn persist_message(
    pool: &PgPool, session_id: &str, role: &str, content: &str,
    token_count: Option<i32>, is_summary: bool,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO rootcx_system.agent_messages (session_id, role, content, token_count, is_summary)
         VALUES ($1::uuid, $2, $3, $4, $5)
         RETURNING id",
    )
    .bind(session_id).bind(role).bind(content)
    .bind(token_count.unwrap_or(0)).bind(is_summary)
    .fetch_one(pool).await
}

pub(crate) async fn persist_tool_call_start(
    pool: &PgPool, session_id: &str, call_id: &str, tool_name: &str, input: &JsonValue,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO rootcx_system.agent_tool_calls (id, session_id, tool_name, input, status)
         VALUES ($1::uuid, $2::uuid, $3, $4, 'running')
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(call_id).bind(session_id).bind(tool_name).bind(input)
    .execute(pool).await?;
    Ok(())
}

pub(crate) async fn persist_tool_call_end(
    pool: &PgPool, call_id: &str, output: Option<&JsonValue>, error: Option<&str>, duration_ms: u64,
) -> Result<(), sqlx::Error> {
    let status = if error.is_some() { "failed" } else { "completed" };
    sqlx::query(
        "UPDATE rootcx_system.agent_tool_calls
         SET output = $2, error = $3, status = $4, duration_ms = $5
         WHERE id = $1::uuid",
    )
    .bind(call_id).bind(output).bind(error).bind(status).bind(duration_ms as i32)
    .execute(pool).await?;
    Ok(())
}

/// Mark session complete: persist assistant message + bump turn_count
pub(crate) async fn finalize_session(
    pool: &PgPool, session_id: &str, user_message: &str,
    assistant_response: &str, tokens: Option<u64>,
) -> Result<(), sqlx::Error> {
    persist_message(pool, session_id, "assistant", assistant_response, tokens.map(|t| t as i32), false).await?;
    let msgs = json!([
        {"role": "user", "content": user_message},
        {"role": "assistant", "content": assistant_response}
    ]);
    sqlx::query(
        "UPDATE rootcx_system.agent_sessions SET
            messages = agent_sessions.messages || $2,
            total_tokens = COALESCE(agent_sessions.total_tokens, 0) + $3,
            turn_count = COALESCE(agent_sessions.turn_count, 0) + 1,
            updated_at = now()
         WHERE id = $1::uuid",
    )
    .bind(session_id).bind(&msgs).bind(tokens.unwrap_or(0) as i64)
    .execute(pool).await?;
    Ok(())
}

pub(crate) async fn persist_session(
    pctx: &PersistCtx, assistant_response: &str, tokens: Option<u64>,
) -> Result<(), sqlx::Error> {
    ensure_session(&pctx.pool, &pctx.session_id, &pctx.app_id, pctx.user_id).await?;
    persist_message(&pctx.pool, &pctx.session_id, "user", &pctx.user_message, None, false).await?;
    finalize_session(&pctx.pool, &pctx.session_id, &pctx.user_message, assistant_response, tokens).await
}
