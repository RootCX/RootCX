use super::*;
use clap::Parser;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};

fn snapshot() -> ActionApprovals {
    serde_json::from_value(json!({
        "revision": "00000000-0000-0000-0000-000000000001",
        "backendDigest": "sha256:reviewed-code",
        "installationId": "00000000-0000-0000-0000-000000000002",
        "actions": [{
            "id": "send_report",
            "authority": {"data": {"reports": ["read", "create", "update", "delete"]}},
            "approvalId": null,
            "status": "pending"
        }]
    }))
    .unwrap()
}

fn pinned(snapshot: &ActionApprovals) -> Review {
    Review {
        revision: snapshot.revision.clone(),
        digest: snapshot.backend_digest.clone(),
        installation: Some(snapshot.installation_id.clone()),
        yes: true,
    }
}

#[test]
fn noninteractive_approval_requires_every_review_identifier() {
    let flags = ["--revision", "--digest", "--installation"];
    let mut complete = vec![
        "rootcx",
        "apps",
        "actions",
        "approve",
        "app",
        "send_report",
        "--yes",
    ];
    for flag in flags {
        complete.extend([flag, "reviewed"]);
    }
    assert!(crate::Cli::try_parse_from(&complete).is_ok());
    for missing in flags {
        let mut args = vec![
            "rootcx",
            "apps",
            "actions",
            "approve",
            "app",
            "send_report",
            "--yes",
        ];
        for flag in flags {
            if flag != missing {
                args.extend([flag, "reviewed"]);
            }
        }
        let error = crate::Cli::try_parse_from(&args)
            .err()
            .expect("missing review pin must fail");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument,
            "{missing}: {error}"
        );
        assert!(error.to_string().contains(missing), "{missing}: {error}");
    }
}

#[test]
fn preview_keeps_full_authority_and_review_identifiers_visible() {
    let mut current = snapshot();
    current.actions[0].authority["futureCapability"] = json!({"scope": "all"});
    let output = preview("app", &current).unwrap();
    for value in [
        current.revision.as_deref().unwrap(),
        current.backend_digest.as_deref().unwrap(),
        &current.installation_id,
        &serde_json::to_string_pretty(&current.actions).unwrap(),
    ] {
        assert!(output.contains(value), "review hides {value}");
    }
}

#[test]
fn approval_requires_a_live_complete_snapshot_and_exact_pins() {
    for field in ["revision", "backendDigest", "installationId"] {
        for replacement in [Value::Null, json!(""), json!(" "), json!("changed")] {
            let mut current = snapshot();
            let review = pinned(&current);
            let value = replacement.as_str().map(str::to_owned);
            match field {
                "revision" => current.revision = value,
                "backendDigest" => current.backend_digest = value,
                _ => current.installation_id = value.unwrap_or_default(),
            }
            assert!(
                reviewed_request(&current, "send_report", &review).is_err(),
                "{field}: {replacement}"
            );
            if replacement != json!("changed") {
                assert!(
                    reviewed_request(&current, "send_report", &Review::default()).is_err(),
                    "{field}: {replacement}"
                );
            }
        }
    }
    assert!(reviewed_request(&snapshot(), "undeclared", &Review::default()).is_err());
    assert!(
        reviewed_request(
            &snapshot(),
            "send_report",
            &Review {
                yes: true,
                ..Review::default()
            }
        )
        .is_err()
    );
}

#[test]
fn redirected_input_cannot_implicitly_confirm_approval() {
    assert!(require_confirmation_mode(false, false).is_err());
    assert!(require_confirmation_mode(false, true).is_ok());
    assert!(require_confirmation_mode(true, false).is_ok());
}

#[tokio::test]
async fn cancelled_or_mismatched_review_never_posts() {
    for mismatch in [false, true] {
        let current = snapshot();
        let mut review = Review::default();
        if mismatch {
            review.digest = Some("unreviewed".into());
        }
        let (client, mut requests) = mock_core(&current, 200).await;
        let mut prompted = false;
        let result = approve(&client, "app", "send_report", &review, || {
            prompted = true;
            Ok(false)
        })
        .await;
        if mismatch {
            assert!(result.is_err());
            assert!(!prompted, "stale review must fail before prompting");
        } else {
            assert!(!result.unwrap());
            assert!(prompted);
        }
        assert!(requests.try_recv().unwrap().0.starts_with("GET "));
        assert!(
            requests.try_recv().is_err(),
            "cancelled/mismatched approval issued another request"
        );
    }
}

#[tokio::test]
async fn confirmation_posts_the_displayed_snapshot_and_conflict_is_not_retried() {
    for status in [200, 409] {
        let current = snapshot();
        let (client, mut requests) = mock_core(&current, status).await;
        let result = approve(&client, "app", "send_report", &Review::default(), || {
            Ok(true)
        })
        .await;
        assert_eq!(result.is_ok(), status == 200, "{status}: {result:?}");
        if status == 409 {
            assert!(format!("{:#}", result.unwrap_err()).contains("review changed"));
        }
        let (get, _) = requests.try_recv().unwrap();
        assert_eq!(get, "GET /api/v1/apps/app/action-approvals HTTP/1.1");
        let (post, body) = requests.try_recv().unwrap();
        assert_eq!(
            post,
            "POST /api/v1/apps/app/action-approvals/send_report HTTP/1.1"
        );
        assert_eq!(
            body,
            json!({
                "revision": current.revision,
                "backendDigest": current.backend_digest,
                "installationId": current.installation_id,
            })
        );
        assert!(
            requests.try_recv().is_err(),
            "approval must not refetch or retry on conflict"
        );
    }
}

#[tokio::test]
async fn pinned_noninteractive_approval_skips_prompt_and_revocation_targets_one_action() {
    let current = snapshot();
    let (client, mut requests) = mock_core(&current, 204).await;
    assert!(
        approve(&client, "app", "send_report", &pinned(&current), || {
            panic!("fully pinned --yes must not prompt")
        })
        .await
        .unwrap()
    );
    requests.try_recv().unwrap();
    requests.try_recv().unwrap();
    client
        .revoke_action_approval("app", "send_report")
        .await
        .unwrap();
    let (delete, body) = requests.try_recv().unwrap();
    assert_eq!(
        delete,
        "DELETE /api/v1/apps/app/action-approvals/send_report HTTP/1.1"
    );
    assert_eq!(body, Value::Null);
    assert!(requests.try_recv().is_err());
}

async fn mock_core(
    snapshot: &ActionApprovals,
    mutation_status: u16,
) -> (RuntimeClient, mpsc::UnboundedReceiver<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = RuntimeClient::new(&format!("http://{}", listener.local_addr().unwrap()));
    client.set_token(Some("test-admin-token".into()));
    let snapshot = serde_json::to_string(snapshot).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut buf = [0; 1024];
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0, "request ended before headers");
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            assert!(
                headers
                    .to_lowercase()
                    .contains("authorization: bearer test-admin-token")
            );
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while bytes.len() < header_end + length {
                let mut buf = [0; 1024];
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0, "request ended before body");
                bytes.extend_from_slice(&buf[..n]);
            }
            let request = headers.lines().next().unwrap().to_owned();
            let body = if length == 0 {
                Value::Null
            } else {
                serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
            };
            let (status, response) = if request.starts_with("GET ") {
                (200, snapshot.as_str())
            } else if mutation_status == 409 {
                (409, r#"{"error":"review changed"}"#)
            } else {
                (mutation_status, "")
            };
            tx.send((request, body)).unwrap();
            stream.write_all(format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                response.len()
            ).as_bytes()).await.unwrap();
        }
    });
    (client, rx)
}
