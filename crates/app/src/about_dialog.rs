//! The "About GitWarp" dialog: mark, version, links and the licence line.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    div, px, App, ClickEvent, FontWeight, Global, InteractiveElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};

use ui::metrics::{FS_BASE, FS_SM, FS_XL};
use ui::theme::ActiveTheme;
use ui::{dialog, overlay, Button, Dialog, DialogFocus};

const REPO_URL: &str = "https://github.com/v1rus-dev/GitWarp";
const REPO_LABEL: &str = "github.com/v1rus-dev/GitWarp";
/// Fallback of the Legal link when no third-party licences file ships with the app.
const LICENSE_URL: &str = "https://github.com/v1rus-dev/GitWarp/blob/main/LICENSE-MIT";
const LICENSES_FILE: &str = "THIRD-PARTY-LICENSES.html";
const LICENSE: &str = "MIT OR Apache-2.0";
const COPYRIGHT: &str = "Copyright © 2026 Yegor Cheprasov";

/// The layer of the About dialog while it is open.
struct OpenAbout(overlay::LayerId);

impl Global for OpenAbout {}

/// Opens the dialog; does nothing while one is already open. `git` is the
/// resolved-git label put into "Copy details".
pub(crate) fn open(git: SharedString, window: &mut Window, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenAbout>() {
        if overlay::layer_view(open.0, window, cx).is_some() {
            return;
        }
    }
    let close_focus = cx.focus_handle().tab_stop(true);
    let copy_focus = cx.focus_handle().tab_stop(true);
    let layer: Rc<Cell<Option<overlay::LayerId>>> = Rc::new(Cell::new(None));
    let layer_for_build = layer.clone();
    let id = dialog::open(window, cx, move |_, cx| {
        build(&layer_for_build, &close_focus, &copy_focus, &git, cx)
    });
    layer.set(Some(id));
    cx.set_global(OpenAbout(id));
}

fn build(
    layer: &Rc<Cell<Option<overlay::LayerId>>>,
    close_focus: &gpui::FocusHandle,
    copy_focus: &gpui::FocusHandle,
    git: &SharedString,
    cx: &App,
) -> Dialog {
    let colors = cx.theme().colors;
    let version = env!("CARGO_PKG_VERSION");

    let link = |id: &'static str, label: SharedString, url: String| {
        div()
            .id(id)
            .text_size(FS_BASE)
            .text_color(colors.accent)
            .cursor_pointer()
            .hover(move |style| style.text_color(colors.accent_hover).underline())
            .child(label)
            .on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut App| cx.open_url(&url))
    };

    let body = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.))
        .py(px(8.))
        .text_center()
        .child(crate::app_icon::logo(px(72.)))
        .child(
            div()
                .mt(px(8.))
                .text_size(FS_XL)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text)
                .child("GitWarp"),
        )
        .child(
            div()
                .text_size(FS_BASE)
                .text_color(colors.muted)
                .child(ui::t!("dialogs.about.version", version = version)),
        )
        .child(
            div()
                .mt(px(8.))
                .child(link("about-repo", REPO_LABEL.into(), REPO_URL.into())),
        )
        .child(
            div()
                .mt(px(8.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(2.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(LICENSE)
                .child(COPYRIGHT),
        )
        .child(div().mt(px(8.)).child(link(
            "about-legal",
            ui::t!("dialogs.about.legal"),
            legal_url(),
        )));

    let details = details_text(
        version,
        std::env::consts::OS,
        std::env::consts::ARCH,
        git.as_ref(),
    );
    let copy = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
        crate::copy_with_toast(details.clone(), ui::t!("dialogs.about.copied"), window, cx);
    };
    let close = {
        let layer = layer.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            if let Some(id) = layer.take() {
                dialog::close(id, window, cx);
            }
        }
    };

    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            Button::new("about-copy")
                .label(ui::t!("dialogs.about.copy"))
                .min_width(px(96.))
                .focus_handle(copy_focus.clone())
                .on_click(copy),
        )
        .child(
            Button::primary("about-close")
                .label(ui::t!("dialogs.about.close"))
                .min_width(px(72.))
                .focus_handle(close_focus.clone())
                .on_click(close),
        );

    Dialog::new(ui::t!("dialogs.about.title"))
        .width(px(380.))
        .compact_body()
        .body(body)
        .footer(footer)
        .focus_order(vec![
            DialogFocus::control(copy_focus.clone()),
            DialogFocus::control(close_focus.clone()),
        ])
        .autofocus(close_focus.clone())
        .default_button(close_focus.clone())
}

