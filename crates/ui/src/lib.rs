//! GitWarp design system: theme and components (GPUI).
//!
//! Geometry is shared by both themes and lives in [`metrics`]; colours live in
//! [`theme`]. The crate knows nothing about git.

pub mod metrics;
pub mod theme;

pub use theme::{ActiveTheme, Appearance, Shadow, Theme, ThemeColors, ThemeMode};
