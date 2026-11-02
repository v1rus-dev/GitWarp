//! GitWarp design system: theme and components (GPUI).
//!
//! Geometry is shared by both themes and lives in [`metrics`]; colours live in
//! [`theme`]. The crate knows nothing about git.

use gpui::App;

pub mod actions;
pub mod assets;
pub mod components;
pub mod focus;
pub mod fonts;
pub mod i18n;
pub mod icon;
pub mod metrics;
pub mod overlay;
pub mod theme;

pub use assets::Assets;
pub use components::*;
pub use icon::{Icon, IconName};
pub use theme::{ActiveTheme, Appearance, Shadow, Theme, ThemeColors, ThemeMode};

/// Installs the theme and interaction infrastructure, then loads the fonts.
///
/// The theme goes first: a font error is not fatal for the caller, and every
/// view reads the theme global.
pub fn init(cx: &mut App) -> gpui::Result<()> {
    theme::init(cx);
    focus::init(cx);
    actions::bind_keys(cx);
    components::bind_text_input_keys(cx);
    components::toast::init(cx);
    components::menu::init(cx);
    overlay::init(cx);
    fonts::load(cx)
}
