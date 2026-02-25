pub mod emitter;
pub mod layers;
pub mod presets;
pub mod registry;
pub mod types;

use emitter::Emitter;
use registry::PresetRegistry;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use types::{AnswerValue, ScaffoldContext};

pub use registry::PresetRegistry as Registry;
pub use types::{AnswerValue as Answer, PresetInfo, Question, RuntimePaths};

fn sanitize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == ' ')
        .collect::<String>()
        .to_lowercase()
        .replace(' ', "_")
}

/// Orchestrates scaffold: resolve preset → build context → run layers.
pub async fn create(
    root: &Path,
    name: &str,
    runtime: RuntimePaths,
    preset_id: &str,
    answers: HashMap<String, AnswerValue>,
    ai_config: Option<rootcx_shared_types::AiConfig>,
) -> Result<(), String> {
    let registry = PresetRegistry::new();
    let preset = registry.get(preset_id)?;
    let layers = preset.layers(&answers);

    let app_id = sanitize_name(name);
    let identifier = format!("com.rootcx.{app_id}");
    let mut h = std::hash::DefaultHasher::new();
    app_id.hash(&mut h);
    let port = 3000 + (h.finish() % 6000) as u16;

    let ctx = ScaffoldContext { app_id, identifier, port, runtime, answers, ai_config };

    let emitter = Emitter::new(root.to_path_buf());
    for layer in &layers {
        layer.emit(&ctx, &emitter).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_name_strips_dangerous_chars() {
        assert_eq!(sanitize_name("My App"), "my_app");
        assert_eq!(sanitize_name("evil</title><script>"), "eviltitlescript");
        assert_eq!(sanitize_name("test\"inject"), "testinject");
        assert_eq!(sanitize_name("a{b}c"), "abc");
        assert_eq!(sanitize_name("hello_world"), "hello_world");
    }
}
