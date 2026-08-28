use crate::emitter::Emitter;
use crate::types::{Layer, LayerFuture, ScaffoldContext};

const BACKEND_WORKER: &str = include_str!("../../templates/backend-worker.ts");
const BACKEND_TYPES: &str = include_str!("../../templates/rootcx-worker.d.ts");

pub struct BackendLayer;

impl Layer for BackendLayer {
    fn emit<'a>(&'a self, _ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        Box::pin(async move {
            e.write("backend/index.ts", BACKEND_WORKER).await?;
            e.write("backend/rootcx-worker.d.ts", BACKEND_TYPES).await
        })
    }
}
