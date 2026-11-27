mod harness;

use harness::TestRuntime;
use reqwest::Method;
use serde_json::{Value, json};

async fn upload(rt: &TestRuntime, app: &str, bytes: &'static [u8]) -> Value {
    let (status, file) = rt
        .upload(
            &format!("/api/v1/apps/{app}/storage/upload"),
            "sample.pdf",
            "application/pdf",
            bytes,
        )
        .await;
    assert_eq!(status, 201, "{file}");
    json!({"fileId": file["file_id"]})
}

#[tokio::test]
async fn publication_lifecycle_is_scoped_and_revocable() {
    let rt = TestRuntime::boot().await;
    rt.install("gallery", "pictures").await;
    rt.install("brochures", "documents").await;
    let admin = &rt.token;
    let member = rt.create_user("gallery-member@test.local").await;
    let channel = "/api/v1/apps/gallery/file-channels/media";
    let publish = "/api/v1/apps/gallery/file-publications/media/cover";
    let public = "/api/v1/public/apps/gallery/files/media/cover";
    assert_eq!(
        rt.request_as(Method::GET, channel, &member, None).await.0,
        403
    );
    let (status, pending) = rt.get_json(channel).await;
    assert_eq!(status, 200);
    assert_eq!(pending["enabled"], false);
    let approved = json!({"installationId": pending["installationId"]});
    assert_eq!(
        rt.put_json(channel, &json!({"installationId":uuid::Uuid::new_v4()}))
            .await
            .0,
        409
    );
    let first = upload(&rt, "gallery", b"%PDF-1.7\nfirst").await;
    let second = upload(&rt, "gallery", b"%PDF-1.7\nsecond").await;
    assert_eq!(rt.get_unauthed(public).await, 404);
    assert_eq!(
        rt.request_as(Method::PUT, channel, &member, Some(&approved))
            .await
            .0,
        403
    );
    assert_eq!(
        rt.request_as(Method::PUT, channel, admin, Some(&approved))
            .await
            .0,
        204
    );
    assert_eq!(
        rt.request_as(Method::PUT, publish, &member, Some(&first))
            .await
            .0,
        403
    );
    let (_, role) = rt.post_json("/api/v1/roles", &json!({
        "name":"gallery_editor", "permissions":["app:gallery:storage.write","app:gallery:storage.publish.media"]
    })).await;
    let member_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email=$1")
            .bind("gallery-member@test.local")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert!(
        rt.post_json(
            "/api/v1/roles/assign",
            &json!({"user_id":member_id,"role":"gallery_editor"})
        )
        .await
        .0
        .is_success(),
        "{role}"
    );
    // A channel editor still cannot publish someone else's private upload.
    assert_eq!(
        rt.request_as(Method::PUT, publish, &member, Some(&first))
            .await
            .0,
        404
    );
    let documents = "/api/v1/apps/gallery/file-channels/documents";
    assert_eq!(rt.put_json(documents, &approved).await.0, 204);
    assert_eq!(
        rt.request_as(
            Method::PUT,
            "/api/v1/apps/gallery/file-publications/documents/fees",
            &member,
            Some(&first)
        )
        .await
        .0,
        403
    );
    let foreign = upload(&rt, "brochures", b"%PDF-1.7\nother").await;
    assert_eq!(rt.put_json(publish, &foreign).await.0, 404);
    let unknown_format = upload(&rt, "gallery", b"plain text").await;
    assert_eq!(rt.put_json(publish, &unknown_format).await.0, 400);
    assert_eq!(rt.put_json(publish, &first).await.0, 204);
    let url = rt.url(public);
    let head = rt.client.head(&url).send().await.unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(head.headers()["content-type"], "application/pdf");
    let response = rt
        .client
        .get(&url)
        .header("Range", "bytes=0-3")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 206);
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"%PDF");
    let original = format!(
        "/api/v1/apps/gallery/storage/{}",
        first["fileId"].as_str().unwrap()
    );
    assert_eq!(rt.delete(&original).await, 409);
    assert_eq!(rt.put_json(publish, &second).await.0, 204);
    assert_eq!(
        rt.client
            .get(&url)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .as_ref(),
        b"%PDF-1.7\nsecond"
    );
    assert!(rt.delete(&original).await.is_success());
    assert_eq!(rt.delete(publish).await, 204);
    assert_eq!(rt.get_unauthed(public).await, 404);
    assert_eq!(rt.put_json(publish, &second).await.0, 204);
    assert_eq!(rt.delete(channel).await, 204);
    assert_eq!(rt.get_unauthed(public).await, 404);
    assert_eq!(
        rt.request_as(Method::PUT, channel, admin, Some(&approved))
            .await
            .0,
        204
    );
    assert_eq!(rt.get_unauthed(public).await, 404);
    assert_eq!(rt.put_json(publish, &second).await.0, 204);
    let (deleted, receipt) = rt.delete_application("gallery").await;
    assert!(deleted.is_success(), "{receipt}");
    assert_eq!(rt.get_unauthed(public).await, 404);
    rt.install("gallery", "pictures").await;
    let (status, fresh) = rt.get_json(channel).await;
    assert_eq!(status, 200);
    assert_eq!(fresh["enabled"], false);
    assert_ne!(approved["installationId"], fresh["installationId"]);
    assert_eq!(rt.put_json(channel, &approved).await.0, 409);
    rt.shutdown().await;
}

