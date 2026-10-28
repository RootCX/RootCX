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

#[tokio::test]
async fn delegated_collection_mutation_retains_parent_owned_scope_without_read_permission() {
    use crate::tools::{mutate_data::MutateDataTool, query_data::QueryDataTool, Tool};
    use serde_json::json;

    let fixture = Fixture::new().await;
    let provider = fixture.install_collection_app().await;
    for principal in 0..4 {
        fixture.permissions(principal, &["*".into()]).await;
    }
    let mut scope = vec!["tool:*".into(), format!("app:{provider}:records.update.own")];
    scope.extend(fixture.apps.iter().map(|app| format!("app:{app}:invoke")));
    let chain = fixture.chain(Some(scope), AgentSource::Direct).await;
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    let another_owner = Uuid::new_v4();
    sqlx::query(&format!("INSERT INTO \"{provider}\".records (id, owner_id, name) VALUES ($1,$2,'mine'),($3,$4,'theirs')"))
        .bind(mine).bind(fixture.principals[0]).bind(theirs).bind(another_owner)
        .execute(&fixture.pool).await.unwrap();
    let single_connection = sqlx::postgres::PgPoolOptions::new().max_connections(1)
        .connect(&std::env::var("TEST_DATABASE_URL").unwrap()).await.unwrap();
    for (label, id, patch, allowed) in [
        ("update own row without read", mine, json!({"name": "changed"}), true),
        ("update someone else's row", theirs, json!({"name": "stolen"}), false),
        ("transfer own row", mine, json!({"owner_id": another_owner}), false),
    ] {
        let mut ctx = fixture.collection_context(&chain[2], json!({
            "app": provider, "entity": "records", "action": "update", "id": id, "data": patch,
        })).await;
        ctx.pool = single_connection.clone();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), MutateDataTool.execute(&ctx))
            .await.expect("collection execution must not reacquire its transaction's sole pool connection");
        assert_eq!(result.is_ok(), allowed, "{label}: {result:?}");
    }
    single_connection.close().await;
    let read = fixture.collection_context(&chain[2], json!({"app": provider, "entity": "records"})).await;
    assert!(QueryDataTool.execute(&read).await.is_err(), "a mutation's internal SELECT cannot authorize a separate read");
    let delete = fixture.collection_context(&chain[2], json!({
        "app": provider, "entity": "records", "action": "delete", "id": mine,
    })).await;
    assert!(MutateDataTool.execute(&delete).await.is_err(), "child admin cannot exceed parent task scope");
    let rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(&format!("SELECT id, owner_id, name FROM \"{provider}\".records ORDER BY name"))
        .fetch_all(&fixture.pool).await.unwrap();
    assert_eq!(rows, vec![(mine, fixture.principals[0], "changed".into()), (theirs, another_owner, "theirs".into())]);
    crate::manifest::uninstall_app(&fixture.pool, &provider, Some(fixture.principals[0])).await.unwrap();
    fixture.cleanup().await;
}

#[tokio::test]
async fn delegated_mutation_sql_marker_cannot_expand_its_collection_or_operation() {
    use super::enforcement::{ContextState, DataAccess, TIMEOUT_AGENT_TOOL_MS};

    let fixture = Fixture::new().await;
    let provider = fixture.install_collection_app().await;
    let state = ContextState {
        user_id: Some(fixture.principals[0]), is_delegated: true,
        effective_perms: vec![format!("app:{provider}:records.update.own"), format!("app:{provider}:other.update.own")],
        ..ContextState::default()
    };
    let types = crate::manifest::field_type_map(&fixture.pool, &provider, "records").await.unwrap();
    for marked in [false, true] {
        let access = DataAccess::app(&provider, &state, "agent_tool", TIMEOUT_AGENT_TOOL_MS);
        let access = if marked { access.for_delegated_collection("records", "update", &types) } else { access };
        let mut tx = access.begin(&fixture.pool).await.unwrap();
        for (key, expected) in [
            (format!("app:{provider}:records.read.own"), marked),
            (format!("app:{provider}:records.read"), false),
            (format!("app:{provider}:records.read.shared"), false),
            (format!("app:{provider}:other.read.own"), false),
            ("app:another:records.read.own".into(), false),
            (format!("app:{provider}:records.delete.own"), false),
        ] {
            let allowed: bool = sqlx::query_scalar("SELECT rootcx_system.check_access($1)")
                .bind(&key).fetch_one(&mut *tx).await.unwrap();
            assert_eq!(allowed, expected, "marked={marked}, key={key}");
        }
        tx.rollback().await.unwrap();
    }
    let mut tx = DataAccess::app(&provider, &state, "worker", TIMEOUT_AGENT_TOOL_MS)
        .begin(&fixture.pool).await.unwrap();
    let forged = sqlx::query("SELECT set_config('rootcx.agent_collection_mutation', 'update', true)")
        .execute(&mut *tx).await;
    assert!(forged.is_err(), "untrusted worker SQL must not pose any Core authority marker");
    tx.rollback().await.unwrap();
    crate::manifest::uninstall_app(&fixture.pool, &provider, Some(fixture.principals[0])).await.unwrap();
    fixture.cleanup().await;
}

