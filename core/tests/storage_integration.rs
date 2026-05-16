mod harness;
use harness::TestRuntime;
use reqwest::{Method, multipart};
use serde_json::{Value, json};

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
    let (s, _) = rt.get_json(&format!("/api/v1/storage/objects/docs?parent_id={subfolder_id}")).await;
    assert_eq!(s, 200); // endpoint still works, just returns empty
    let (s, body) = rt.get_json(&format!("/api/v1/storage/objects/docs?parent_id={subfolder_id}")).await;
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
