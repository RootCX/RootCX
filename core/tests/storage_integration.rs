mod harness;
use axum::body::Bytes;
use futures_util::{StreamExt, stream};
use harness::TestRuntime;
use reqwest::multipart;
use serde_json::{Value, json};
use sqlx::postgres::types::Oid;

#[tokio::test]
async fn storage_full_lifecycle() {
    let rt = TestRuntime::boot().await;

    // ─── Buckets ───

    // Create bucket
    let (s, body) = rt.post_json("/api/v1/storage/buckets", &json!({"name": "docs"})).await;
    assert_eq!(s, 201, "create bucket: {body}");
    assert_eq!(body["name"], "docs");

    // List buckets (should include default + docs)
    let (s, body) = rt.get_json("/api/v1/storage/buckets").await;
    assert_eq!(s, 200);
    let names: Vec<&str> = body.as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"default"));
    assert!(names.contains(&"docs"));

    // Duplicate bucket rejected
    let (s, _) = rt.post_json("/api/v1/storage/buckets", &json!({"name": "docs"})).await;
    assert_eq!(s, 400);

    // Invalid bucket name rejected
    let (s, _) = rt.post_json("/api/v1/storage/buckets", &json!({"name": "has space"})).await;
    assert_eq!(s, 400);

    // ─── Folders ───

    // Create folder at root
    let (s, body) = rt.post_json("/api/v1/storage/objects/docs", &json!({"name": "invoices"})).await;
    assert_eq!(s, 201, "create folder: {body}");
    let folder_id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["is_folder"], true);

    // Create subfolder
    let (s, body) = rt.post_json("/api/v1/storage/objects/docs", &json!({"name": "2026", "parent_id": folder_id})).await;
    assert_eq!(s, 201, "create subfolder: {body}");
    let subfolder_id = body["id"].as_str().unwrap().to_string();

    // Duplicate folder name at same level rejected
    let (s, _) = rt.post_json("/api/v1/storage/objects/docs", &json!({"name": "invoices"})).await;
    assert_eq!(s, 400);

    // Same name in different parent allowed
    let (s, _) = rt.post_json("/api/v1/storage/objects/docs", &json!({"name": "invoices", "parent_id": folder_id})).await;
    assert_eq!(s, 201);

    // ─── Upload ───

    // Upload file at root
    let (s, body) = rt.upload("/api/v1/storage/objects/docs/upload", "readme.md", "text/plain", b"# Hello").await;
    assert_eq!(s, 201, "upload root file: {body}");
    let root_file_id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["name"], "readme.md");
    assert_eq!(body["size"], 7);

    // Upload file into subfolder (with parent_id in multipart)
    let part = multipart::Part::bytes(b"invoice data".to_vec()).file_name("inv-001.pdf").mime_str("application/pdf").unwrap();
    let form = multipart::Form::new()
        .text("parent_id", subfolder_id.clone())
        .part("file", part);
    let r = rt.client.post(rt.url("/api/v1/storage/objects/docs/upload"))
        .bearer_auth(&rt.token).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 201);
    let body: Value = r.json().await.unwrap();
    let nested_file_id = body["id"].as_str().unwrap().to_string();

    // ─── List ───

    // List root of bucket (should see folder + root file, not nested file)
    let (s, body) = rt.get_json("/api/v1/storage/objects/docs").await;
    assert_eq!(s, 200);
    let items = body.as_array().unwrap();
    let ids: Vec<&str> = items.iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&folder_id.as_str()), "folder should be at root");
    assert!(ids.contains(&root_file_id.as_str()), "root file should be at root");
    assert!(!ids.contains(&nested_file_id.as_str()), "nested file should NOT be at root");

    // List subfolder
    let (s, body) = rt.get_json(&format!("/api/v1/storage/objects/docs?parent_id={subfolder_id}")).await;
    assert_eq!(s, 200);
    let items = body.as_array().unwrap();
    assert!(items.iter().any(|o| o["id"].as_str() == Some(nested_file_id.as_str())));

    // ─── Download ───

    let (s, bytes, ct) = rt.get_raw(&format!("/api/v1/storage/objects/docs/{root_file_id}")).await;
    assert_eq!(s, 200);
    assert_eq!(bytes, b"# Hello");
    assert_eq!(ct, "text/plain");

    // Download folder returns error
    let (s, _) = rt.get_json(&format!("/api/v1/storage/objects/docs/{folder_id}")).await;
    assert_eq!(s, 400);

    // ─── Rename ───

    let (s, _) = rt.patch_json(&format!("/api/v1/storage/objects/docs/{root_file_id}"), &json!({"name": "README.md"})).await;
    assert_eq!(s, 200);

    // Verify rename took effect
    let (s, body) = rt.get_json("/api/v1/storage/objects/docs").await;
    assert_eq!(s, 200);
    let renamed = body.as_array().unwrap().iter().find(|o| o["id"].as_str() == Some(root_file_id.as_str())).unwrap();
    assert_eq!(renamed["name"], "README.md");

    // Rename to existing name rejected
    let (s, _) = rt.post_json("/api/v1/storage/objects/docs", &json!({"name": "conflict.txt"})).await;
    let (s2, _) = rt.patch_json(&format!("/api/v1/storage/objects/docs/{root_file_id}"), &json!({"name": "conflict.txt"})).await;
    assert_eq!(s, 201);
    assert_eq!(s2, 400);

    // ─── Move ───

    // Move root file into subfolder
    let (s, _) = rt.patch_json(&format!("/api/v1/storage/objects/docs/{root_file_id}"), &json!({"parent_id": subfolder_id})).await;
    assert_eq!(s, 200);

    // File no longer at root
    let (s, body) = rt.get_json("/api/v1/storage/objects/docs").await;
    assert_eq!(s, 200);
    let root_ids: Vec<&str> = body.as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert!(!root_ids.contains(&root_file_id.as_str()), "moved file should not be at root");

    // File visible in subfolder
    let (s, body) = rt.get_json(&format!("/api/v1/storage/objects/docs?parent_id={subfolder_id}")).await;
    assert_eq!(s, 200);
    let sub_ids: Vec<&str> = body.as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert!(sub_ids.contains(&root_file_id.as_str()), "moved file should be in subfolder");

    // Move to root (parent_id = null)
    let (s, _) = rt.patch_json(&format!("/api/v1/storage/objects/docs/{root_file_id}"), &json!({"parent_id": "null"})).await;
    assert_eq!(s, 200);

    // ─── Ancestors ───

    // Get ancestors of nested file (should return: folder → subfolder → file, ordered root-first)
    let (s, body) = rt.get_json(&format!("/api/v1/storage/objects/docs/{nested_file_id}/ancestors")).await;
    assert_eq!(s, 200, "ancestors: {body}");
    let ancestors = body.as_array().unwrap();
    assert!(ancestors.len() >= 2, "should have at least folder + subfolder in ancestors, got {}", ancestors.len());
    // First ancestor should be the root-level folder
    assert_eq!(ancestors[0]["id"].as_str().unwrap(), folder_id, "first ancestor should be root folder");
    // Second should be the subfolder
    assert_eq!(ancestors[1]["id"].as_str().unwrap(), subfolder_id, "second ancestor should be subfolder");
    // Last should be the file itself
    assert_eq!(ancestors.last().unwrap()["id"].as_str().unwrap(), nested_file_id, "last should be the target");

    // Ancestors of root-level object returns just itself
    let (s, body) = rt.get_json(&format!("/api/v1/storage/objects/docs/{folder_id}/ancestors")).await;
    assert_eq!(s, 200);
    let ancestors = body.as_array().unwrap();
    assert_eq!(ancestors.len(), 1);
    assert_eq!(ancestors[0]["id"].as_str().unwrap(), folder_id);

    // ─── Circular move prevention ───

    // Move folder into its own subfolder should be rejected
    let (s, body) = rt.patch_json(&format!("/api/v1/storage/objects/docs/{folder_id}"), &json!({"parent_id": subfolder_id})).await;
    assert_eq!(s, 400, "circular move should be rejected: {body}");

    // Move into self should be rejected
    let (s, _) = rt.patch_json(&format!("/api/v1/storage/objects/docs/{folder_id}"), &json!({"parent_id": folder_id})).await;
    assert_eq!(s, 400);

    // ─── Delete ───

    // Delete folder cascades (subfolder + nested file)
    let (s, _) = rt.delete_json(&format!("/api/v1/storage/objects/docs/{folder_id}")).await;
    assert_eq!(s, 200);

    // Subfolder and nested file should be gone
    let (s, body) = rt.get_json(&format!("/api/v1/storage/objects/docs?parent_id={subfolder_id}")).await;
    assert_eq!(s, 200); // endpoint still works, just returns empty
    assert!(body.as_array().unwrap().is_empty(), "subfolder contents should be cascaded");

    // Delete nonexistent returns 404
    let s = rt.delete(&format!("/api/v1/storage/objects/docs/{folder_id}")).await;
    assert_eq!(s, 404);

    // ─── Delete bucket with objects rejected ───
    let (s, _) = rt.delete_json("/api/v1/storage/buckets/docs").await;
    assert_eq!(s, 409, "bucket with objects should reject delete");

    // Clean up remaining objects then delete bucket
    let s = rt.delete(&format!("/api/v1/storage/objects/docs/{root_file_id}")).await;
    assert_eq!(s, 200);
    // Delete remaining (the "conflict.txt" and "invoices" inside folder which was a dup)
    let (_, remaining) = rt.get_json("/api/v1/storage/objects/docs").await;
    for obj in remaining.as_array().unwrap() {
        rt.delete(&format!("/api/v1/storage/objects/docs/{}", obj["id"].as_str().unwrap())).await;
    }
    let (s, _) = rt.delete_json("/api/v1/storage/buckets/docs").await;
    assert_eq!(s, 200);

    // ─── Unauthenticated access rejected ───
    assert_eq!(rt.get_unauthed("/api/v1/storage/buckets").await, 401);
    assert_eq!(rt.get_unauthed("/api/v1/storage/objects/default").await, 401);

    rt.shutdown().await;
}

