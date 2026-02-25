use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_shared_types::ToolDescriptor;

use super::{Tool, ToolContext};
use crate::extensions::browser::queue::BrowserQueue;

pub struct BrowserTool {
    queue: Arc<BrowserQueue>,
}

impl BrowserTool {
    pub fn new(queue: Arc<BrowserQueue>) -> Self {
        Self { queue }
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "browser".into(),
            description: concat!(
                "Control a real browser. Available actions:\n\n",
                "navigate  — Go to URL. Returns page snapshot with element refs and readable text.\n",
                "snapshot  — Get current page state. Use mode=\"efficient\" for just interactive elements, or mode=\"full\" (default) for full content.\n",
                "click     — Click element by ref (e.g. ref_id: 5 for [e5]). Returns OK, not a snapshot.\n",
                "type      — Type text into element. Clears existing value first. Returns OK.\n",
                "press_key — Press Enter, Tab, Escape, ArrowDown, Space, etc. Returns OK.\n",
                "select_option — Select dropdown option by value or visible text. Returns OK.\n",
                "scroll    — Scroll the page. Returns OK.\n",
                "hover     — Hover element to reveal tooltips/menus. Returns OK.\n\n",
                "IMPORTANT: click/type/scroll/press_key/hover return OK without a snapshot. Call snapshot after to see the result.\n\n",
                "Snapshot format:\n",
                "  -- h1: \"Dashboard\" --\n",
                "  Welcome to your dashboard\n",
                "  -- navigation: \"Main\" --\n",
                "    [e1] link \"Home\"\n",
                "    [e2] link \"Settings\"\n",
                "  [e3] textbox \"Search\" [focused]\n",
                "  [e4] button \"Submit\"\n",
                "  Error: please fill in all fields\n\n",
                "Lines with [eN] are interactive — use their number as ref_id.\n",
                "Lines without [eN] are readable text content.\n",
                "States: [checked], [expanded], [collapsed], [disabled], [focused], [required]\n",
                "[nth=N] disambiguates duplicate elements (e.g. two \"Add to cart\" buttons).",
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["navigate", "click", "type", "scroll", "press_key", "select_option", "hover", "snapshot"], "description": "Browser action" },
                    "url": { "type": "string", "description": "URL (for navigate)" },
                    "ref_id": { "type": "integer", "description": "Element ref number [eN] (for click/type/select_option/hover)" },
                    "text": { "type": "string", "description": "Text to type (for type)" },
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction" },
                    "amount": { "type": "integer", "description": "Scroll viewport units (default: 3)" },
                    "key": { "type": "string", "description": "Key name (for press_key): Enter, Tab, Escape, ArrowDown, Space, etc." },
                    "value": { "type": "string", "description": "Option value or text (for select_option)" },
                    "mode": { "type": "string", "enum": ["full", "efficient"], "description": "Snapshot mode (for snapshot): full=all content, efficient=interactive only" }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let action = ctx.args.get("action").and_then(|v| v.as_str())
            .ok_or("missing: action")?;
        let mut params = serde_json::Map::new();
        for key in ["url", "ref_id", "text", "direction", "amount", "key", "value", "mode"] {
            if let Some(v) = ctx.args.get(key) { params.insert(key.into(), v.clone()); }
        }
        self.queue.submit(action, JsonValue::Object(params)).await
    }
}