/// The text "Copy details" puts on the clipboard.
fn details_text(version: &str, os: &str, arch: &str, git: &str) -> String {
    let os = match os {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    };
    format!("GitWarp {version}\n{os} {arch}\n{git}")
}

/// Where the Legal link goes: the shipped third-party licences file, else the
/// repository's licence page.
fn legal_url() -> String {
    let file = std::env::current_exe()
        .ok()
        .and_then(|exe| licenses_path(&exe, cfg!(target_os = "macos")))
        .filter(|path| path.is_file());
    match file {
        Some(path) => file_url(&path.to_string_lossy()),
        None => LICENSE_URL.to_string(),
    }
}

/// Where the release bundle keeps the licences file: next to the executable,
/// on macOS in `Contents/Resources` (the executable is in `Contents/MacOS`).
fn licenses_path(exe: &Path, mac: bool) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let dir = if mac {
        dir.parent()?.join("Resources")
    } else {
        dir.to_path_buf()
    };
    Some(dir.join(LICENSES_FILE))
}

/// A `file:///` URL for an absolute path in Windows (`C:\x`) or Unix (`/x`)
/// form. Spaces and non-ASCII bytes are percent-encoded.
fn file_url(path: &str) -> String {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let path = path.replace('\\', "/");
    let mut url = String::from("file://");
    if !path.starts_with('/') {
        url.push('/');
    }
    let drive = path.as_bytes().get(1) == Some(&b':');
    for (i, byte) in path.bytes().enumerate() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/')
            || (drive && i == 1 && byte == b':');
        if keep {
            url.push(byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_text_lists_version_platform_and_git() {
        assert_eq!(
            details_text("0.4.0", "windows", "x86_64", "git 2.54.0 · bundled"),
            "GitWarp 0.4.0\nWindows x86_64\ngit 2.54.0 · bundled"
        );
        assert_eq!(
            details_text("1", "macos", "aarch64", "g"),
            "GitWarp 1\nmacOS aarch64\ng"
        );
        assert_eq!(
            details_text("1", "freebsd", "x", "g"),
            "GitWarp 1\nfreebsd x\ng"
        );
    }

    #[test]
    fn file_url_for_a_windows_path() {
        assert_eq!(
            file_url(r"C:\Program Files\GitWarp\THIRD-PARTY-LICENSES.html"),
            "file:///C:/Program%20Files/GitWarp/THIRD-PARTY-LICENSES.html"
        );
        assert_eq!(file_url(r"\\?\D:\app\x.html"), "file:///D:/app/x.html");
    }

    #[test]
    fn file_url_for_a_unix_path() {
        assert_eq!(
            file_url("/Applications/GitWarp.app/Contents/Resources/THIRD-PARTY-LICENSES.html"),
            "file:///Applications/GitWarp.app/Contents/Resources/THIRD-PARTY-LICENSES.html"
        );
    }

    #[test]
    fn file_url_encodes_spaces_and_non_ascii() {
        assert_eq!(
            file_url("/home/Ян/my app/x.html"),
            "file:///home/%D0%AF%D0%BD/my%20app/x.html"
        );
        assert_eq!(file_url("/a#b%c?d"), "file:///a%23b%25c%3Fd");
    }

    #[test]
    fn licenses_file_location() {
        assert_eq!(
            licenses_path(Path::new("/opt/gitwarp/gitwarp"), false),
            Some(PathBuf::from("/opt/gitwarp").join(LICENSES_FILE))
        );
        assert_eq!(
            licenses_path(Path::new("/A/GitWarp.app/Contents/MacOS/GitWarp"), true),
            Some(PathBuf::from("/A/GitWarp.app/Contents/Resources").join(LICENSES_FILE))
        );
    }
}
