use crate::presets::BlankPreset;
use crate::types::{Preset, PresetInfo, Question};

pub struct PresetRegistry {
    presets: Vec<Box<dyn Preset>>,
}

impl PresetRegistry {
    pub fn new() -> Self {
        Self { presets: vec![Box::new(BlankPreset)] }
    }

    pub fn list(&self) -> Vec<PresetInfo> {
        self.presets.iter().map(|p| p.info()).collect()
    }

    pub fn questions(&self, id: &str) -> Result<Vec<Question>, String> {
        self.get(id).map(|p| p.questions())
    }

    pub fn get(&self, id: &str) -> Result<&dyn Preset, String> {
        self.presets
            .iter()
            .find(|p| p.info().id == id)
            .map(|p| p.as_ref())
            .ok_or_else(|| format!("unknown preset: {id}"))
    }
}
