use crate::governance::execution::{ApprovalOwner, ExecutionOrigin};
use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

/// Pin the particular link delegation: unlinking and relinking cannot revive a run.
#[derive(Debug)]
pub(super) struct ChannelSession {
    channel: Uuid,
    chat: String,
    delegation: Uuid,
}

impl ChannelSession {
    pub(super) async fn load(
        pool: &PgPool,
        channel: &str,
        chat: &str,
        human: Uuid,
        app: &str,
    ) -> Result<Self, String> {
        let channel = channel.parse::<Uuid>().map_err(|e| e.to_string())?;
        let trigger = super::channel_delegation_ref(&channel.to_string(), human);
        let delegation = sqlx::query_scalar(
            "SELECT id FROM rootcx_system.delegations WHERE trigger_type = 'channel' AND trigger_ref = $1
             AND delegator_uid = $2 AND delegatee_uid = $3 AND revoked_at IS NULL
             AND (expires_at IS NULL OR expires_at > now())",
        ).bind(trigger).bind(human).bind(crate::extensions::agents::agent_user_id(app))
            .fetch_optional(pool).await.map_err(|e| e.to_string())?.ok_or("channel delegation missing")?;
        let session = Self {
            channel,
            chat: chat.into(),
            delegation,
        };
        session.validate(pool, human, app).await?;
        Ok(session)
    }

    pub(super) fn owner(&self, human: Uuid) -> ApprovalOwner {
        ApprovalOwner {
            user_id: human,
            origin: Some(self.key()),
        }
    }
}

#[async_trait]
impl ExecutionOrigin for ChannelSession {
    fn key(&self) -> String {
        serde_json::to_string(&(self.channel, &self.chat, self.delegation)).expect("channel origin")
    }

    async fn validate(&self, pool: &PgPool, human: Uuid, root_app: &str) -> Result<(), String> {
        crate::governance::triggers::fire_gate::assert_can_fire(pool, Some(human), root_app)
            .await
            .map_err(|e| e.to_string())?;
        let row: Option<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT c.provider, c.config FROM rootcx_system.channels c
             JOIN rootcx_system.channel_identities i ON i.channel_id = c.id
             JOIN rootcx_system.delegations d ON d.id = $4
             WHERE c.id = $1 AND c.status = 'active' AND i.external_chat_id = $2 AND i.user_id = $3
             AND d.delegator_uid = $3 AND d.delegatee_uid = $5 AND d.revoked_at IS NULL
             AND (d.expires_at IS NULL OR d.expires_at > now())",
        )
        .bind(self.channel)
        .bind(&self.chat)
        .bind(human)
        .bind(self.delegation)
        .bind(crate::extensions::agents::agent_user_id(root_app))
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
        let (name, config) = row.ok_or("channel identity or delegation revoked")?;
        let provider = super::provider_for(&name, &config).ok_or("unknown channel provider")?;
        provider
            .authorize_session(&config, &self.chat, human)
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::execution::{AgentAuthority, AgentSource};
    use std::sync::Arc;

    #[tokio::test]
    async fn channel_authority_is_bound_to_identity_and_the_original_delegation() {
        let pool = super::super::test_pool().await;
        let human = Uuid::new_v4();
        let app = format!("channel_{}", Uuid::new_v4().simple());
        let agent = crate::extensions::agents::agent_user_id(&app);
        let role = format!("channel-role-{human}");
        for (id, kind, owner) in [(human, "human", None), (agent, "agent", Some(human))] {
            sqlx::query("INSERT INTO rootcx_system.users (id,email,kind,owner_of_record) VALUES ($1,$2,$3,$4)")
                .bind(id).bind(format!("{id}@test.invalid")).bind(kind).bind(owner).execute(&pool).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO rootcx_system.rbac_roles (name,permissions) VALUES ($1,ARRAY['*'])",
        )
        .bind(&role)
        .execute(&pool)
        .await
        .unwrap();
        for uid in [human, agent] {
            sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id,role) VALUES ($1,$2)")
                .bind(uid)
                .bind(&role)
                .execute(&pool)
                .await
                .unwrap();
        }
        for provider in ["telegram", "slack"] {
            let channel = Uuid::new_v4();
            let chat = "authenticated-peer";
            sqlx::query("INSERT INTO rootcx_system.channels (id,provider,name,status) VALUES ($1,$2,'Test','active')")
                .bind(channel).bind(provider).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO rootcx_system.channel_identities (channel_id,external_chat_id,user_id) VALUES ($1,$2,$3)")
                .bind(channel).bind(chat).bind(human).execute(&pool).await.unwrap();
            let trigger =
                crate::extensions::channels::channel_delegation_ref(&channel.to_string(), human);
            let delegation = crate::governance::delegation::create(
                &pool,
                human,
                agent,
                "channel",
                Some(trigger),
            )
            .await
            .unwrap();
            let origin = ChannelSession::load(&pool, &channel.to_string(), chat, human, &app)
                .await
                .unwrap();
            let authority = AgentAuthority::admit(
                &pool,
                &app,
                Some(human),
                None,
                AgentSource::Origin(Arc::new(origin)),
            )
            .await
            .unwrap();
            assert!(authority.revalidate(&pool).await.is_ok(), "{provider}");
            assert!(
                ChannelSession::load(&pool, &channel.to_string(), "another-peer", human, &app)
                    .await
                    .is_err(),
                "{provider}: wrong sender"
            );
            assert!(
                ChannelSession::load(&pool, &channel.to_string(), chat, Uuid::new_v4(), &app)
                    .await
                    .is_err(),
                "{provider}: wrong identity"
            );
            sqlx::query("UPDATE rootcx_system.channels SET status='inactive' WHERE id=$1")
                .bind(channel)
                .execute(&pool)
                .await
                .unwrap();
            assert!(
                authority.revalidate(&pool).await.is_err(),
                "{provider}: inactive channel"
            );
            sqlx::query("UPDATE rootcx_system.channels SET status='active' WHERE id=$1")
                .bind(channel)
                .execute(&pool)
                .await
                .unwrap();
            crate::governance::delegation::revoke(&pool, delegation)
                .await
                .unwrap();
            assert!(
                authority.revalidate(&pool).await.is_err(),
                "{provider}: revoked delegation"
            );
            crate::governance::delegation::create(&pool, human, agent, "channel", Some(trigger))
                .await
                .unwrap();
            assert!(
                ChannelSession::load(&pool, &channel.to_string(), chat, human, &app)
                    .await
                    .is_ok(),
                "{provider}: fresh link"
            );
            assert!(
                authority.revalidate(&pool).await.is_err(),
                "{provider}: relinking cannot revive an old run"
            );
            sqlx::query("DELETE FROM rootcx_system.delegations WHERE trigger_ref=$1")
                .bind(trigger)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM rootcx_system.channels WHERE id=$1")
                .bind(channel)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE role=$1")
            .bind(&role)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM rootcx_system.rbac_roles WHERE name=$1")
            .bind(&role)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM rootcx_system.users WHERE id=ANY($1)")
            .bind(&[agent, human][..])
            .execute(&pool)
            .await
            .unwrap();
    }
}
