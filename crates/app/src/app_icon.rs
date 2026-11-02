//! The GitWarp app mark: window icon and the in-app logo.

use std::sync::{Arc, OnceLock};

use gpui::{img, px, Image, ImageFormat, IntoElement, Pixels, Styled};

static WINDOW_ICON: OnceLock<Option<Arc<image::RgbaImage>>> = OnceLock::new();
static LOGO_LARGE: OnceLock<Arc<Image>> = OnceLock::new();
static LOGO_SMALL: OnceLock<Arc<Image>> = OnceLock::new();

/// The X11 window icon, decoded once from the embedded 256px png.
///
/// Wayland ignores it: the compositor looks the icon up by `app_id`
/// (`dev.v1rus.gitwarp`) in the installed desktop entry instead.
pub fn window_icon() -> Option<Arc<image::RgbaImage>> {
    WINDOW_ICON
        .get_or_init(|| {
            let bytes = include_bytes!("../assets/app-icon/gitwarp-256.png");
            match image::load_from_memory(bytes) {
                Ok(image) => Some(Arc::new(image.to_rgba8())),
                Err(error) => {
                    log::warn!("could not decode the app icon: {error}");
                    None
                }
            }
        })
        .clone()
}

/// The app mark as an image element. Uses the thicker small mark at
/// 32px and below, where the full mark's strokes turn into mush.
pub fn logo(size: Pixels) -> impl IntoElement {
    let image = if size <= px(32.) {
        small_logo()
    } else {
        large_logo()
    };
    img(image).size(size).flex_none()
}

fn large_logo() -> Arc<Image> {
    LOGO_LARGE
        .get_or_init(|| {
            Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                include_bytes!("../assets/app-icon/gitwarp.svg").to_vec(),
            ))
        })
        .clone()
}

fn small_logo() -> Arc<Image> {
    LOGO_SMALL
        .get_or_init(|| {
            Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                include_bytes!("../assets/app-icon/gitwarp-small.svg").to_vec(),
            ))
        })
        .clone()
}
