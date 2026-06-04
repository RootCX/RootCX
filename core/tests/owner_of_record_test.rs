//! owner_of_record invariant tests (Governance G4):
//! every non-human identity must have a human owner of record.

mod harness;

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

async fn create_sa(rt: &harness::TestRuntime, slug: &str) -> (Uuid, Value) {
    let (s, body) = rt
        .request_as(Method::POST, "/api/v1/service-accounts", &rt.token, Some(&json!({ "slug": slug })))
        .await;
    assert_eq!(s, StatusCode::CREATED, "create SA: {body}");
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    (id, body)
}

async fn create_credential(rt: &harness::TestRuntime, sa: Uuid) -> String {
    let (s, body) = rt
        .request_as(Method::POST, &format!("/api/v1/service-accounts/{sa}/credentials"), &rt.token, Some(&json!({ "name": "default" })))
        .await;
    assert_eq!(s, StatusCode::CREATED, "create credential: {body}");
    body["key"].as_str().unwrap().to_string()
}

async fn token_exchange(rt: &harness::TestRuntime, sa: Uuid, secret: &str) -> (StatusCode, Value) {
    let cid = sa.to_string();
    let r = rt.client.post(rt.url("/api/v1/auth/token"))
        .form(&[("grant_type", "client_credentials"), ("client_id", cid.as_str()), ("client_secret", secret)])
        .send().await.unwrap();
    let s = r.status();
    (s, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn sa_creation_sets_owner_of_record() {
    let rt = harness::TestRuntime::boot().await;
    let (sa_id, _) = create_sa(&rt, "owned-bot").await;

    let owner: Option<Uuid> = sqlx::query_scalar(
        "SELECT owner_of_record FROM rootcx_system.users WHERE id = $1",
    ).bind(sa_id).fetch_one(rt.pool()).await.unwrap();

    assert!(owner.is_some(), "owner_of_record must be set on creation");

    // The owner should be the admin who created it
    let admin_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'",
    ).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(owner.unwrap(), admin_id);

    rt.shutdown().await;
}

#[tokio::test]
async fn sa_without_owner_cannot_issue_token() {
    let rt = harness::TestRuntime::boot().await;
    let (sa_id, _) = create_sa(&rt, "orphan-bot").await;
    let secret = create_credential(&rt, sa_id).await;

    // Manually null out owner_of_record to simulate legacy/orphaned SA
    sqlx::query("UPDATE rootcx_system.users SET owner_of_record = NULL WHERE id = $1")
        .bind(sa_id).execute(rt.pool()).await.unwrap();

    let (status, body) = token_exchange(&rt, sa_id, &secret).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.to_string().contains("no owner of record"),
        "error must mention owner: {body}"
    );

    rt.shutdown().await;
}

#[tokio::test]
async fn transfer_ownership_works() {
    let rt = harness::TestRuntime::boot().await;
    let (sa_id, _) = create_sa(&rt, "transfer-bot").await;

    // Create a second human
    rt.post_unauthed("/api/v1/auth/register", &json!({"email": "new-owner@test.local", "password": "Str0ngPass1"})).await;
    let new_owner_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.users WHERE email = 'new-owner@test.local'",
    ).fetch_one(rt.pool()).await.unwrap();

    let (s, body) = rt.request_as(
        Method::POST,
        &format!("/api/v1/service-accounts/{sa_id}/transfer-ownership"),
        &rt.token,
        Some(&json!({ "new_owner": new_owner_id })),
    ).await;
    assert_eq!(s, StatusCode::OK, "transfer ownership: {body}");

    let owner: Uuid = sqlx::query_scalar(
        "SELECT owner_of_record FROM rootcx_system.users WHERE id = $1",
    ).bind(sa_id).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(owner, new_owner_id);

    rt.shutdown().await;
}

#[tokio::test]
async fn transfer_ownership_rejects_non_human() {
    let rt = harness::TestRuntime::boot().await;
    let (sa_id, _) = create_sa(&rt, "reject-bot").await;
    let (sa2_id, _) = create_sa(&rt, "not-a-human").await;

    let (s, body) = rt.request_as(
        Method::POST,
        &format!("/api/v1/service-accounts/{sa_id}/transfer-ownership"),
        &rt.token,
        Some(&json!({ "new_owner": sa2_id })),
    ).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "must reject non-human: {body}");
    assert!(body.to_string().contains("human"), "error must mention human: {body}");

    rt.shutdown().await;
}
