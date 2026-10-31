//! The GitWarp colour theme and the active-theme global.
//!
//! Both themes share the geometry in [`crate::metrics`] and differ only in
//! colour. Access the active theme through [`ActiveTheme`], e.g.
//! `cx.theme().colors.bg`.
#![allow(clippy::unusual_byte_groupings)] // `0xRRGGBB_AA` mirrors the CSS colour.

use gpui::{
    hsla, point, px, rgb, rgba, App, BoxShadow, Global, Hsla, Pixels, Window, WindowAppearance,
};

/// The resolved colour theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

/// Which theme the user asked for. `System` follows the OS appearance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    System,
    Dark,
    Light,
}

/// The accent colour chosen in Appearance settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Accent {
    #[default]
    Blue,
    Violet,
    Green,
    Orange,
}

/// The row density chosen in Appearance settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Density {
    Compact,
    #[default]
    Normal,
    Roomy,
}

impl Density {
    /// The height of a list or tree row, in logical pixels.
    pub fn row_height(self) -> Pixels {
        match self {
            Density::Compact => px(22.),
            Density::Normal => px(26.),
            Density::Roomy => px(30.),
        }
    }
}

/// The appearance options that live above a theme's dark/light palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeStyle {
    pub accent: Accent,
    pub density: Density,
    /// Base interface font size, 12–14 px.
    pub ui_font_size: f32,
    /// Diff and editor font size, 9–20 px.
    pub mono_font_size: f32,
}

impl Default for ThemeStyle {
    fn default() -> Self {
        Self {
            accent: Accent::default(),
            density: Density::default(),
            ui_font_size: 13.,
            mono_font_size: 12.,
        }
    }
}

/// One drop-shadow layer.
#[derive(Clone, Copy, Debug)]
pub struct Shadow {
    pub offset_y: Pixels,
    pub blur: Pixels,
    /// Extra size on every side; 1 turns the layer into a 1px ring.
    pub spread: Pixels,
    pub color: Hsla,
}

/// One colour token per field.
#[derive(Clone, Copy, Debug)]
pub struct ThemeColors {
    pub bg: Hsla,
    pub surface: Hsla,
    pub sidebar: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    pub selection: Hsla,
    pub selection_fg: Hsla,
    pub selection_sub: Hsla,
    pub selection_inactive: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub disabled: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_fg: Hsla,
    pub accent_soft: Hsla,
    pub focus: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub info: Hsla,
    pub st_added: Hsla,
    pub st_modified: Hsla,
    pub st_deleted: Hsla,
    pub st_renamed: Hsla,
    pub st_untracked: Hsla,
    pub st_conflict: Hsla,
    pub diff_add_bg: Hsla,
    pub diff_add_strong: Hsla,
    pub diff_del_bg: Hsla,
    pub diff_del_strong: Hsla,
    pub diff_hunk_bg: Hsla,
    pub code_kw: Hsla,
    pub code_str: Hsla,
    pub code_num: Hsla,
    pub code_com: Hsla,
    pub code_ty: Hsla,
    pub code_fn: Hsla,
    pub overlay: Hsla,
}

impl ThemeColors {
    fn dark() -> Self {
        let text = solid(0xd9dce2);
        let muted = solid(0x9aa1ad);
        Self {
            bg: solid(0x181b20),
            surface: solid(0x1d2026),
            sidebar: solid(0x15171c),
            hover: solid(0x22262d),
            active: solid(0x2a2f38),
            selection: solid(0x263c5e),
            selection_fg: text,
            selection_sub: muted,
            selection_inactive: solid(0x232d3e),
            border: solid(0x2a2e36),
            border_strong: solid(0x4a515d),
            text,
            muted,
            disabled: solid(0x6b7280),
            accent: solid(0x6ea4f7),
            accent_hover: solid(0x86b5f9),
            accent_fg: solid(0x0f1722),
            accent_soft: translucent(0x6ea4f7_26),
            focus: solid(0x86b5f9),
            error: solid(0xe5767f),
            warning: solid(0xe5c07b),
            success: solid(0x8cc88f),
            info: solid(0x6bb2f0),
            st_added: solid(0x8cc88f),
            st_modified: solid(0xe5c07b),
            st_deleted: solid(0xe5767f),
            st_renamed: solid(0x6bb2f0),
            st_untracked: solid(0xb794e0),
            st_conflict: solid(0xe0966c),
            diff_add_bg: translucent(0x8cc88f_1c),
            diff_add_strong: translucent(0x8cc88f_47),
            diff_del_bg: translucent(0xe5767f_1f),
            diff_del_strong: translucent(0xe5767f_4d),
            diff_hunk_bg: translucent(0x6ea4f7_17),
            code_kw: solid(0xc98be0),
            code_str: solid(0x98c379),
            code_num: solid(0xd9a26b),
            code_com: solid(0x8d94a1),
            code_ty: solid(0xe5c07b),
            code_fn: solid(0x6bb2f0),
            overlay: translucent(0x06080c_99),
        }
    }

