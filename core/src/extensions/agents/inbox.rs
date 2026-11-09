//! Messages a person sends while their agent turn runs. The harness reads them
//! between steps; a final read that finds nothing closes the turn in the same
//! transaction, so a later message is never accepted by a turn that will not read it.
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredFile {
    pub file_id: Uuid,
    pub name: String,
    pub content_type: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Queued {
    pub id: String,
    pub content: String,
    pub files: Vec<StoredFile>,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Drained {
    pub messages: Vec<Queued>,
    pub stop: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Push {
    Accepted,
    Duplicate,
    NotRunning,
}

pub(crate) const DDL: [&str; 3] = [
    "CREATE TABLE IF NOT EXISTS rootcx_system.agent_inbox_runs (
        session_id UUID PRIMARY KEY,
        invoke_id TEXT NOT NULL UNIQUE,
        stop_requested BOOLEAN NOT NULL DEFAULT false,
        opened_at TIMESTAMPTZ NOT NULL DEFAULT now()
    )",
    "CREATE TABLE IF NOT EXISTS rootcx_system.agent_inbox (
        session_id UUID NOT NULL,
        message_id TEXT NOT NULL,
        content TEXT NOT NULL,
        files JSONB NOT NULL DEFAULT '[]',
        delivered_invoke TEXT,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        PRIMARY KEY (session_id, message_id)
    )",
    // Workers do not survive a Core restart, so no turn can still read its inbox.
    "DELETE FROM rootcx_system.agent_inbox_runs",
];

/// Opens the inbox when the app's agent runs on the built-in harness; false otherwise.
pub(crate) async fn open_for(pool: &PgPool, app_id: &str, session: &str, invoke: &str) -> sqlx::Result<bool> {
    let Ok(session) = session.parse::<Uuid>() else { return Ok(false) };
    let managed = sqlx::query_scalar::<_, Option<bool>>(
        "SELECT config #>> '{harness,type}' = 'deepagents' FROM rootcx_system.agents WHERE app_id=$1",
    ).bind(app_id).fetch_optional(pool).await?.flatten();
    if managed != Some(true) { return Ok(false); }
    open(pool, session, invoke).await?;
    Ok(true)
}

/// Called once the turn holds its session lock.
pub(crate) async fn open(pool: &PgPool, session: Uuid, invoke: &str) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM rootcx_system.agent_inbox WHERE session_id=$1
        AND (delivered_invoke IS NULL OR created_at < now() - interval '1 hour')")
        .bind(session).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO rootcx_system.agent_inbox_runs(session_id, invoke_id) VALUES($1,$2)
        ON CONFLICT (session_id) DO UPDATE SET invoke_id=EXCLUDED.invoke_id, stop_requested=false, opened_at=now()")
        .bind(session).bind(invoke).execute(&mut *tx).await?;
    tx.commit().await
}

pub(crate) async fn push(
    pool: &PgPool, session: Uuid, message_id: &str, content: &str, files: &[StoredFile],
) -> sqlx::Result<Push> {
    let mut tx = pool.begin().await?;
    let stopping: Option<bool> = sqlx::query_scalar(
        "SELECT stop_requested FROM rootcx_system.agent_inbox_runs WHERE session_id=$1 FOR UPDATE",
    ).bind(session).fetch_optional(&mut *tx).await?;
    if stopping != Some(false) { return Ok(Push::NotRunning); }
    let inserted = sqlx::query("INSERT INTO rootcx_system.agent_inbox(session_id, message_id, content, files)
        VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
        .bind(session).bind(message_id).bind(content).bind(sqlx::types::Json(files))
        .execute(&mut *tx).await?.rows_affected();
    tx.commit().await?;
    Ok(if inserted == 0 { Push::Duplicate } else { Push::Accepted })
}

pub(crate) async fn is_open(pool: &PgPool, session: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.agent_inbox_runs WHERE session_id=$1 AND NOT stop_requested)")
        .bind(session).fetch_one(pool).await
}

/// True when a running turn will stop at its next step.
pub(crate) async fn stop(pool: &PgPool, session: Uuid) -> sqlx::Result<bool> {
    let updated = sqlx::query("UPDATE rootcx_system.agent_inbox_runs SET stop_requested=true WHERE session_id=$1")
        .bind(session).execute(pool).await?.rows_affected();
    Ok(updated > 0)
}

