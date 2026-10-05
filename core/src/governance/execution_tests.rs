use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use super::authority::has_permission;
use super::execution::{AgentAuthority, AgentSource, ExecutionOrigin};

const READ_OWN: &str = "app:business:records.read.own";
const UPDATE: &str = "app:business:records.update";
const DELETE: &str = "app:business:records.delete";

struct Fixture {
    pool: PgPool,
    principals: [Uuid; 4],
    apps: [String; 3],
    roles: [String; 4],
    permissions: Vec<String>,
}

impl Fixture {
    async fn new() -> Self {
        let pool = crate::extensions::test_db::pool().await;
        let prefix = format!("execution_{}", Uuid::new_v4().simple());
        let apps = [
            format!("{prefix}_root"),
            format!("{prefix}_middle"),
            format!("{prefix}_leaf"),
        ];
        let principals = [
            Uuid::new_v4(),
            crate::extensions::agents::agent_user_id(&apps[0]),
            crate::extensions::agents::agent_user_id(&apps[1]),
            crate::extensions::agents::agent_user_id(&apps[2]),
        ];
        let roles = std::array::from_fn(|i| format!("{prefix}_role_{i}"));
        let mut permissions = vec![
            "tool:call_action".into(),
            "tool:invoke_agent".into(),
            READ_OWN.into(),
            UPDATE.into(),
        ];
        permissions.extend(apps.iter().map(|app| format!("app:{app}:invoke")));
        for (index, user) in principals.iter().enumerate() {
            sqlx::query("INSERT INTO rootcx_system.users (id, email, kind, owner_of_record) VALUES ($1, $2, $3, $4)")
                .bind(user).bind(format!("{user}@execution.test"))
                .bind(if index == 0 { "human" } else { "agent" })
                .bind((index != 0).then_some(principals[0])).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ($1, $2)")
                .bind(&roles[index])
                .bind(&permissions)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2)",
            )
            .bind(user)
            .bind(&roles[index])
            .execute(&pool)
            .await
            .unwrap();
        }
        Self {
            pool,
            principals,
            apps,
            roles,
            permissions,
        }
    }

    async fn permissions(&self, principal: usize, permissions: &[String]) {
        sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions = $2 WHERE name = $1")
            .bind(&self.roles[principal])
            .bind(permissions)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn chain(&self, scope: Option<Vec<String>>, source: AgentSource) -> [AgentAuthority; 3] {
        let root = AgentAuthority::admit(
            &self.pool,
            &self.apps[0],
            Some(self.principals[0]),
            scope,
            source,
        )
        .await
        .unwrap();
        let middle = AgentAuthority::admit(
            &self.pool,
            &self.apps[1],
            Some(self.principals[0]),
            None,
            AgentSource::Child(root.clone()),
        )
        .await
        .unwrap();
        let leaf = AgentAuthority::admit(
            &self.pool,
            &self.apps[2],
            Some(self.principals[0]),
            None,
            AgentSource::Child(middle.clone()),
        )
        .await
        .unwrap();
        [root, middle, leaf]
    }

    async fn cleanup(self) {
        sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = ANY($1)")
            .bind(self.principals.as_slice())
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM rootcx_system.rbac_roles WHERE name = ANY($1)")
            .bind(self.roles.as_slice())
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM rootcx_system.users WHERE id = ANY($1)")
            .bind(self.principals.as_slice())
            .execute(&self.pool)
            .await
            .unwrap();
        self.pool.close().await;
    }
}

