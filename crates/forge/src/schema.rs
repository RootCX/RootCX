use sqlx::SqlitePool;

use crate::error::ForgeError;

const DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS sessions (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL DEFAULT '',
        directory TEXT NOT NULL DEFAULT '',
        summary_message_id TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS messages (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
        error TEXT,
        created_at TEXT NOT NULL,
        completed_at TEXT
    )",
    "CREATE TABLE IF NOT EXISTS parts (
        id TEXT PRIMARY KEY,
        message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
        type TEXT NOT NULL CHECK (type IN ('text', 'reasoning', 'tool')),
        content TEXT NOT NULL DEFAULT '',
        tool_name TEXT,
        tool_state TEXT,
        tool_input TEXT,
        created_at TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id)",
    "CREATE INDEX IF NOT EXISTS idx_parts_message ON parts(message_id)",
];

pub async fn bootstrap(pool: &SqlitePool) -> Result<(), ForgeError> {
    sqlx::query("PRAGMA journal_mode=WAL").execute(pool).await?;
    sqlx::query("PRAGMA foreign_keys=ON").execute(pool).await?;
    for ddl in DDL {
        sqlx::query(ddl).execute(pool).await?;
    }
    Ok(())
}