pub(crate) async fn drain(pool: &PgPool, session: Uuid, invoke: &str, final_: bool) -> sqlx::Result<Drained> {
    let mut tx = pool.begin().await?;
    let Some(stop) = sqlx::query_scalar::<_, bool>(
        "SELECT stop_requested FROM rootcx_system.agent_inbox_runs WHERE session_id=$1 AND invoke_id=$2 FOR UPDATE",
    ).bind(session).bind(invoke).fetch_optional(&mut *tx).await? else { return Ok(Drained::default()) };
    type Row = (chrono::DateTime<chrono::Utc>, String, String, sqlx::types::Json<Vec<StoredFile>>);
    let mut rows: Vec<Row> = sqlx::query_as(
        "UPDATE rootcx_system.agent_inbox SET delivered_invoke=$2
         WHERE session_id=$1 AND delivered_invoke IS NULL
         RETURNING created_at, message_id, content, files",
    ).bind(session).bind(invoke).fetch_all(&mut *tx).await?;
    if stop || (final_ && rows.is_empty()) {
        sqlx::query("DELETE FROM rootcx_system.agent_inbox_runs WHERE session_id=$1")
            .bind(session).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    // RETURNING has no order; the person's order is arrival order.
    rows.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let messages = rows.into_iter().map(|(_, id, content, files)| Queued { id, content, files: files.0 }).collect();
    Ok(Drained { messages, stop })
}

/// The harness's read: fresh download links, since a message can wait out a long action.
pub(crate) async fn drain_for_harness(
    pool: &PgPool, nonces: &std::sync::Mutex<crate::extensions::storage::nonce::NonceStore>, runtime_url: &str,
    app_id: &str, session: &str, invoke: &str, final_: bool,
) -> Result<serde_json::Value, String> {
    let session = session.parse::<Uuid>().map_err(|_| "invalid agent session")?;
    let drained = drain(pool, session, invoke, final_).await.map_err(|e| e.to_string())?;
    let mut nonces = nonces.lock().unwrap_or_else(|e| e.into_inner());
    let messages: Vec<_> = drained.messages.into_iter().map(|m| serde_json::json!({
        "id": m.id, "content": m.content,
        "files": m.files.into_iter().map(|f| serde_json::json!({
            "name": f.name, "content_type": f.content_type, "file_id": f.file_id,
            "url": crate::extensions::storage::download_url(runtime_url, &nonces.create_download(f.file_id, app_id)),
        })).collect::<Vec<_>>(),
    })).collect();
    Ok(serde_json::json!({ "messages": messages, "stop": drained.stop }))
}

/// Ends the turn however its task ended. Unread messages followed that request.
pub(crate) async fn close(pool: &PgPool, invoke: &str) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM rootcx_system.agent_inbox WHERE delivered_invoke IS NULL AND session_id IN
        (SELECT session_id FROM rootcx_system.agent_inbox_runs WHERE invoke_id=$1)")
        .bind(invoke).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM rootcx_system.agent_inbox_runs WHERE invoke_id=$1")
        .bind(invoke).execute(&mut *tx).await?;
    tx.commit().await
}

/// What this turn read, oldest first, for the conversation transcript.
pub(crate) async fn delivered(pool: &PgPool, invoke: &str) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar("SELECT content FROM rootcx_system.agent_inbox WHERE delivered_invoke=$1 ORDER BY created_at, message_id")
        .bind(invoke).fetch_all(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> PgPool {
        static READY: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
        let pool = crate::extensions::test_db::pool().await;
        READY.get_or_init(|| async {
            for ddl in &DDL[..2] { sqlx::query(ddl).execute(&pool).await.unwrap(); }
        }).await;
        pool
    }
    async fn running() -> (PgPool, Uuid, String) {
        let (pool, session, turn) = (pool().await, Uuid::new_v4(), Uuid::new_v4().to_string());
        open(&pool, session, &turn).await.unwrap();
        (pool, session, turn)
    }
    fn ids(d: &Drained) -> Vec<&str> { d.messages.iter().map(|m| m.id.as_str()).collect() }

    #[tokio::test]
    async fn a_final_read_closes_the_turn_only_when_nothing_waits() {
        let (pool, session, turn) = running().await;
        push(&pool, session, "m1", "15 m²", &[]).await.unwrap();
        assert_eq!(ids(&drain(&pool, session, &turn, true).await.unwrap()), ["m1"]);
        assert_eq!(drain(&pool, session, &turn, true).await.unwrap(), Drained::default());
        assert_eq!(push(&pool, session, "m2", "trop tard", &[]).await.unwrap(), Push::NotRunning);
    }

    #[tokio::test]
    async fn concurrent_messages_are_each_read_once() {
        let (pool, session, turn) = running().await;
        let names: Vec<String> = (0..8).map(|i| format!("m{i}")).collect();
        let (sent, read) = tokio::join!(
            futures::future::join_all(names.iter().map(|id| push(&pool, session, id, "x", &[]))),
            futures::future::join_all((0..4).map(|_| drain(&pool, session, &turn, false))),
        );
        assert!(sent.iter().all(|r| *r.as_ref().unwrap() == Push::Accepted));
        let mut seen: Vec<String> = read.into_iter().flat_map(|r| r.unwrap().messages).map(|m| m.id).collect();
        seen.extend(drain(&pool, session, &turn, true).await.unwrap().messages.into_iter().map(|m| m.id));
        seen.sort();
        assert_eq!(seen, names);
    }

    #[tokio::test]
    async fn stop_is_read_once_with_waiting_messages_and_closes_the_turn() {
        let (pool, session, turn) = running().await;
        push(&pool, session, "m1", "attends", &[]).await.unwrap();
        assert!(stop(&pool, session).await.unwrap());
        assert_eq!(push(&pool, session, "m2", "après stop", &[]).await.unwrap(), Push::NotRunning);
        let drained = drain(&pool, session, &turn, false).await.unwrap();
        assert!(drained.stop);
        assert_eq!(ids(&drained), ["m1"]);
        assert!(!stop(&pool, session).await.unwrap());
    }

    #[tokio::test]
    async fn close_drops_unread_messages_and_keeps_what_the_turn_read() {
        let (pool, session, turn) = running().await;
        push(&pool, session, "read", "15 m²", &[]).await.unwrap();
        drain(&pool, session, &turn, false).await.unwrap();
        push(&pool, session, "unread", "peinture", &[]).await.unwrap();
        close(&pool, &turn).await.unwrap();
        assert_eq!(push(&pool, session, "after", "x", &[]).await.unwrap(), Push::NotRunning);
        assert_eq!(delivered(&pool, &turn).await.unwrap(), ["15 m²"]);
    }
}
