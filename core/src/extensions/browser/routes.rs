use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{Extension, Json};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::queue::BrowserQueue;
use crate::api_error::ApiError;

#[derive(Deserialize)]
pub struct NavigateReq { pub url: String }

#[derive(Deserialize)]
pub struct ClickReq { pub ref_id: u32 }

#[derive(Deserialize)]
pub struct TypeReq { pub ref_id: u32, pub text: String }

#[derive(Deserialize)]
pub struct ScrollReq {
    #[serde(default = "d_dir")]  pub direction: String,
    #[serde(default = "d_amt")]  pub amount: u32,
}
fn d_dir() -> String { "down".into() }
fn d_amt() -> u32 { 3 }

#[derive(Serialize)]
pub struct OkResponse { pub ok: bool }

#[derive(Deserialize)]
pub struct CmdResult { #[serde(flatten)] pub data: Value }

pub async fn command_stream(
    Extension(q): Extension<Arc<BrowserQueue>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = q.subscribe();
    Sse::new(futures::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(msg) => Some((Ok(Event::default().event("browser_cmd").data(msg)), rx)),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) =>
                Some((Ok(Event::default().comment("lagged")), rx)),
            Err(_) => None,
        }
    })).keep_alive(KeepAlive::default())
}

pub async fn submit_result(
    Extension(q): Extension<Arc<BrowserQueue>>,
    Path(id): Path<u64>,
    Json(body): Json<CmdResult>,
) -> Json<OkResponse> {
    q.resolve(id, body.data).await;
    Json(OkResponse { ok: true })
}

pub async fn navigate(Extension(q): Extension<Arc<BrowserQueue>>, Json(b): Json<NavigateReq>) -> Result<Json<Value>, ApiError> {
    q.submit("navigate", json!({"url": b.url})).await.map(Json).map_err(ApiError::Internal)
}

pub async fn snapshot(Extension(q): Extension<Arc<BrowserQueue>>) -> Result<Json<Value>, ApiError> {
    q.submit("snapshot", json!({})).await.map(Json).map_err(ApiError::Internal)
}

pub async fn click(Extension(q): Extension<Arc<BrowserQueue>>, Json(b): Json<ClickReq>) -> Result<Json<Value>, ApiError> {
    q.submit("click", json!({"ref_id": b.ref_id})).await.map(Json).map_err(ApiError::Internal)
}

pub async fn type_text(Extension(q): Extension<Arc<BrowserQueue>>, Json(b): Json<TypeReq>) -> Result<Json<Value>, ApiError> {
    q.submit("type", json!({"ref_id": b.ref_id, "text": b.text})).await.map(Json).map_err(ApiError::Internal)
}

pub async fn scroll(Extension(q): Extension<Arc<BrowserQueue>>, Json(b): Json<ScrollReq>) -> Result<Json<Value>, ApiError> {
    q.submit("scroll", json!({"direction": b.direction, "amount": b.amount})).await.map(Json).map_err(ApiError::Internal)
}
