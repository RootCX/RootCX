use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{Layer, ScaffoldContext};
use std::future::Future;
use std::pin::Pin;

const BUTTON_TSX: &str = include_str!("../../../templates/components/button.tsx");
const INPUT_TSX: &str = include_str!("../../../templates/components/input.tsx");
const LABEL_TSX: &str = include_str!("../../../templates/components/label.tsx");
const CARD_TSX: &str = include_str!("../../../templates/components/card.tsx");

/// Emits: shadcn/ui base components (button, input, label, card)
pub struct UiKitLayer;

impl Layer for UiKitLayer {
    fn emit<'a>(&'a self, _ctx: &'a ScaffoldContext, e: &'a Emitter) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            e.write("src/components/ui/button.tsx", BUTTON_TSX).await?;
            e.write("src/components/ui/input.tsx", INPUT_TSX).await?;
            e.write("src/components/ui/label.tsx", LABEL_TSX).await?;
            e.write("src/components/ui/card.tsx", CARD_TSX).await?;
            Ok(())
        })
    }
}
