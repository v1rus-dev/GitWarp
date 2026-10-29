//! The GitWarp colour theme and the active-theme global.
//!
//! Both themes share the geometry in [`crate::metrics`] and differ only in
//! colour. Access the active theme through [`ActiveTheme`], e.g.
//! `cx.theme().colors.bg`.
#![allow(clippy::unusual_byte_groupings)] // `0xRRGGBB_AA` mirrors the CSS colour.

use gpui::{hsla, px, rgb, rgba, App, Global, Hsla, Pixels, Window, WindowAppearance};

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
    theme: Theme,
}

impl Global for ThemeState {}

impl ThemeState {
    fn resolve(mode: ThemeMode, appearance: WindowAppearance) -> Self {
        let theme = match mode {
            ThemeMode::Dark => Theme::dark(),
            ThemeMode::Light => Theme::light(),
            ThemeMode::System => match appearance {
                WindowAppearance::Light | WindowAppearance::VibrantLight => Theme::light(),
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme::dark(),
            },
        };
        Self { mode, theme }
    }
}

/// Installs the theme global, following the system appearance.
pub fn init(cx: &mut App) {
    let state = ThemeState::resolve(ThemeMode::System, cx.window_appearance());
    cx.set_global(state);
}

/// Switches the theme mode and repaints every window.
pub fn set_mode(mode: ThemeMode, cx: &mut App) {
    let state = ThemeState::resolve(mode, cx.window_appearance());
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
    let next = ThemeState::resolve(ThemeMode::System, appearance);
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