struct Fixture {
    pool: PgPool,
    principals: [Uuid; 4],
    apps: [String; 3],
    roles: [String; 4],
    permissions: Vec<String>,
}

impl Fixture {
    async fn install_collection_app(&self) -> String {
        use crate::extensions::RuntimeExtension;
        static HOOKS: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
        HOOKS.get_or_init(|| async {
            crate::extensions::hooks::HooksExtension.bootstrap(&self.pool).await.unwrap();
        }).await;
        let app = format!("data_{}", Uuid::new_v4().simple());
        let manifest = serde_json::from_value(serde_json::json!({
            "appId": app, "name": app, "version": "1.0.0",
            "dataContract": [{"entityName": "records", "fields": [
                {"name": "owner_id", "type": "uuid", "owner": true},
                {"name": "name", "type": "text"}
            ]}]
        })).unwrap();
        crate::manifest::install_app(
            &self.pool, &manifest, &[], self.principals[0],
            &crate::secrets::SecretManager::with_key(&[7; 32]),
        ).await.unwrap();
        super::row_access::install(&self.pool, &manifest).await.unwrap();
        app
    }

    async fn collection_context(&self, authority: &AgentAuthority, args: serde_json::Value) -> crate::tools::ToolContext {
        crate::tools::ToolContext {
            authority: Some(authority.clone()),
            pool: self.pool.clone(),
            core_bound_app_id: Some(authority.app().into()),
            app_id: authority.app().into(),
            user_id: crate::extensions::agents::agent_user_id(authority.app()),
            invoker_user_id: Some(authority.human()),
            permissions: authority.revalidate(&self.pool).await.unwrap(),
            task_scope: authority.task_scope(), args,
            agent_dispatch: None, integration_caller: None, action_caller: None,
            stream_tx: None, idempotency_key: None,
        }
    }

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

#[tokio::test]
async fn app_deletion_requires_bound_confirmation_and_live_delegated_rights() {
    use crate::extensions::RuntimeExtension;
    use crate::tools::{delete_app::DeleteAppTool, Tool};
    use serde_json::json;
    let fixture = Fixture::new().await;
    crate::extensions::agents::AgentExtension.bootstrap(&fixture.pool).await.unwrap();
    crate::builder::bootstrap(&fixture.pool).await.unwrap();
    for principal in 0..4 { fixture.permissions(principal, &["*".into()]).await; }
    let provider = fixture.install_collection_app().await;
    let chain = fixture.chain(None, AgentSource::Direct).await;
    let mut authority = chain[0].clone();
    let prepared = DeleteAppTool.prepare_confirmation(&fixture.pool, &authority, &json!({"appId":provider})).await.unwrap().unwrap();
    let mut ctx = fixture.collection_context(&authority, prepared.args.clone()).await;
    assert!(DeleteAppTool.execute(&ctx).await.is_err(), "model arguments alone are not a confirmation");
    authority.approve_call("delete_app", &prepared.args);
    let child = AgentAuthority::admit(&fixture.pool, &fixture.apps[1], Some(fixture.principals[0]), None, AgentSource::Child(authority.clone())).await.unwrap();
    assert!(!child.confirmed_call("delete_app", &prepared.args), "a child cannot inherit a parent's receipt");
    ctx.authority = Some(authority.clone());
    ctx.args["appId"] = json!("another_app");
    assert!(DeleteAppTool.execute(&ctx).await.is_err(), "approval cannot be retargeted");
    ctx.args = prepared.args.clone();
    for (principal, label) in [(0, "human"), (1, "agent")] {
        fixture.permissions(principal, &["tool:*".into(), format!("app:{}:invoke", fixture.apps[0])]).await;
        assert!(DeleteAppTool.execute(&ctx).await.is_err(), "revoked {label} after approval");
        fixture.permissions(principal, &["*".into()]).await;
    }
    let mut scope = vec!["tool:*".into()];
    scope.extend(fixture.apps.iter().map(|app| format!("app:{app}:invoke")));
    let scoped = fixture.chain(Some(scope), AgentSource::Direct).await;
    assert!(DeleteAppTool.prepare_confirmation(&fixture.pool, &scoped[0], &json!({"appId":provider})).await.is_err(), "task scope cannot acquire delete from admin");
    let admitted = DeleteAppTool.execute(&ctx).await.unwrap();
    assert_eq!(admitted["state"], "deleting", "default admin permissions require no additional grant");
    let audit: (Uuid, Option<Uuid>) = sqlx::query_as("SELECT actor_uid,delegator_uid FROM rootcx_system.app_deletion_events WHERE operation_id=$1::uuid AND event='confirmed'")
        .bind(admitted["id"].as_str().unwrap()).fetch_one(&fixture.pool).await.unwrap();
    assert_eq!(audit, (fixture.principals[1], Some(fixture.principals[0])));
    // This fixture has no background runtime: release its admitted operation for cleanup.
    sqlx::query("UPDATE rootcx_system.app_deletions SET state='succeeded' WHERE id=$1::uuid").bind(admitted["id"].as_str().unwrap()).execute(&fixture.pool).await.unwrap();
    crate::manifest::uninstall_app(&fixture.pool, &provider, Some(fixture.principals[0])).await.unwrap();
    fixture.cleanup().await;
}