    fn light() -> Self {
        let text = solid(0x1f2329);
        let muted = solid(0x5f6773);
        Self {
            bg: solid(0xffffff),
            surface: solid(0xffffff),
            sidebar: solid(0xfbfcfe),
            hover: solid(0xf2f6fd),
            active: solid(0xe7effc),
            selection: solid(0xdce9fc),
            selection_fg: text,
            selection_sub: muted,
            selection_inactive: solid(0xe9f1fe),
            border: solid(0xe8ecf3),
            border_strong: solid(0xb9c0cb),
            text,
            muted,
            disabled: solid(0xa3aab5),
            accent: solid(0x2a66d6),
            accent_hover: solid(0x1f55b8),
            accent_fg: solid(0xffffff),
            accent_soft: translucent(0x2a66d6_1a),
            focus: solid(0x2a66d6),
            error: solid(0xc93434),
            warning: solid(0x8f5f00),
            success: solid(0x1c7a3a),
            info: solid(0x1f5fbf),
            st_added: solid(0x1c7a3a),
            st_modified: solid(0x8f5f00),
            st_deleted: solid(0xc93434),
            st_renamed: solid(0x1f5fbf),
            st_untracked: solid(0x7b3fb0),
            st_conflict: solid(0xa8480f),
            diff_add_bg: translucent(0x2ea043_17),
            diff_add_strong: translucent(0x2ea043_3d),
            diff_del_bg: translucent(0xd63c3c_14),
            diff_del_strong: translucent(0xd63c3c_38),
            diff_hunk_bg: translucent(0x2a66d6_0f),
            code_kw: solid(0x8f3fb0),
            code_str: solid(0x35761b),
            code_num: solid(0x9a5600),
            code_com: solid(0x66707d),
            code_ty: solid(0x8a5a00),
            code_fn: solid(0x1f5fbf),
            overlay: translucent(0x1f293a_38),
        }
    }
}

/// A resolved theme: colours, shadows and the graph lane lightness.
#[derive(Clone, Debug)]
pub struct Theme {
    pub appearance: Appearance,
    pub colors: ThemeColors,
    pub shadow_pop: [Shadow; 2],
    /// The light theme's primary-button shadow.
    pub primary_shadow: Option<Shadow>,
    /// Lane lightness: 0.62 on dark, 0.45 on light.
    pub graph_lightness: f32,
    /// Height of one list or tree row.
    pub row_height: Pixels,
    /// Base interface font size.
    pub ui_font_size: Pixels,
    /// Diff and editor font size.
    pub mono_font_size: Pixels,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            appearance: Appearance::Dark,
            colors: ThemeColors::dark(),
            shadow_pop: [
                Shadow {
                    offset_y: px(10.),
                    blur: px(32.),
                    spread: px(0.),
                    color: translucent(0x000000_6b),
                },
                Shadow {
                    offset_y: px(0.),
                    blur: px(0.),
                    spread: px(1.),
                    color: translucent(0xffffff_0a),
                },
            ],
            primary_shadow: None,
            graph_lightness: 0.62,
            row_height: px(26.),
            ui_font_size: px(13.),
            mono_font_size: px(12.),
        }
    }

    pub fn light() -> Self {
        Self {
            appearance: Appearance::Light,
            colors: ThemeColors::light(),
            shadow_pop: [
                Shadow {
                    offset_y: px(10.),
                    blur: px(32.),
                    spread: px(0.),
                    color: translucent(0x182438_1f),
                },
                Shadow {
                    offset_y: px(0.),
                    blur: px(0.),
                    spread: px(1.),
                    color: translucent(0x182438_0a),
                },
            ],
            primary_shadow: Some(Shadow {
                offset_y: px(1.),
                blur: px(2.),
                spread: px(0.),
                color: translucent(0x2a66d6_40),
            }),
            graph_lightness: 0.45,
            row_height: px(26.),
            ui_font_size: px(13.),
            mono_font_size: px(12.),
        }
    }

    /// Applies the user's accent, density and type sizes to this palette.
    pub fn apply_style(&mut self, style: ThemeStyle) {
        let accent = match style.accent {
            Accent::Blue => self.colors.accent,
            Accent::Violet => self.colors.st_untracked,
            Accent::Green => self.colors.st_added,
            Accent::Orange => self.colors.st_conflict,
        };
        self.colors.accent = accent;
        self.colors.accent_hover = accent;
        self.colors.accent_soft = accent.opacity(0.16);
        self.colors.focus = accent;
        self.row_height = style.density.row_height();
        self.ui_font_size = px(style.ui_font_size.clamp(12., 14.));
        self.mono_font_size = px(style.mono_font_size.clamp(9., 20.));
    }

    /// The floating-layer shadow: both CSS layers of `shadow-pop`.
    pub fn pop_shadow(&self) -> Vec<BoxShadow> {
        self.shadow_pop
            .iter()
            .map(|shadow| BoxShadow {
                color: shadow.color,
                offset: point(px(0.), shadow.offset_y),
                blur_radius: shadow.blur,
                spread_radius: shadow.spread,
                inset: false,
            })
            .collect()
    }

    /// The colour a given accent choice resolves to in this palette.
    pub fn accent_color(&self, accent: Accent) -> Hsla {
        match accent {
            Accent::Blue => match self.appearance {
                Appearance::Dark => solid(0x6ea4f7),
                Appearance::Light => solid(0x2a66d6),
            },
            Accent::Violet => self.colors.st_untracked,
            Accent::Green => self.colors.st_added,
            Accent::Orange => self.colors.st_conflict,
        }
    }

    /// Lane colour: hues spread by the golden angle, so neighbours never clash.
    pub fn lane_color(&self, lane: u32) -> Hsla {
        let hue = (lane as f32 * 137.508 / 360.).fract();
        hsla(hue, 0.65, self.graph_lightness, 1.)
    }
}

