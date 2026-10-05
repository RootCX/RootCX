//! Live agent authority stays in Core; transports only supply their origin check.
use super::authority::{has_permission, intersect_permissions, resolve_permissions};
use crate::auth::identity::principal_enabled;
use async_trait::async_trait;
use sqlx::PgPool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalOwner {
    pub user_id: Uuid,
    pub origin: Option<String>,
}

#[async_trait]
pub trait ExecutionOrigin: std::fmt::Debug + Send + Sync {
    /// Stable identity separating independently revocable origins, never a permission grant.
    fn key(&self) -> String;
    async fn validate(&self, pool: &PgPool, human: Uuid, root_app: &str) -> Result<(), String>;
}

#[derive(Default)]
pub enum AgentSource {
    #[default]
    Direct,
    Origin(Arc<dyn ExecutionOrigin>),
    Child(AgentAuthority),
}

#[derive(Clone, Debug)]
pub struct AgentAuthority {
    human: Uuid,
    agents: Vec<String>,
    ceiling: Vec<String>,
    task_scope: Option<Vec<String>>,
    origin: Option<Arc<dyn ExecutionOrigin>>,
    lifetime: CancellationToken,
}

impl AgentAuthority {
    pub(crate) async fn admit(
        pool: &PgPool,
        app: &str,
        human: Option<Uuid>,
        scope: Option<Vec<String>>,
        source: AgentSource,
    ) -> Result<Self, String> {
        let human = human.ok_or("agent execution requires a responsible human")?;
        let (mut agents, mut ceiling, origin, lifetime, task_scope) = match source {
            AgentSource::Child(parent) => {
                if parent.human != human {
                    return Err("delegation identity mismatch".into());
                }
                if parent.agents.iter().any(|id| id == app) {
                    return Err("agent delegation cycle".into());
                }
                let ceiling = parent.revalidate(pool).await?;
                (
                    parent.agents,
                    ceiling,
                    parent.origin,
                    parent.lifetime.child_token(),
                    parent.task_scope,
                )
            }
            source => {
                if !principal_enabled(pool, human).await {
                    return Err("responsible human disabled or missing".into());
                }
                let ceiling = resolve_permissions(pool, human)
                    .await
                    .map_err(|e| format!("{e:?}"))?
                    .1;
                let origin = match source {
                    AgentSource::Origin(origin) => Some(origin),
                    _ => None,
                };
                (vec![], ceiling, origin, CancellationToken::new(), scope)
            }
        };
        if !has_permission(&ceiling, &format!("app:{app}:invoke")) {
            return Err(format!("permission denied: app:{app}:invoke"));
        }
        let agent = crate::extensions::agents::agent_user_id(app);
        if !principal_enabled(pool, agent).await {
            return Err("agent disabled or missing".into());
        }
        let current = resolve_permissions(pool, agent)
            .await
            .map_err(|e| format!("{e:?}"))?
            .1;
        ceiling = intersect_permissions(&ceiling, &current);
        if let Some(scope) = &task_scope {
            ceiling = intersect_permissions(&ceiling, scope);
        }
        agents.push(app.into());
        let authority = Self {
            human,
            agents,
            ceiling,
            task_scope,
            origin,
            lifetime,
        };
        if let Some(origin) = &authority.origin {
            origin.validate(pool, human, authority.root_app()).await?;
        }
        authority.ensure_active()?;
        Ok(authority)
    }

    /// Re-read every principal while retaining the original ceiling. New grants
    /// never enlarge an admitted run, including after a confirmation wait.
    pub(crate) async fn revalidate(&self, pool: &PgPool) -> Result<Vec<String>, String> {
        self.ensure_active()?;
        if !principal_enabled(pool, self.human).await {
            return Err("responsible human disabled or missing".into());
        }
        if let Some(origin) = &self.origin {
            origin.validate(pool, self.human, self.root_app()).await?;
        }
        let mut current = resolve_permissions(pool, self.human)
            .await
            .map_err(|e| format!("{e:?}"))?
            .1;
        for app in &self.agents {
            if !has_permission(&current, &format!("app:{app}:invoke")) {
                return Err(format!("permission denied: app:{app}:invoke"));
            }
            let agent = crate::extensions::agents::agent_user_id(app);
            if !principal_enabled(pool, agent).await {
                return Err("agent disabled or missing".into());
            }
            let permissions = resolve_permissions(pool, agent)
                .await
                .map_err(|e| format!("{e:?}"))?
                .1;
            current = intersect_permissions(&current, &permissions);
        }
        self.ensure_active()?;
        Ok(intersect_permissions(&self.ceiling, &current))
    }

    // Invocation IDs are visible to worker code. Different origins/lineages must
    // not share a process and borrow each other's live invocation IDs.
    pub(crate) fn partition_key(&self) -> String {
        serde_json::to_string(&(&self.agents, self.origin.as_ref().map(|o| o.key())))
            .expect("authority key")
    }
    pub(crate) fn owner(&self) -> ApprovalOwner {
        ApprovalOwner {
            user_id: self.human,
            origin: self.origin.as_ref().map(|o| o.key()),
        }
    }
    pub(crate) fn human(&self) -> Uuid {
        self.human
    }
    pub(crate) fn app(&self) -> &str {
        self.agents.last().expect("admitted agent")
    }
    pub(crate) fn root_app(&self) -> &str {
        &self.agents[0]
    }
    pub(crate) fn ceiling(&self) -> &[String] {
        &self.ceiling
    }
    pub(crate) fn task_scope(&self) -> Option<Vec<String>> {
        self.task_scope.clone()
    }
    pub(crate) fn is_child(&self) -> bool {
        self.agents.len() > 1
    }
    pub(crate) fn finish(&self) {
        self.lifetime.cancel();
    }
    pub(crate) async fn cancelled(&self) {
        self.lifetime.cancelled().await;
    }
    fn ensure_active(&self) -> Result<(), String> {
        if self.lifetime.is_cancelled() {
            Err("agent invocation ended".into())
        } else {
            Ok(())
        }
    }
}