fn streamed_file_part(
    name: &str,
    chunk_count: usize,
    chunk_bytes: usize,
    fill: u8,
) -> multipart::Part {
    let chunks = stream::iter((0..chunk_count).map(move |_| {
        Ok::<_, std::io::Error>(Bytes::from(vec![fill; chunk_bytes]))
    }));
    multipart::Part::stream_with_length(
        reqwest::Body::wrap_stream(chunks),
        (chunk_count * chunk_bytes) as u64,
    )
    .file_name(name.to_string())
    .mime_str("application/octet-stream")
    .unwrap()
}

#[tokio::test]
async fn large_object_survives_http_lifecycle_above_legacy_limit() {
    const CHUNK_BYTES: usize = 1024 * 1024;
    const CHUNK_COUNT: usize = 65;
    const TOTAL_BYTES: u64 = (CHUNK_BYTES * CHUNK_COUNT) as u64;

    let rt = TestRuntime::boot().await;
    let (status, body) = rt
        .post_json(
            "/api/v1/storage/buckets",
            &json!({"name": "large-files"}),
        )
        .await;
    assert_eq!(status, 201, "create bucket: {body}");

    let part = streamed_file_part("large.bin", CHUNK_COUNT, CHUNK_BYTES, 0x5a);
    let response = rt
        .client
        .post(rt.url("/api/v1/storage/objects/large-files/upload"))
        .bearer_auth(&rt.token)
        .multipart(multipart::Form::new().part("file", part))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 201, "large upload: {body}");
    assert_eq!(body["size"], TOTAL_BYTES);

    let file_id: uuid::Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let (inline_content, oid): (Option<Vec<u8>>, Option<Oid>) = sqlx::query_as(
        "SELECT content, content_oid FROM rootcx_system.storage_objects WHERE id = $1",
    )
    .bind(file_id)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert!(inline_content.is_none());
    let oid = oid.expect("large object OID");
    let stored_bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(octet_length(data)), 0)::BIGINT FROM pg_largeobject WHERE loid = $1",
    )
    .bind(oid)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert_eq!(stored_bytes as u64, TOTAL_BYTES);
    let response = rt
        .client
        .get(rt.url(&format!(
            "/api/v1/storage/objects/large-files/{file_id}"
        )))
        .bearer_auth(&rt.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut downloaded_bytes = 0_u64;
    let mut download = response.bytes_stream();
    while let Some(chunk) = download.next().await {
        let chunk = chunk.unwrap();
        assert!(chunk.iter().all(|byte| *byte == 0x5a));
        downloaded_bytes += chunk.len() as u64;
    }
    assert_eq!(downloaded_bytes, TOTAL_BYTES);

    assert_eq!(
        rt.delete(&format!(
            "/api/v1/storage/objects/large-files/{file_id}"
        ))
        .await,
        200
    );
    let object_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_largeobject_metadata WHERE oid = $1)",
    )
    .bind(oid)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert!(!object_exists);

    rt.shutdown().await;
}

