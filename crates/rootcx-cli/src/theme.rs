use cliclack::{Theme, ThemeState};
use console::Style;

pub struct RootcxTheme;

// 256-color 33 (dodger blue) is the closest match to #009EFF
const BRAND: u8 = 33;

impl Theme for RootcxTheme {
    fn bar_color(&self, _state: &ThemeState) -> Style {
        Style::new().color256(BRAND)
    }

    fn state_symbol_color(&self, _state: &ThemeState) -> Style {
        Style::new().color256(BRAND)
    }
}
