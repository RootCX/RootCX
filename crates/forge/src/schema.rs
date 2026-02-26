use sqlx::PgPool;

use crate::error::ForgeError;

const DDL: &[&str] = &[
    "CREATE SCHEMA IF NOT EXISTS forge",
    "CREATE TABLE IF NOT EXISTS forge.sessions (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        title TEXT NOT NULL DEFAULT '',
        directory TEXT NOT NULL DEFAULT '',
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
    )",
    "CREATE TABLE IF NOT EXISTS forge.messages (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        session_id UUID NOT NULL REFERENCES forge.sessions(id) ON DELETE CASCADE,
        role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
        error JSONB,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        completed_at TIMESTAMPTZ
    )",
    "CREATE TABLE IF NOT EXISTS forge.parts (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        message_id UUID NOT NULL REFERENCES forge.messages(id) ON DELETE CASCADE,
        type TEXT NOT NULL CHECK (type IN ('text', 'reasoning', 'tool')),
        content TEXT NOT NULL DEFAULT '',
        tool_name TEXT,
        tool_state JSONB,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now()
    )",
    "CREATE INDEX IF NOT EXISTS idx_forge_messages_session ON forge.messages(session_id)",
    "CREATE INDEX IF NOT EXISTS idx_forge_parts_message ON forge.parts(message_id)",
    "ALTER TABLE forge.sessions ADD COLUMN IF NOT EXISTS summary_message_id UUID REFERENCES forge.messages(id) ON DELETE SET NULL",
    "ALTER TABLE forge.parts ADD COLUMN IF NOT EXISTS tool_input JSONB",
];

pub async fn bootstrap(pool: &PgPool) -> Result<(), ForgeError> {
    for ddl in DDL {
        sqlx::query(ddl).execute(pool).await?;
    }
    Ok(())
}