/// Opaque colour from a `0xRRGGBB` literal.
fn solid(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Colour from a `0xRRGGBBAA` literal.
fn translucent(hex: u32) -> Hsla {
    rgba(hex).into()
}

/// The active mode and its resolved theme.
struct ThemeState {
    mode: ThemeMode,
    style: ThemeStyle,
    theme: Theme,
}

impl Global for ThemeState {}

impl ThemeState {
    fn resolve(mode: ThemeMode, style: ThemeStyle, appearance: WindowAppearance) -> Self {
        let mut theme = match mode {
            ThemeMode::Dark => Theme::dark(),
            ThemeMode::Light => Theme::light(),
            ThemeMode::System => match appearance {
                WindowAppearance::Light | WindowAppearance::VibrantLight => Theme::light(),
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme::dark(),
            },
        };
        theme.apply_style(style);
        Self { mode, style, theme }
    }
}

/// Installs the theme global, following the system appearance.
pub fn init(cx: &mut App) {
    let state = ThemeState::resolve(
        ThemeMode::System,
        ThemeStyle::default(),
        cx.window_appearance(),
    );
    cx.set_global(state);
}

/// Switches the theme mode and repaints every window.
pub fn set_mode(mode: ThemeMode, cx: &mut App) {
    let style = cx
        .try_global::<ThemeState>()
        .map(|state| state.style)
        .unwrap_or_default();
    let state = ThemeState::resolve(mode, style, cx.window_appearance());
    cx.set_global(state);
    cx.refresh_windows();
}

/// Applies the accent, density and type sizes and repaints every window.
pub fn set_style(style: ThemeStyle, cx: &mut App) {
    let mode = cx
        .try_global::<ThemeState>()
        .map(|state| state.mode)
        .unwrap_or_default();
    let state = ThemeState::resolve(mode, style, cx.window_appearance());
    cx.set_global(state);
    cx.refresh_windows();
}

/// Keeps `System` mode in step with OS appearance changes for this window.
pub fn follow_system(window: &mut Window, cx: &mut App) {
    sync(window.appearance(), cx);
    window
        .observe_window_appearance(|window, cx| sync(window.appearance(), cx))
        .detach();
}

/// Applies `appearance` when the mode is `System`.
fn sync(appearance: WindowAppearance, cx: &mut App) {
    let Some(state) = cx.try_global::<ThemeState>() else {
        return;
    };
    if state.mode != ThemeMode::System {
        return;
    }
    let next = ThemeState::resolve(ThemeMode::System, state.style, appearance);
    if next.theme.appearance == state.theme.appearance {
        return;
    }
    cx.set_global(next);
    cx.refresh_windows();
}

/// Read the active theme from `App`, and therefore from contexts and views.
pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        &self.global::<ThemeState>().theme
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_style_applies_density_accent_and_type_sizes() {
        let mut theme = Theme::dark();
        theme.apply_style(ThemeStyle {
            accent: Accent::Green,
            density: Density::Roomy,
            ui_font_size: 14.,
            mono_font_size: 16.,
        });
        assert_eq!(theme.row_height, px(30.));
        assert_eq!(theme.ui_font_size, px(14.));
        assert_eq!(theme.mono_font_size, px(16.));
        assert_eq!(theme.colors.accent, theme.colors.st_added);
        assert_eq!(theme.colors.focus, theme.colors.st_added);
        assert_eq!(
            theme.accent_color(Accent::Violet),
            theme.colors.st_untracked
        );
        assert_eq!(theme.accent_color(Accent::Orange), theme.colors.st_conflict);

        // Out-of-range values are clamped instead of shrinking the UI away.
        theme.apply_style(ThemeStyle {
            ui_font_size: 40.,
            mono_font_size: 1.,
            ..ThemeStyle::default()
        });
        assert_eq!(theme.ui_font_size, px(14.));
        assert_eq!(theme.mono_font_size, px(9.));
        assert_eq!(theme.row_height, px(26.));
    }
}
