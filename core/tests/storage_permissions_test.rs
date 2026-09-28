mod harness;

use harness::TestRuntime;
use reqwest::{Method, multipart};
use serde_json::Value;
use uuid::Uuid;

const APP: &str = "private_files";
const CONTENT: &[u8] = b"private document";

async fn storage_user(rt: &TestRuntime) -> String {
    let token = rt.create_user("storage-user@test.local").await;
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name) VALUES ('storage-test')")
        .execute(rt.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
         SELECT id, 'storage-test' FROM rootcx_system.users WHERE email = 'storage-user@test.local'",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    token
}

async fn set_permissions(rt: &TestRuntime, permissions: &[&str]) {
    sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions = $1 WHERE name = 'storage-test'")
        .bind(permissions)
        .execute(rt.pool())
        .await
        .unwrap();
}

fn file_form() -> multipart::Form {
    multipart::Form::new().part(
        "file",
        multipart::Part::bytes(CONTENT)
            .file_name("private.txt")
            .mime_str("text/plain")
            .unwrap(),
    )
}

#[tokio::test]
async fn app_storage_download_requires_read_for_the_requested_app() {
    let rt = TestRuntime::boot().await;
    rt.install(APP, "documents").await;
    rt.install("other_files", "documents").await;
    let token = storage_user(&rt).await;
    let (status, file) = rt
        .upload(
            &format!("/api/v1/apps/{APP}/storage/upload"),
            "private.txt",
            "text/plain",
            CONTENT,
        )
        .await;
    assert_eq!(status, 201, "{file}");
    let file_id = file["file_id"].as_str().unwrap();
    let url = rt.url(&format!("/api/v1/apps/{APP}/storage/{file_id}"));

    for (permissions, expected) in [
        (vec![], 403),
        (vec!["app:private_files:documents.read"], 403),
        (vec!["storage:read"], 403),
        (vec!["app:other_files:storage.read"], 403),
        (vec!["app:private_files:Storage.read"], 403),
        (vec!["app:private_files:storage.write"], 403),
        (vec!["app:private_files:storage.read"], 200),
        (vec![], 403), // Revocation must apply to the same still-valid token.
        (vec!["app:private_files:*"], 200),
        (vec!["*"], 200),
    ] {
        set_permissions(&rt, &permissions).await;
        for method in [Method::GET, Method::HEAD] {
            let response = rt
                .client
                .request(method.clone(), &url)
                .bearer_auth(&token)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{method} {permissions:?}");
            if method == Method::GET {
                if expected == 200 {
                    assert_eq!(
                        response.bytes().await.unwrap().as_ref(),
                        CONTENT,
                        "{permissions:?}"
                    );
                } else {
                    let body: Value = response.json().await.unwrap();
                    assert_eq!(
                        body["error"], "permission denied: app:private_files:storage.read",
                        "{permissions:?}"
                    );
                }
            }
        }
    }

    set_permissions(&rt, &["app:other_files:storage.read"]).await;
    let response = rt
        .client
        .get(rt.url(&format!("/api/v1/apps/other_files/storage/{file_id}")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        404,
        "an authorized app path must not expose another app's file"
    );

    set_permissions(&rt, &[]).await;
    let response = rt
        .client
        .get(rt.url(&format!("/api/v1/apps/{APP}/storage/{}", Uuid::new_v4())))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        403,
        "authorization must precede file lookup"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn app_storage_upload_requires_write_without_persisting_denied_files() {
    let rt = TestRuntime::boot().await;
    rt.install(APP, "documents").await;
    let token = storage_user(&rt).await;
    let url = rt.url(&format!("/api/v1/apps/{APP}/storage/upload"));

    for (permissions, expected) in [
        (vec![], 403),
        (vec!["app:private_files:storage.read"], 403),
        (vec!["app:other_files:storage.write"], 403),
        (vec!["storage:write"], 403),
        (vec!["app:private_files:Storage.write"], 403),
        (vec!["app:private_files:storage.write"], 201),
        (vec![], 403),
        (vec!["app:private_files:*"], 201),
        (vec!["*"], 201),
    ] {
        set_permissions(&rt, &permissions).await;
        let rows_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rootcx_system.files")
            .fetch_one(rt.pool())
            .await
            .unwrap();
        let objects_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pg_largeobject_metadata")
                .fetch_one(rt.pool())
                .await
                .unwrap();
        let response = rt
            .client
            .post(&url)
            .bearer_auth(&token)
            .multipart(file_form())
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert_eq!(status, expected, "{permissions:?}: {body}");
        if expected == 403 {
            assert_eq!(
                body["error"], "permission denied: app:private_files:storage.write",
                "{permissions:?}"
            );
            let rows_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rootcx_system.files")
                .fetch_one(rt.pool())
                .await
                .unwrap();
            let objects_after: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM pg_largeobject_metadata")
                    .fetch_one(rt.pool())
                    .await
                    .unwrap();
            assert_eq!(
                rows_after, rows_before,
                "{permissions:?}: denied upload persisted metadata"
            );
            assert_eq!(
                objects_after, objects_before,
                "{permissions:?}: denied upload leaked a Large Object"
            );
        } else {
            let file_id = body["file_id"].as_str().unwrap();
            let (status, bytes, _) = rt
                .get_raw(&format!("/api/v1/apps/{APP}/storage/{file_id}"))
                .await;
            assert_eq!(status, 200, "{permissions:?}: uploaded file missing");
            assert_eq!(bytes, CONTENT, "{permissions:?}: uploaded content changed");
        }
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn app_storage_delete_requires_write_and_preserves_denied_files() {
    let rt = TestRuntime::boot().await;
    rt.install(APP, "documents").await;
    rt.install("other_files", "documents").await;
    let token = storage_user(&rt).await;

    for (permissions, expected) in [
        (vec![], 403),
        (vec!["app:private_files:storage.read"], 403),
        (vec!["app:other_files:storage.write"], 403),
        (vec!["storage:delete"], 403),
        (vec!["app:private_files:storage.delete"], 403),
        (vec!["app:private_files:Storage.write"], 403),
        (vec!["app:private_files:storage.write"], 200),
        (vec![], 403),
        (vec!["app:private_files:*"], 200),
        (vec!["*"], 200),
    ] {
        let (status, file) = rt
            .upload(
                &format!("/api/v1/apps/{APP}/storage/upload"),
                "private.txt",
                "text/plain",
                CONTENT,
            )
            .await;
        assert_eq!(status, 201, "{permissions:?}: {file}");
        let file_id: Uuid = file["file_id"].as_str().unwrap().parse().unwrap();
        let path = format!("/api/v1/apps/{APP}/storage/{file_id}");
        let oid: sqlx::postgres::types::Oid =
            sqlx::query_scalar("SELECT content_oid FROM rootcx_system.files WHERE id = $1")
                .bind(file_id)
                .fetch_one(rt.pool())
                .await
                .unwrap();
        set_permissions(&rt, &permissions).await;
        let (status, body) = rt.request_as(Method::DELETE, &path, &token, None).await;
        assert_eq!(status, expected, "{permissions:?}: {body}");
        let (read_status, bytes, _) = rt.get_raw(&path).await;
        let object_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_largeobject_metadata WHERE oid = $1)",
        )
        .bind(oid)
        .fetch_one(rt.pool())
        .await
        .unwrap();
        if expected == 403 {
            assert_eq!(
                body["error"], "permission denied: app:private_files:storage.write",
                "{permissions:?}"
            );
            assert_eq!(
                read_status, 200,
                "{permissions:?}: denied delete removed the file"
            );
            assert_eq!(
                bytes, CONTENT,
                "{permissions:?}: denied delete changed content"
            );
            assert!(
                object_exists,
                "{permissions:?}: denied delete unlinked the Large Object"
            );
        } else {
            assert_eq!(body["deleted"], file_id.to_string(), "{permissions:?}");
            assert_eq!(
                read_status, 404,
                "{permissions:?}: allowed delete retained the file"
            );
            assert!(
                !object_exists,
                "{permissions:?}: allowed delete orphaned the Large Object"
            );
        }
    }

    let (status, file) = rt
        .upload(
            &format!("/api/v1/apps/{APP}/storage/upload"),
            "private.txt",
            "text/plain",
            CONTENT,
        )
        .await;
    assert_eq!(status, 201, "{file}");
    let file_id = file["file_id"].as_str().unwrap();
    set_permissions(&rt, &["app:other_files:storage.write"]).await;
    let (status, _) = rt
        .request_as(
            Method::DELETE,
            &format!("/api/v1/apps/other_files/storage/{file_id}"),
            &token,
            None,
        )
        .await;
    assert_eq!(
        status, 404,
        "an authorized app path must not delete another app's file"
    );
    let (status, bytes, _) = rt
        .get_raw(&format!("/api/v1/apps/{APP}/storage/{file_id}"))
        .await;
    assert_eq!(status, 200);
    assert_eq!(bytes, CONTENT);
    rt.shutdown().await;
}
