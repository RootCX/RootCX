mod harness;

use uuid::Uuid;

async fn user_for_kind(pool: &sqlx::PgPool, kind: &str, suffix: &str) -> Uuid {
    if kind == "agent" {
        let app_id = format!("testapp-{suffix}");
        let uid = rootcx_core::extensions::agents::agent_user_id(&app_id);
        sqlx::query(
            "INSERT INTO rootcx_system.users (id, email, is_system, kind) \
             VALUES ($1, $2, true, 'agent') ON CONFLICT (id) DO NOTHING"
        ).bind(uid).bind(format!("agent+{app_id}@localhost"))
        .execute(pool).await.unwrap();
        uid
    } else {
        let uid = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO rootcx_system.users (id, email, kind) VALUES ($1, $2, $3)"
        ).bind(uid).bind(format!("{kind}-{suffix}@test.local")).bind(kind)
        .execute(pool).await.unwrap();
        uid
    }
}

#[tokio::test]
async fn delegation_matrix_kind_enforcement() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();

    struct Case { delegator_kind: &'static str, delegatee_kind: &'static str, trigger: &'static str, ok: bool, label: &'static str }

    let cases = [
        Case { delegator_kind: "human",   delegatee_kind: "agent",   trigger: "manual", ok: true,  label: "human -> agent succeeds" },
        Case { delegator_kind: "human",   delegatee_kind: "service", trigger: "manual", ok: true,  label: "human -> service succeeds" },
        Case { delegator_kind: "human",   delegatee_kind: "human",   trigger: "manual", ok: false, label: "human -> human refused" },
        Case { delegator_kind: "agent",   delegatee_kind: "service", trigger: "act_as", ok: false, label: "agent -> service act_as refused" },
        Case { delegator_kind: "service", delegatee_kind: "agent",   trigger: "manual", ok: true,  label: "service -> agent succeeds" },
    ];

    for (i, c) in cases.iter().enumerate() {
        let delegator = user_for_kind(pool, c.delegator_kind, &format!("src-{i}")).await;
        let delegatee = user_for_kind(pool, c.delegatee_kind, &format!("dst-{i}")).await;

        let result = rootcx_core::delegations::create(pool, delegator, delegatee, c.trigger, None).await;
        assert_eq!(result.is_ok(), c.ok, "case '{}': expected ok={}, got {:?}", c.label, c.ok, result);
    }
}
