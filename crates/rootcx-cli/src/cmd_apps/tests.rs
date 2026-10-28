use super::remove;
use rootcx_client::RuntimeClient;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(
    replies: Vec<(u16, Value)>,
) -> (RuntimeClient, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = RuntimeClient::new(&format!("http://{}", listener.local_addr().unwrap()));
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 1024];
                let size = stream.read(&mut chunk).await.unwrap();
                if size == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..size]);
                if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            requests.push(
                String::from_utf8(bytes)
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned(),
            );
            let body = body.to_string();
            let response = format!(
                "HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
        // Keep listening briefly: an erroneous fallback must be observed, not
        // hidden by a connection-refused error after the scripted responses.
        if let Ok(Ok((mut stream, _))) =
            tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept()).await
        {
            let mut bytes = [0; 4096];
            let size = stream.read(&mut bytes).await.unwrap();
            requests.push(
                String::from_utf8_lossy(&bytes[..size])
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned(),
            );
            stream
                .write_all(
                    b"HTTP/1.1 500 Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                )
                .await
                .unwrap();
        }
        requests
    });
    (client, task)
}

fn apps(kind: &str) -> Value {
    json!([{"id":"target","name":"Target","version":"1.0.0","status":"installed","type":kind,"entities":[]}])
}

#[tokio::test]
async fn remove_selects_the_existing_lifecycle_for_each_app_kind() {
    for (kind, replies, expected) in [
        (
            "app",
            vec![
                (200, json!({"id":"receipt","applicationName":"Target"})),
                (200, json!({"state":"deleting"})),
                (200, json!({"state":"succeeded"})),
            ],
            vec![
                "POST /api/v1/apps/target/deletion HTTP/1.1",
                "POST /api/v1/app-deletions/receipt/confirm HTTP/1.1",
                "GET /api/v1/app-deletions/receipt HTTP/1.1",
            ],
        ),
        (
            "agent",
            vec![(200, json!({}))],
            vec!["DELETE /api/v1/apps/target HTTP/1.1"],
        ),
        (
            "integration",
            vec![(200, json!({}))],
            vec!["DELETE /api/v1/integrations/catalog/target HTTP/1.1"],
        ),
    ] {
        let mut responses = vec![(200, apps(kind))];
        responses.extend(replies);
        let (client, task) = server(responses).await;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            remove(&client, "target", true),
        )
        .await
        .unwrap();
        assert!(result.is_ok(), "{kind}: {result:?}");
        let requests = task.await.unwrap();
        assert_eq!(requests[0], "GET /api/v1/apps HTTP/1.1");
        assert_eq!(requests[1..], expected, "{kind}");
    }
}

#[tokio::test]
async fn refused_confirmation_never_falls_back_to_legacy_delete() {
    for status in [403, 409, 500] {
        let (client, task) = server(vec![
            (200, apps("app")),
            (200, json!({"id":"receipt","applicationName":"Target"})),
            (status, json!({"error":"refused"})),
        ])
        .await;
        let result = remove(&client, "target", true).await;
        assert!(result.is_err(), "{status}");
        let requests = task.await.unwrap();
        assert_eq!(requests.len(), 3, "{status}");
        assert!(
            requests
                .iter()
                .all(|request| !request.starts_with("DELETE")),
            "{status}"
        );
    }
}
