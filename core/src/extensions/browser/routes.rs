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

#[derive(Deserialize)] pub(crate) struct NavigateReq { url: String }
#[derive(Deserialize)] pub(crate) struct ClickReq { ref_id: u32 }
#[derive(Deserialize)] pub(crate) struct TypeReq { ref_id: u32, text: String }
#[derive(Deserialize)] pub(crate) struct PressKeyReq { key: String }
#[derive(Deserialize)] pub(crate) struct SelectOptionReq { ref_id: u32, value: String }
#[derive(Deserialize)] pub(crate) struct HoverReq { ref_id: u32 }
#[derive(Deserialize)]
pub(crate) struct ScrollReq {
    #[serde(default = "d_dir")] direction: String,
    #[serde(default = "d_amt")] amount: u32,
}
fn d_dir() -> String { "down".into() }
fn d_amt() -> u32 { 3 }

#[derive(Deserialize)]
pub(crate) struct SnapshotReq { #[serde(default)] mode: Option<String> }

#[derive(Serialize)] pub(crate) struct OkResponse { ok: bool }
#[derive(Deserialize)] pub(crate) struct CmdResult { #[serde(flatten)] data: Value }

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

pub async fn snapshot(Extension(q): Extension<Arc<BrowserQueue>>, Json(b): Json<SnapshotReq>) -> Result<Json<Value>, ApiError> {
    q.submit("snapshot", json!({"mode": b.mode})).await.map(Json).map_err(ApiError::Internal)
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

pub async fn press_key(Extension(q): Extension<Arc<BrowserQueue>>, Json(b): Json<PressKeyReq>) -> Result<Json<Value>, ApiError> {
    q.submit("press_key", json!({"key": b.key})).await.map(Json).map_err(ApiError::Internal)
}

pub async fn select_option(Extension(q): Extension<Arc<BrowserQueue>>, Json(b): Json<SelectOptionReq>) -> Result<Json<Value>, ApiError> {
    q.submit("select_option", json!({"ref_id": b.ref_id, "value": b.value})).await.map(Json).map_err(ApiError::Internal)
}

pub async fn hover(Extension(q): Extension<Arc<BrowserQueue>>, Json(b): Json<HoverReq>) -> Result<Json<Value>, ApiError> {
    q.submit("hover", json!({"ref_id": b.ref_id})).await.map(Json).map_err(ApiError::Internal)
}
