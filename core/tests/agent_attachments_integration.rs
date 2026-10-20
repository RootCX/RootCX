mod harness;

use axum::{Json, Router, response::IntoResponse, routing::post};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::Sha256;
use std::time::Duration;
use uuid::Uuid;

fn webhook(
    rt: &harness::TestRuntime,
    provider: &str,
    channel: Uuid,
    body: Value,
) -> reqwest::RequestBuilder {
    let body = body.to_string();
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"local-media-only").unwrap();
    mac.update(format!("core:{channel}\n{timestamp}\n{body}").as_bytes());
    rt.client
        .post(rt.url(&format!("/api/v1/channels/{provider}/{channel}/webhook")))
        .header("content-type", "application/json")
        .header("x-rootcx-timestamp", timestamp)
        .header(
            "x-rootcx-signature",
            hex::encode(mac.finalize().into_bytes()),
        )
        .body(body)
        .timeout(Duration::from_secs(60))
}

fn model_response() -> String {
    [
        json!({"type":"message_start","message":{"id":"local","type":"message","role":"assistant","model":"local-vision","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"photo understood"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}}),
        json!({"type":"message_stop"}),
    ].into_iter().map(|v| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap())).collect()
}

// Opt-in because this test bundles the locally installed production Assistant.
// Only the model endpoint and messaging provider are simulated; HTTP, identity,
// delegation, storage, one-use downloads and the Assistant worker are real.
#[tokio::test]
#[ignore = "requires ROOTCX_TEST_ASSISTANT_DIR with installed dependencies and isolated local TEST_DATABASE_URL"]
async fn photos_cross_real_core_and_assistant_for_web_and_managed_channels() {
    let database: sqlx::postgres::PgConnectOptions = std::env::var("TEST_DATABASE_URL")
        .expect("local test database required")
        .parse()
        .unwrap();
    assert!(
        matches!(database.get_host(), "127.0.0.1" | "localhost" | "::1"),
        "photo recipe requires a loopback database"
    );
    assert_eq!(database.get_database(), Some("rootcx_test"));
    let source = std::path::PathBuf::from(
        std::env::var("ROOTCX_TEST_ASSISTANT_DIR").expect("Assistant source required"),
    );
    let bundle_dir = tempfile::tempdir().unwrap();
    let bundle = bundle_dir.path().join("index.js");
    let bundled = std::process::Command::new("bun")
        .args(["build", "--target=bun", "--outfile"])
        .arg(&bundle)
        .arg(source.join("index.ts"))
        .output()
        .expect("local bun build");
    assert!(
        bundled.status.success(),
        "{}",
        String::from_utf8_lossy(&bundled.stderr)
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (model_tx, mut model_rx) = tokio::sync::mpsc::channel::<Value>(16);
    let (reply_tx, mut reply_rx) = tokio::sync::mpsc::channel::<Value>(16);
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new()
            .route("/authorize", post(|Json(body): Json<Value>| async move { Json(json!({"authorized":true,"grantId":body["to"],"coreUserId":body["coreUserId"]})) }))
            .route("/send", post(move |Json(body): Json<Value>| { let tx = reply_tx.clone(); async move { tx.send(body).await.unwrap(); Json(json!({"ok":true})) } }))
            .route("/v1/messages", post(move |Json(body): Json<Value>| { let tx = model_tx.clone(); async move {
                tx.send(body).await.unwrap(); ([("content-type", "text/event-stream")], model_response()).into_response()
            } }))).await.unwrap();
    });
    unsafe {
        std::env::set_var("ROOTCX_TENANT_REF", "local-media-test");
        std::env::set_var("ROOTCX_WHATSAPP_TENANT_REF", "local-media-test");
        std::env::set_var("ROOTCX_CHANNEL_GATEWAY_URL", &base);
        std::env::set_var("ROOTCX_CHANNEL_RELAY_KEY", "local-media-only");
        std::env::set_var("ROOTCX_WHATSAPP_PHONE_NUMBER", "15550000000");
        std::env::set_var("ROOTCX_CHANNEL_TELEGRAM_USERNAME", "local_media_test");
    }
    let rt = harness::TestRuntime::boot().await;
    for (key, value) in [
        ("ROOTCX_API_KEY", "local-only"),
        ("ROOTCX_LLM_ENDPOINT", base.as_str()),
    ] {
        let (status, body) = rt
            .post_json(
                "/api/v1/apps/assistant/secrets",
                &json!({"key":key,"value":value}),
            )
            .await;
        assert!(status.is_success(), "{body}");
    }
    let (status, body) = rt.post_json("/api/v1/llm-models", &json!({"id":"local","name":"Local vision","provider":"rootcx","model":"local-vision","config":{},"is_default":true})).await;
    assert!(status.is_success(), "{status}: {body}");
    let backend = std::fs::read(&bundle).unwrap();
    let config =
        br#"{"name":"Assistant","memory":{"enabled":false},"supervision":{"mode":"autonomous"}}"#;
    let (status, body) = rt
        .deploy(
            "assistant",
            &harness::make_tar_gz(&[("index.ts", &backend), ("agent.json", config)]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a5foAAAAASUVORK5CYII=").unwrap();
    let human: Uuid =
        sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email='admin@test.local'")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    let (status, upload) = rt
        .upload(
            "/api/v1/apps/assistant/storage/upload",
            "chantier.png",
            "image/png",
            &png,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{upload}");
    let file = upload["file_id"].as_str().unwrap();
    for provider in ["web", "whatsapp", "telegram"] {
        let caption = format!("Photo chantier depuis {provider}: décris ce qui est visible.");
        if provider == "web" {
            let response = rt
                .client
                .post(rt.url("/api/v1/apps/assistant/agent/invoke"))
                .bearer_auth(&rt.token)
                .json(&json!({"message":caption,"file_ids":[file]}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let events = response.text().await.unwrap();
            assert!(events.contains("photo understood"), "{events}");
        } else {
            let channel: Uuid =
                sqlx::query_scalar("SELECT id FROM rootcx_system.channels WHERE provider=$1")
                    .bind(provider)
                    .fetch_one(rt.pool())
                    .await
                    .unwrap();
            let grant = Uuid::new_v4().to_string();
            let (status, body) = rt
                .post_json(
                    &format!("/api/v1/channels/{channel}/managed-link"),
                    &json!({"grantId":grant}),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let invalid = webhook(&rt, provider, channel, json!({"id":Uuid::new_v4(),"chat_id":grant,"text":"malformed image","attachments":[{"name":"bad.png","content_type":"image/png","data":STANDARD.encode(b"not a PNG") }]})).send().await.unwrap();
            assert_eq!(
                invalid.status(),
                StatusCode::BAD_REQUEST,
                "{provider} validates signed bytes before admission"
            );
            let response = webhook(&rt, provider, channel, json!({"id":Uuid::new_v4(),"chat_id":grant,"text":caption,"attachments":[{"name":"chantier.png","content_type":"image/png","data":STANDARD.encode(&png)}]})).send().await.unwrap();
            assert!(
                response.status().is_success(),
                "{}",
                response.text().await.unwrap()
            );
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    let reply = reply_rx.recv().await.unwrap();
                    if reply["text"] == "photo understood" {
                        break;
                    }
                }
            })
            .await
            .expect("model reply delivered to originating channel");
        }
        let request = tokio::time::timeout(Duration::from_secs(5), model_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let content = request["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_array()
            .unwrap();
        assert!(
            content
                .iter()
                .any(|c| c["type"] == "text" && c["text"].as_str().unwrap().contains(&caption)),
            "{request}"
        );
        let image = content
            .iter()
            .find(|c| c["type"] == "image")
            .expect("real Assistant sent image blocks");
        assert_eq!(image["source"]["media_type"], "image/png");
        assert_eq!(
            STANDARD
                .decode(image["source"]["data"].as_str().unwrap())
                .unwrap(),
            png
        );
    }
    let owners: Vec<Option<Uuid>> =
        sqlx::query_scalar("SELECT uploaded_by FROM rootcx_system.files WHERE app_id='assistant'")
            .fetch_all(rt.pool())
            .await
            .unwrap();
    assert_eq!(
        owners,
        vec![Some(human); 3],
        "all three entries attribute storage to the responsible human"
    );

    let token = rt.create_user("no-storage@test.local").await;
    sqlx::query("INSERT INTO rootcx_system.rbac_roles(name,permissions) VALUES('invoke-only',ARRAY['app:assistant:invoke'])").execute(rt.pool()).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments(user_id,role) SELECT id,'invoke-only' FROM rootcx_system.users WHERE email='no-storage@test.local'").execute(rt.pool()).await.unwrap();
    let (status, _) = rt
        .request_as(
            reqwest::Method::POST,
            "/api/v1/apps/assistant/agent/invoke",
            &token,
            Some(&json!({"message":"read private image","file_ids":[file]})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "invoke cannot bypass storage.read"
    );
    let (status, _) = rt
        .post_json(
            "/api/v1/apps/assistant/agent/invoke",
            &json!({"message":"too many","file_ids":vec![file;11]}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, malformed) = rt
        .upload(
            "/api/v1/apps/assistant/storage/upload",
            "bad.png",
            "image/png",
            b"not a PNG",
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = rt
        .post_json(
            "/api/v1/apps/assistant/agent/invoke",
            &json!({"message":"bad image","file_ids":[malformed["file_id"]]}),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "web shares channel image validation"
    );
    rt.install("other_media", "records").await;
    let (status, other) = rt
        .upload(
            "/api/v1/apps/other_media/storage/upload",
            "private.png",
            "image/png",
            &png,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = rt
        .post_json(
            "/api/v1/apps/assistant/agent/invoke",
            &json!({"message":"wrong app","file_ids":[other["file_id"]]}),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "file IDs stay scoped to the invoked app"
    );
    assert!(
        model_rx.try_recv().is_err(),
        "rejected inputs never reach the model"
    );
    server.abort();
}
