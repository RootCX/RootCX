use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{Layer, ScaffoldContext};
use std::future::Future;
use std::pin::Pin;

const BACKEND_WORKER: &str = include_str!("../../../templates/backend-worker.ts");

/// Emits: backend/index.ts worker template
pub struct BackendLayer;

impl Layer for BackendLayer {
    fn emit<'a>(&'a self, _ctx: &'a ScaffoldContext, e: &'a Emitter) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            e.write("backend/index.ts", BACKEND_WORKER).await
        })
    }
}
