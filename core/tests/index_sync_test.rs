//! Declarative-index reconciliation against a real schema.
//!
//! Drives `schema_sync::sync_schema` (the deploy path) with an entity carrying
//! an `indexes` block and asserts: declared indexes are created and tagged as
//! managed, re-sync is idempotent, removing an index from the manifest drops it,
//! and the table's primary-key index is never touched.

mod harness;

use std::collections::HashMap;
use rootcx_types::{EntityContract, FieldContract, IndexColumn, IndexContract};

fn field(name: &str, required: bool) -> FieldContract {
    FieldContract {
        name: name.into(), field_type: "text".into(), required,
        default_value: None, enum_values: None, references: None,
        is_primary_key: None, on_delete: None,
    }
}

// Mirror exactly the columns harness `install("crm","contacts")` creates, so the
// column diff is empty and only the index reconcile acts.
fn contacts(indexes: Vec<IndexContract>) -> Vec<EntityContract> {
    vec![EntityContract {
        entity_name: "contacts".into(),
        fields: vec![
            field("first_name", true), field("last_name", true),
            field("email", false), field("phone", false),
            field("company", false), field("notes", false),
        ],
        identity_kind: None, identity_key: None, indexes, checks: vec![],
    }]
}

async fn index_names(rt: &harness::TestRuntime) -> Vec<String> {
    sqlx::query_scalar("SELECT indexname FROM pg_indexes WHERE schemaname='crm' AND tablename='contacts' ORDER BY indexname")
        .fetch_all(rt.pool()).await.unwrap()
}

async fn sync(rt: &harness::TestRuntime, indexes: Vec<IndexContract>) {
    rootcx_core::schema_sync::sync_schema(rt.pool(), "crm", &contacts(indexes), &HashMap::new())
        .await.unwrap();
}

fn idx(name: &str, columns: &[&str], unique: bool) -> IndexContract {
    IndexContract {
        name: Some(name.into()),
        columns: columns.iter().map(|c| IndexColumn::Name((*c).into())).collect(),
        unique, using: None, where_clause: None, with: Default::default(),
    }
}

#[tokio::test]
async fn reconciles_create_idempotent_drop_without_touching_pk() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("crm", "contacts").await;

    let pk = "contacts_pkey".to_string();
    assert!(index_names(&rt).await.contains(&pk), "PK index exists after install");

    // ── Create: two declared indexes appear, PK still there. ──
    sync(&rt, vec![
        idx("ix_contacts_name", &["last_name", "first_name"], false),
        idx("ix_contacts_email_uq", &["email"], true),
    ]).await;
    let after = index_names(&rt).await;
    assert!(after.contains(&"ix_contacts_name".to_string()), "{after:?}");
    assert!(after.contains(&"ix_contacts_email_uq".to_string()), "{after:?}");
    assert!(after.contains(&pk), "PK untouched: {after:?}");

    // The unique one must actually be unique.
    let is_unique: bool = sqlx::query_scalar(
        "SELECT indisunique FROM pg_index x JOIN pg_class c ON c.oid=x.indexrelid WHERE c.relname='ix_contacts_email_uq'")
        .fetch_one(rt.pool()).await.unwrap();
    assert!(is_unique, "declared unique index must be unique");

    // ── Idempotent: same manifest, no error, same set. ──
    sync(&rt, vec![
        idx("ix_contacts_name", &["last_name", "first_name"], false),
        idx("ix_contacts_email_uq", &["email"], true),
    ]).await;
    assert_eq!(index_names(&rt).await, after, "re-sync is a no-op");

    // ── Verify (read-only drift report) mirrors the dataContract surface. ──
    async fn index_drift(rt: &harness::TestRuntime, indexes: Vec<IndexContract>) -> Vec<(String, String)> {
        let v = rootcx_core::schema_sync::verify_all(rt.pool(), "crm", &contacts(indexes), &std::collections::HashMap::new())
            .await.unwrap();
        v.changes.into_iter()
            .filter(|c| matches!(c.change_type.as_str(), "add_index" | "replace_index" | "drop_index"))
            .map(|c| (c.change_type, c.column)).collect()
    }
    // In sync → no drift.
    let synced = vec![
        idx("ix_contacts_name", &["last_name", "first_name"], false),
        idx("ix_contacts_email_uq", &["email"], true),
    ];
    assert!(index_drift(&rt, synced.clone()).await.is_empty(), "synced state reports no index drift");
    // Declare a third (not yet created) → reports add_index.
    let mut plus = synced.clone();
    plus.push(idx("ix_contacts_company", &["company"], false));
    assert_eq!(index_drift(&rt, plus).await, vec![("add_index".into(), "ix_contacts_company".into())]);
    // Drop one from the declaration → reports drop_index.
    assert_eq!(index_drift(&rt, vec![idx("ix_contacts_name", &["last_name", "first_name"], false)]).await,
        vec![("drop_index".into(), "ix_contacts_email_uq".into())]);
    // Change a declared one (same name, new cols) → reports replace_index.
    let changed = vec![idx("ix_contacts_name", &["company"], false), idx("ix_contacts_email_uq", &["email"], true)];
    assert_eq!(index_drift(&rt, changed).await, vec![("replace_index".into(), "ix_contacts_name".into())]);

    // ── Change: same name, different columns → the index is actually replaced. ──
    async fn indexed_cols(rt: &harness::TestRuntime, name: &str) -> String {
        sqlx::query_scalar("SELECT pg_get_indexdef(c.oid) FROM pg_class c WHERE c.relname = $1")
            .bind(name).fetch_one(rt.pool()).await.unwrap()
    }
    let before_def = indexed_cols(&rt, "ix_contacts_name").await;
    assert!(before_def.contains("last_name") && before_def.contains("first_name"), "{before_def}");
    sync(&rt, vec![
        idx("ix_contacts_name", &["company"], false), // same name, new columns
        idx("ix_contacts_email_uq", &["email"], true),
    ]).await;
    let after_def = indexed_cols(&rt, "ix_contacts_name").await;
    assert!(after_def.contains("company") && !after_def.contains("last_name"),
        "changed spec under same name must be replaced: {after_def}");
    // restore for the drop assertions below
    sync(&rt, vec![
        idx("ix_contacts_name", &["last_name", "first_name"], false),
        idx("ix_contacts_email_uq", &["email"], true),
    ]).await;

    // ── Drop: remove one from the manifest → it's dropped; the other + PK stay. ──
    sync(&rt, vec![idx("ix_contacts_name", &["last_name", "first_name"], false)]).await;
    let pruned = index_names(&rt).await;
    assert!(!pruned.contains(&"ix_contacts_email_uq".to_string()), "removed index dropped: {pruned:?}");
    assert!(pruned.contains(&"ix_contacts_name".to_string()), "kept index stays: {pruned:?}");
    assert!(pruned.contains(&pk), "PK never dropped: {pruned:?}");

    // ── Drop all declared → only PK (and any system indexes) remain; PK safe. ──
    sync(&rt, vec![]).await;
    let bare = index_names(&rt).await;
    assert!(!bare.iter().any(|n| n.starts_with("ix_contacts")), "all managed dropped: {bare:?}");
    assert!(bare.contains(&pk), "PK survives full prune: {bare:?}");

    rt.shutdown().await;
}
