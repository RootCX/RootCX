use sqlx::PgPool;
use uuid::Uuid;

pub(super) enum Admission {
    Ready,
    Duplicate(String),
    Busy,
}

pub(super) async fn bootstrap(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rootcx_system.channel_receipts (
        channel_id UUID NOT NULL REFERENCES rootcx_system.channels(id) ON DELETE CASCADE,
        delivery_id TEXT NOT NULL, chat_id TEXT, is_message BOOLEAN NOT NULL,
        status TEXT NOT NULL DEFAULT 'processing', received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        PRIMARY KEY (channel_id, delivery_id)
    )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS channel_receipts_running_chat
        ON rootcx_system.channel_receipts(channel_id, chat_id)
        WHERE is_message AND status = 'processing'",
    )
    .execute(pool)
    .await?;
    // Unknown-outcome legacy receipts must remain non-replayable after upgrading.
    sqlx::query("INSERT INTO rootcx_system.channel_receipts(channel_id, delivery_id, is_message, status, received_at)
        SELECT channel_id, message_id, false, status, received_at FROM rootcx_system.whatsapp_receipts
        ON CONFLICT DO NOTHING").execute(pool).await?;
    Ok(())
}

pub(super) async fn admit(
    pool: &PgPool,
    channel: Uuid,
    delivery: &str,
    chat: &str,
    is_message: bool,
) -> Result<Admission, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // This lock lasts only through admission; LLM waits never reserve a pool connection.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("channel-intake:{channel}:{chat}"))
        .execute(&mut *tx)
        .await?;
    if let Some(status) = sqlx::query_scalar::<_, String>(
        "SELECT status FROM rootcx_system.channel_receipts WHERE channel_id=$1 AND delivery_id=$2",
    )
    .bind(channel)
    .bind(delivery)
    .fetch_optional(&mut *tx)
    .await?
    {
        return Ok(Admission::Duplicate(status));
    }
    if is_message
        && sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM rootcx_system.channel_receipts
         WHERE channel_id=$1 AND chat_id=$2 AND is_message AND status='processing')",
        )
        .bind(channel)
        .bind(chat)
        .fetch_one(&mut *tx)
        .await?
    {
        return Ok(Admission::Busy);
    }
    let inserted = sqlx::query(
        "INSERT INTO rootcx_system.channel_receipts(channel_id,delivery_id,chat_id,is_message)
        VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING",
    )
    .bind(channel)
    .bind(delivery)
    .bind(chat)
    .bind(is_message)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    if inserted == 0 {
        let status = sqlx::query_scalar("SELECT status FROM rootcx_system.channel_receipts WHERE channel_id=$1 AND delivery_id=$2")
            .bind(channel).bind(delivery).fetch_one(pool).await?;
        return Ok(Admission::Duplicate(status));
    }
    Ok(Admission::Ready)
}

pub(super) async fn finish(
    pool: &PgPool,
    channel: Uuid,
    delivery: &str,
    succeeded: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE rootcx_system.channel_receipts SET status=$3
        WHERE channel_id=$1 AND delivery_id=$2 AND status='processing'",
    )
    .bind(channel)
    .bind(delivery)
    .bind(if succeeded { "done" } else { "failed" })
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delivery_admission_survives_duplicates_and_keeps_confirmation_paths_open() {
        let pool = super::super::test_pool().await;
        let channels = [Uuid::new_v4(), Uuid::new_v4()];
        for channel in channels {
            sqlx::query("INSERT INTO rootcx_system.channels(id,provider,name) VALUES($1,'telegram','Intake test')")
                .bind(channel).execute(&pool).await.unwrap();
        }
        let (a, b) = tokio::join!(
            admit(&pool, channels[0], "same", "chat", true),
            admit(&pool, channels[0], "same", "chat", true)
        );
        assert_eq!(
            [a.unwrap(), b.unwrap()]
                .iter()
                .filter(|a| matches!(a, Admission::Ready))
                .count(),
            1
        );
        assert!(matches!(
            admit(&pool, channels[0], "second", "chat", true)
                .await
                .unwrap(),
            Admission::Busy
        ));
        assert!(matches!(
            admit(&pool, channels[0], "approve", "chat", false)
                .await
                .unwrap(),
            Admission::Ready
        ));
        assert!(matches!(
            admit(&pool, channels[1], "same", "chat", true)
                .await
                .unwrap(),
            Admission::Ready
        ));
        assert!(matches!(
            admit(&pool, channels[0], "other", "other-chat", true)
                .await
                .unwrap(),
            Admission::Ready
        ));
        finish(&pool, channels[0], "same", false).await.unwrap();
        assert!(
            matches!(admit(&pool, channels[0], "same", "chat", true).await.unwrap(), Admission::Duplicate(s) if s == "failed")
        );
        assert!(matches!(
            admit(&pool, channels[0], "second", "chat", true)
                .await
                .unwrap(),
            Admission::Ready
        ));
        for channel in channels {
            sqlx::query("DELETE FROM rootcx_system.channels WHERE id=$1")
                .bind(channel)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
}
