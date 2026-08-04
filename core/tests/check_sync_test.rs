//! Enum CHECK migration against a real PostgreSQL schema.

mod harness;

use std::collections::HashMap;

use rootcx_types::{EntityContract, FieldContract};

fn enum_entity(values: &[&str]) -> EntityContract {
    EntityContract {
        entity_name: "import_run".into(),
        fields: vec![FieldContract {
            name: "status".into(),
            field_type: "text".into(),
            required: true,
            precision: None,
            scale: None,
            default_value: None,
            enum_values: Some(values.iter().map(|value| (*value).into()).collect()),
            references: None,
            is_primary_key: None,
            on_delete: None,
            sensitive: false,
        }],
        identity_kind: None,
        identity_key: None,
        indexes: vec![],
        checks: vec![],
    }
}

async fn check_names(rt: &harness::TestRuntime) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT con.conname \
         FROM pg_constraint con \
         JOIN pg_class t ON t.oid = con.conrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         WHERE n.nspname = 'enum_sync' AND t.relname = 'import_run' \
           AND con.contype = 'c' \
         ORDER BY con.conname",
    )
    .fetch_all(rt.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn replaces_legacy_inline_enum_check_without_touching_custom_checks() {
    let rt = harness::TestRuntime::boot().await;
    sqlx::query("CREATE SCHEMA enum_sync")
        .execute(rt.pool())
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE enum_sync.import_run (\
             status TEXT NOT NULL, \
             CONSTRAINT keep_status_nonempty CHECK (status <> '')\
         )",
    )
    .execute(rt.pool())
    .await
    .unwrap();

    let entity = enum_entity(&["ready", "queued", "failed"]);
    rootcx_core::schema_sync::sync_schema(
        rt.pool(),
        "enum_sync",
        std::slice::from_ref(&entity),
        &HashMap::new(),
    )
    .await
    .unwrap();
    sqlx::query(
        "ALTER TABLE enum_sync.import_run \
         ADD CONSTRAINT import_run_status_check \
         CHECK (status IN ('ready', 'failed'))",
    )
    .execute(rt.pool())
    .await
    .unwrap();

    // Reproduce a redeploy where the managed CHECK is already current. The
    // historical inline CHECK must still be migrated instead of being skipped.
    rootcx_core::schema_sync::sync_schema(
        rt.pool(),
        "enum_sync",
        &[entity],
        &HashMap::new(),
    )
    .await
    .unwrap();

    let names = check_names(&rt).await;
    assert!(
        !names.contains(&"import_run_status_check".to_string()),
        "legacy enum CHECK remains: {names:?}"
    );
    assert!(
        names.contains(&"chk_import_run_status".to_string()),
        "managed enum CHECK missing: {names:?}"
    );
    assert!(
        names.contains(&"keep_status_nonempty".to_string()),
        "custom CHECK was removed: {names:?}"
    );

    sqlx::query("INSERT INTO enum_sync.import_run (status) VALUES ('queued')")
        .execute(rt.pool())
        .await
        .expect("new enum value must be accepted after reconciliation");
    assert!(
        sqlx::query("INSERT INTO enum_sync.import_run (status) VALUES ('unknown')")
            .execute(rt.pool())
            .await
            .is_err(),
        "managed enum CHECK must reject undeclared values"
    );

    rt.shutdown().await;
}
