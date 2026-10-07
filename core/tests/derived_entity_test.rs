//! Derived entities against a real install (ADR 0012).
//!
//! A derived entity is a table its app rebuilds from its other entities. Through
//! the real install route this asserts that it gets exactly its declared columns,
//! no audit or hooks trigger (also when an ordinary entity becomes derived), the
//! same row-level security as any entity, and that every API path refuses it with
//! the reason rather than a missing-`id` failure.

mod harness;

use serde_json::{Value, json};

fn manifest(version: &str, cache_is_derived: bool) -> Value {
    json!({
        "appId": "words", "name": "words", "version": version,
        "dataContract": [
            { "entityName": "article", "fields": [{ "name": "designation", "type": "text", "required": true }] },
            { "entityName": "article_word", "derivedFrom": "article",
              "fields": [
                  { "name": "word_no", "type": "number", "required": true },
                  { "name": "article_id", "type": "entity_link", "required": true,
                    "references": { "entity": "article", "field": "id" }, "on_delete": "cascade" },
                  { "name": "weight", "type": "number", "required": true }
              ],
              "indexes": [{ "name": "ix_article_word_word", "columns": ["word_no", "article_id"] }] },
            { "entityName": "cache", "derivedFrom": if cache_is_derived { Some("article") } else { None },
              "fields": [{ "name": "word", "type": "text" }] }
        ]
    })
}

async fn columns(rt: &harness::TestRuntime, table: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns
          WHERE table_schema = 'words' AND table_name = $1 ORDER BY column_name",
    ).bind(table).fetch_all(rt.pool()).await.unwrap()
}

async fn triggers(rt: &harness::TestRuntime, table: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT tgname::text FROM pg_trigger
          WHERE tgrelid = format('words.%I', $1::text)::regclass AND NOT tgisinternal ORDER BY tgname",
    ).bind(table).fetch_all(rt.pool()).await.unwrap()
}

#[tokio::test]
async fn a_derived_entity_is_installed_as_a_change_free_table_guarded_like_its_source() {
    let rt = harness::TestRuntime::boot().await;
    rt.install_manifest(&manifest("1.0.0", false)).await;

    // Exactly the declared columns, and its declared index.
    assert_eq!(columns(&rt, "article_word").await, ["article_id", "weight", "word_no"]);
    let index: Option<String> = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes WHERE schemaname = 'words' AND indexname = 'ix_article_word_word'",
    ).fetch_optional(rt.pool()).await.unwrap();
    assert!(index.is_some_and(|def| def.contains("(word_no, article_id)")), "declared index missing");

    // No change trigger on the derived table; the ordinary ones keep theirs.
    assert!(triggers(&rt, "article_word").await.is_empty(), "a derived table carries no audit or hooks trigger");
    assert_eq!(triggers(&rt, "article").await, ["audit_words_article", "hooks_words_article"]);

    // Governed like any entity: forced RLS and the generated select policy.
    let (rls, forced): (bool, bool) = sqlx::query_as(
        "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid = 'words.article_word'::regclass",
    ).fetch_one(rt.pool()).await.unwrap();
    assert!(rls && forced, "a derived table keeps forced row-level security");
    // Guarded by the keys of the entity it is derived from: reading the index takes
    // the right to read articles, and no key of its own is minted.
    let select: String = sqlx::query_scalar(
        "SELECT qual FROM pg_policies WHERE schemaname = 'words' AND tablename = 'article_word' AND policyname = 'rootcx_rls_select'",
    ).fetch_one(rt.pool()).await.unwrap();
    assert!(select.contains("app:words:article.read"), "{select}");
    let own_keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rootcx_system.rbac_permissions WHERE key LIKE 'app:words:article\\_word.%'",
    ).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(own_keys, 0, "a derived entity with a source mints no keys of its own");
    let source_keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rootcx_system.rbac_permissions WHERE key = 'app:words:article.read'",
    ).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(source_keys, 1);
}

#[tokio::test]
async fn every_api_path_refuses_a_derived_entity_with_its_reason() {
    let rt = harness::TestRuntime::boot().await;
    rt.install_manifest(&manifest("1.0.0", false)).await;

    // Every API path resolves entities in one place, which refuses a derived one.
    for (status, body) in [
        rt.get_json("/api/v1/apps/words/collections/article_word").await,
        rt.post_json("/api/v1/apps/words/collections/article_word", &json!({ "word_no": 1, "weight": 4 })).await,
    ] {
        assert_eq!(status, 403, "{body}");
        assert!(body.to_string().contains("is derived"), "the refusal names its reason: {body}");
    }
    let (status, _) = rt.get_json("/api/v1/apps/words/collections/article").await;
    assert_eq!(status, 200, "ordinary entities stay reachable");
}

#[tokio::test]
async fn an_ordinary_entity_that_becomes_derived_loses_its_change_triggers() {
    let rt = harness::TestRuntime::boot().await;
    rt.install_manifest(&manifest("1.0.0", false)).await;
    assert_eq!(triggers(&rt, "cache").await, ["audit_words_cache", "hooks_words_cache"]);

    // The triggers an earlier install attached go; the existing system columns
    // are left as they are.
    rt.install_manifest(&manifest("1.1.0", true)).await;
    assert!(triggers(&rt, "cache").await.is_empty(), "becoming derived drops the change triggers");
    assert_eq!(columns(&rt, "cache").await, ["created_at", "id", "updated_at", "word"]);
}

#[tokio::test]
async fn install_refuses_a_link_to_a_derived_entity() {
    let rt = harness::TestRuntime::boot().await;
    let manifest = json!({
        "appId": "links", "name": "links", "version": "1.0.0",
        "dataContract": [
            { "entityName": "word", "derivedFrom": "note", "fields": [{ "name": "word_no", "type": "number" }] },
            { "entityName": "note", "fields": [{ "name": "word_id", "type": "entity_link",
                "references": { "entity": "word", "field": "id" } }] }
        ]
    });
    let (status, body) = rt.post_json("/api/v1/apps", &manifest).await;
    assert_eq!(status, 400, "{body}");
    assert!(body.to_string().contains("derived entity 'word' cannot be linked to"), "{body}");
}
