use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

#[test]
fn sources_cannot_escape_or_import_credentials_and_dependencies() {
    for p in [
        "../x",
        "/etc/passwd",
        "a/../../x",
        "a//x",
        "a/./x",
        "a\\x",
        ".git/config",
        "x/.git/hooks/pre-commit",
        ".env",
        "backend/.env.production",
        "node_modules/x",
        "dist/index.html",
        ".npmrc",
    ] {
        assert!(files::path(p).is_err(), "accepted {p}");
    }
    for p in [
        "manifest.json",
        "src/App.tsx",
        "backend/index.ts",
        ".env.example",
        ".gitignore",
    ] {
        assert!(files::path(p).is_ok(), "rejected {p}");
    }
}

#[tokio::test]
async fn git_revision_retains_previous_sources_and_binary_assets() {
    let tmp = tempfile::tempdir().unwrap();
    let initial = Files::from([
        ("manifest.json".into(), STANDARD.encode(b"{}")),
        ("public/logo.png".into(), STANDARD.encode([0, 255, 1, 2])),
    ]);
    files::write(tmp.path(), &initial).await.unwrap();
    files::git(tmp.path(), &["init", "--initial-branch=main"])
        .await
        .unwrap();
    let first = files::commit(tmp.path(), "initial").await.unwrap();
    let mut changed = initial.clone();
    changed.insert("manifest.json".into(), STANDARD.encode(b"{\"version\":2}"));
    files::write(tmp.path(), &changed).await.unwrap();
    let second = files::commit(tmp.path(), "change").await.unwrap();
    assert_eq!(files::snapshot(tmp.path(), &first).await.unwrap(), initial);
    assert_eq!(files::snapshot(tmp.path(), &second).await.unwrap(), changed);
}

#[test]
fn automatic_publication_rejects_data_loss_before_schema_changes() {
    let parse = |fields: Value| {
        serde_json::from_value::<rootcx_types::AppManifest>(json!({"appId":"sample","name":"Sample","version":"1.0.0","dataContract":[{"entityName":"quotes","fields":fields}]})).unwrap()
    };
    let before = parse(json!([{"name":"label","type":"text"}]));
    for fields in [
        json!([]),
        json!([{"name":"label","type":"integer"}]),
        json!([{"name":"label","type":"text"},{"name":"visit_date","type":"date","required":true}]),
    ] {
        assert!(
            release::compatible(&before, &parse(fields.clone())).is_err(),
            "accepted destructive change {fields}"
        );
    }
    assert!(
        release::compatible(
            &before,
            &parse(json!([{"name":"label","type":"text"},{"name":"visit_date","type":"date"}]))
        )
        .is_ok()
    );
}