#[tokio::test]
async fn partial_reads_follow_the_current_representation_for_legacy_and_streamed_uploads() {
    use uuid::Uuid;
    let rt = TestRuntime::boot().await;
    rt.install("gallery", "pictures").await;
    let channel = "/api/v1/apps/gallery/file-channels/media";
    let publish = "/api/v1/apps/gallery/file-publications/media/asset";
    let url = rt.url("/api/v1/public/apps/gallery/files/media/asset");
    let (status, pending) = rt.get_json(channel).await;
    assert_eq!(status, 200);
    let approved = json!({"installationId":pending["installationId"]});
    assert_eq!(rt.put_json(channel, &approved).await.0, 204);

    // Cross a stream chunk boundary; the archived inline backend must expose the same bytes.
    let mut bytes: Vec<u8> = (0..300_000).map(|n| (n % 251) as u8).collect();
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let (status, uploaded) = rt
        .upload(
            "/api/v1/apps/gallery/storage/upload",
            "picture",
            "application/octet-stream",
            &bytes,
        )
        .await;
    assert_eq!(status, 201);
    let legacy_id: Uuid = sqlx::query_scalar(
        "INSERT INTO rootcx_system.files (app_id,name,content_type,size,content,uploaded_by)
         SELECT 'gallery','legacy-image','application/octet-stream',$1,$2,id FROM rootcx_system.users
         WHERE email='admin@test.local' RETURNING id",
    ).bind(bytes.len() as i64).bind(&bytes).fetch_one(rt.pool()).await.unwrap();
    // Older rows with both representations are served inline by the private reader.
    // A different OID signature must not change the public MIME or body.
    let mixed_id: Uuid = sqlx::query_scalar(
        "INSERT INTO rootcx_system.files(app_id,name,content_type,size,content,content_oid,uploaded_by)
         SELECT app_id,name,content_type,size,content,lo_from_bytea(0,$2),uploaded_by
         FROM rootcx_system.files WHERE id=$1 RETURNING id"
    ).bind(legacy_id).bind(b"%PDF-1.7".as_slice()).fetch_one(rt.pool()).await.unwrap();
    let mut last_tag = None;
    for file in [
        uploaded["file_id"].clone(),
        json!(legacy_id),
        json!(mixed_id),
    ] {
        assert_eq!(rt.put_json(publish, &json!({"fileId":file})).await.0, 204);
        let head = rt.client.head(&url).send().await.unwrap();
        assert_eq!(head.status(), 200);
        assert_eq!(head.headers()["content-type"], "image/png");
        assert_eq!(head.headers()["content-disposition"], "inline");
        assert_eq!(head.headers()["content-length"], "300000");
        assert_eq!(head.headers()["cache-control"], "no-store");
        assert_eq!(head.headers()["x-content-type-options"], "nosniff");
        let tag = head.headers()["etag"].clone();
        assert!(head.bytes().await.unwrap().is_empty());
        if let Some(stale) = last_tag.replace(tag.clone()) {
            assert_ne!(stale, tag);
            let response = rt
                .client
                .get(&url)
                .header("Range", "bytes=-2")
                .header("If-Range", stale)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(response.bytes().await.unwrap().as_ref(), bytes.as_slice());
        }
        for (range, expected, bounds) in [
            (
                "bytes=1999-279999",
                "bytes 1999-279999/300000",
                1999..280000,
            ),
            (
                "bytes=299998-",
                "bytes 299998-299999/300000",
                299998..300000,
            ),
            ("bytes=-2", "bytes 299998-299999/300000", 299998..300000),
        ] {
            let response = rt
                .client
                .get(&url)
                .header("Range", range)
                .header("If-Range", tag.clone())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 206, "{range}");
            assert_eq!(response.headers()["content-range"], expected);
            assert_eq!(
                response.headers()["content-length"]
                    .to_str()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap(),
                bounds.len()
            );
            assert_eq!(response.bytes().await.unwrap().as_ref(), &bytes[bounds]);
        }
        let unsatisfiable = rt
            .client
            .get(&url)
            .header("Range", "bytes=300000-")
            .send()
            .await
            .unwrap();
        assert_eq!(unsatisfiable.status(), 416);
        assert_eq!(unsatisfiable.headers()["content-range"], "bytes */300000");
        assert!(unsatisfiable.bytes().await.unwrap().is_empty());
    }
    rt.shutdown().await;
}