#[tokio::test]
async fn live_authority_tracks_every_principal_in_the_delegation_chain() {
    let fixture = Fixture::new().await;
    let chain = fixture.chain(None, AgentSource::Direct).await;
    let leaf = &chain[2];
    assert!(has_permission(
        &leaf.revalidate(&fixture.pool).await.unwrap(),
        UPDATE
    ));
    let narrowed: Vec<_> = fixture
        .permissions
        .iter()
        .filter(|p| p.as_str() != UPDATE)
        .cloned()
        .collect();
    for (principal, name) in ["human", "root", "intermediate", "leaf"].iter().enumerate() {
        fixture.permissions(principal, &narrowed).await;
        let current = leaf.revalidate(&fixture.pool).await.unwrap();
        assert!(!has_permission(&current, UPDATE), "revoked {name}");
        assert!(
            has_permission(&current, READ_OWN),
            "unrelated right of {name}"
        );
        assert!(
            !has_permission(&current, "app:business:records.read"),
            "own scope of {name}"
        );
        fixture.permissions(principal, &fixture.permissions).await;

        sqlx::query("UPDATE rootcx_system.users SET disabled_at = now() WHERE id = $1")
            .bind(fixture.principals[principal])
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert!(
            leaf.revalidate(&fixture.pool).await.is_err(),
            "disabled {name}"
        );
        sqlx::query("UPDATE rootcx_system.users SET disabled_at = NULL WHERE id = $1")
            .bind(fixture.principals[principal])
            .execute(&fixture.pool)
            .await
            .unwrap();
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn new_role_grants_cannot_widen_admitted_authority() {
    let fixture = Fixture::new().await;
    let chain = fixture.chain(None, AgentSource::Direct).await;
    let mut expanded = fixture.permissions.clone();
    expanded.push(DELETE.into());
    for principal in 0..4 {
        fixture.permissions(principal, &expanded).await;
    }
    for authority in &chain {
        let current = authority.revalidate(&fixture.pool).await.unwrap();
        assert!(
            has_permission(&current, UPDATE),
            "existing right, {}",
            authority.app()
        );
        assert!(
            !has_permission(&current, DELETE),
            "new right, {}",
            authority.app()
        );
    }
    let late_child = AgentAuthority::admit(
        &fixture.pool,
        &fixture.apps[1],
        Some(fixture.principals[0]),
        None,
        AgentSource::Child(chain[0].clone()),
    )
    .await
    .unwrap();
    assert!(
        !has_permission(&late_child.revalidate(&fixture.pool).await.unwrap(), DELETE),
        "a new child inherits its parent's frozen ceiling"
    );
    let fresh = fixture.chain(None, AgentSource::Direct).await;
    assert!(
        has_permission(&fresh[2].revalidate(&fixture.pool).await.unwrap(), DELETE),
        "a fresh invocation can use newly authorized rights"
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn child_scope_cannot_widen_the_parent_scope() {
    let fixture = Fixture::new().await;
    let scope: Vec<_> = fixture
        .permissions
        .iter()
        .filter(|p| p.as_str() != UPDATE)
        .cloned()
        .collect();
    let root = AgentAuthority::admit(
        &fixture.pool,
        &fixture.apps[0],
        Some(fixture.principals[0]),
        Some(scope),
        AgentSource::Direct,
    )
    .await
    .unwrap();
    for child_scope in [None, Some(vec!["*".into()])] {
        let child = AgentAuthority::admit(
            &fixture.pool,
            &fixture.apps[1],
            Some(fixture.principals[0]),
            child_scope.clone(),
            AgentSource::Child(root.clone()),
        )
        .await
        .unwrap();
        let current = child.revalidate(&fixture.pool).await.unwrap();
        assert!(has_permission(&current, READ_OWN), "{child_scope:?}");
        assert!(!has_permission(&current, UPDATE), "{child_scope:?}");
    }
    fixture.cleanup().await;
}

#[derive(Debug)]
struct Origin(AtomicBool);

#[async_trait]
impl ExecutionOrigin for Origin {
    fn key(&self) -> String {
        "test-channel:conversation".into()
    }
    async fn validate(&self, _: &PgPool, _: Uuid, _: &str) -> Result<(), String> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("origin revoked".into())
        }
    }
}

#[tokio::test]
async fn origin_revocation_reaches_existing_and_new_descendants() {
    let fixture = Fixture::new().await;
    let origin = Arc::new(Origin(AtomicBool::new(true)));
    let chain = fixture
        .chain(None, AgentSource::Origin(origin.clone()))
        .await;
    assert!(has_permission(
        &chain[2].revalidate(&fixture.pool).await.unwrap(),
        UPDATE
    ));
    origin.0.store(false, Ordering::SeqCst);
    for authority in &chain {
        assert!(
            authority.revalidate(&fixture.pool).await.is_err(),
            "{}",
            authority.app()
        );
    }
    assert!(
        AgentAuthority::admit(
            &fixture.pool,
            &fixture.apps[1],
            Some(fixture.principals[0]),
            None,
            AgentSource::Child(chain[0].clone())
        )
        .await
        .is_err()
    );
    assert!(
        AgentAuthority::admit(
            &fixture.pool,
            &fixture.apps[0],
            Some(fixture.principals[0]),
            None,
            AgentSource::Origin(origin)
        )
        .await
        .is_err()
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn ending_an_invocation_cancels_descendants_without_cancelling_ancestors() {
    let fixture = Fixture::new().await;
    for ended in 0..3 {
        let chain = fixture.chain(None, AgentSource::Direct).await;
        chain[ended].finish();
        for (index, authority) in chain.iter().enumerate() {
            assert_eq!(
                authority.revalidate(&fixture.pool).await.is_err(),
                index >= ended,
                "ended={ended}, checked={index}"
            );
        }
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn admission_rejects_missing_identity_cycles_and_lost_child_invoke_rights() {
    let fixture = Fixture::new().await;
    assert!(
        AgentAuthority::admit(
            &fixture.pool,
            &fixture.apps[0],
            None,
            None,
            AgentSource::Direct
        )
        .await
        .is_err()
    );
    let chain = fixture.chain(None, AgentSource::Direct).await;
    assert!(
        AgentAuthority::admit(
            &fixture.pool,
            &fixture.apps[1],
            Some(Uuid::new_v4()),
            None,
            AgentSource::Child(chain[0].clone())
        )
        .await
        .is_err(),
        "child cannot replace responsible human"
    );
    assert!(
        AgentAuthority::admit(
            &fixture.pool,
            &fixture.apps[0],
            Some(fixture.principals[0]),
            None,
            AgentSource::Child(chain[1].clone())
        )
        .await
        .is_err(),
        "child cannot cycle back to its root"
    );
    let child_invoke = format!("app:{}:invoke", fixture.apps[1]);
    let narrowed: Vec<_> = fixture
        .permissions
        .iter()
        .filter(|p| *p != &child_invoke)
        .cloned()
        .collect();
    for principal in [0, 1] {
        fixture.permissions(principal, &narrowed).await;
        assert!(
            chain[2].revalidate(&fixture.pool).await.is_err(),
            "existing child, principal={principal}"
        );
        assert!(
            AgentAuthority::admit(
                &fixture.pool,
                &fixture.apps[1],
                Some(fixture.principals[0]),
                None,
                AgentSource::Child(chain[0].clone())
            )
            .await
            .is_err(),
            "new child, principal={principal}"
        );
        fixture.permissions(principal, &fixture.permissions).await;
    }
    fixture.cleanup().await;
}
