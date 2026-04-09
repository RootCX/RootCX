mod tauri_layer;

use std::collections::HashMap;
use std::path::Path;

pub use rootcx_scaffold::types::{AnswerValue as Answer, PresetInfo, Question};
pub use rootcx_scaffold::Registry;

/// Scaffold a project with the Studio's TauriLayer added on top.
pub async fn create(
    root: &Path,
    name: &str,
    preset_id: &str,
    answers: HashMap<String, rootcx_scaffold::types::AnswerValue>,
) -> Result<(), String> {
    let extra: Vec<Box<dyn rootcx_scaffold::types::Layer>> =
        vec![Box::new(tauri_layer::TauriLayer)];
    rootcx_scaffold::create(root, name, preset_id, answers, extra).await
}