#[tokio::test]
async fn rejected_uploads_leave_no_rows_or_large_objects() {
    const CHUNK_BYTES: usize = 1024 * 1024;

    let rt = TestRuntime::boot().await;
    let (status, body) = rt
        .post_json(
            "/api/v1/storage/buckets",
            &json!({"name": "bounded", "max_file_size": CHUNK_BYTES}),
        )
        .await;
    assert_eq!(status, 201, "create bucket: {body}");

    for (case, chunk_count) in [("empty", 0), ("over bucket limit", 2)] {
        let large_objects_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pg_largeobject_metadata")
                .fetch_one(rt.pool())
                .await
                .unwrap();
        let part = streamed_file_part(
            &format!("{case}.bin"),
            chunk_count,
            CHUNK_BYTES,
            0x42,
        );
        let response = rt
            .client
            .post(rt.url("/api/v1/storage/objects/bounded/upload"))
            .bearer_auth(&rt.token)
            .multipart(multipart::Form::new().part("file", part))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let response_body = response.text().await.unwrap();
        assert_eq!(status, 400, "{case}: {response_body}");

        let stored_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM rootcx_system.storage_objects WHERE bucket = 'bounded'",
        )
        .fetch_one(rt.pool())
        .await
        .unwrap();
        assert_eq!(stored_rows, 0, "{case}: rejected upload persisted metadata");
        let large_objects_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pg_largeobject_metadata")
                .fetch_one(rt.pool())
                .await
                .unwrap();
        assert_eq!(
            large_objects_after, large_objects_before,
            "{case}: rejected upload orphaned a Large Object"
        );
    }

    rt.shutdown().await;
}
