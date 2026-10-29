mod graph_paint;
mod history_view;
mod repo_view;
mod settings;
mod theme;
mod welcome;
mod workspace;

use gpui::{
    prelude::*, px, size, Action, App, Bounds, Global, KeyBinding, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions,
};

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use git_core::{Git, GitChoice};
use settings::{Settings, SettingsStore, ThemePref};
use workspace::Workspace;

/// The loaded settings and where they are saved.
struct AppSettings {
    path: Option<PathBuf>,
    settings: Settings,
}

impl Global for AppSettings {}

/// A keybinding whose `secondary-` modifier is resolved for the host OS.
fn binding(cx: &App, keystrokes: &str, action: impl Action) -> KeyBinding {
    KeyBinding::load(keystrokes, Box::new(action), None, false, None, cx).expect("valid keystroke")
}

/// Applies `f` to the loaded settings and saves them.
fn update_settings(cx: &mut App, f: impl FnOnce(&mut Settings)) {
    if cx.try_global::<AppSettings>().is_none() {
        return;
    }
    let state = cx.global_mut::<AppSettings>();
    f(&mut state.settings);
    if let Some(path) = &state.path {
        if let Err(e) = SettingsStore::new(path.clone()).save(&state.settings) {
            log::warn!("failed to save settings to {}: {e}", path.display());
        }
    }
}

const USAGE: &str = "Usage: gitwarp [--git <auto|system|bundled|PATH>] [REPO]

Options:
  --git <VALUE>  Which git to run (default: auto; env: GITWARP_GIT, the flag wins)
  -h, --help     Print this help

Without REPO the welcome screen opens.";

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let (path, git) = match parse_args(std::env::args_os().skip(1), std::env::var_os("GITWARP_GIT"))
    {
        Ok(Cli::Run { path, git }) => (path, git),
        Ok(Cli::Help) => {
            println!("{USAGE}");
            return;
        }
        Err(message) => {
            eprintln!("gitwarp: {message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    // Resolved once, before any window exists, so the probe cannot freeze the UI.
    let git = Git::resolve(git)
        .map(|r| {
            if let Some(reason) = r.fallback {
                log::warn!("bundled git unusable, using system git: {reason}");
            }
            Arc::new(r.git)
        })
        .map_err(|e| e.to_string());

    let config_path = settings::config_dir().map(|dir| dir.join("settings.json"));
    let loaded = match &config_path {
        Some(path) => SettingsStore::new(path.clone()).load(),
        None => {
            log::warn!("no configuration directory found; using default settings");
            Settings::default()
        }
    };

    gpui_platform::application()
        .with_assets(ui::Assets)
        .run(move |cx: &mut App| {
            if let Err(e) = ui::init(cx) {
                log::warn!("failed to load the bundled fonts: {e}");
            }
            if loaded.theme != ThemePref::System {
                ui::theme::set_mode(loaded.theme.into(), cx);
            }
            // Repository shortcuts. `secondary-` is Ctrl on Linux and Windows
            // and Cmd on macOS. The views handle them, with a window-root
            // fallback for when nothing holds the focus.
            let shortcuts = [
                binding(cx, "secondary-b", repo_view::ToggleSidebar),
                binding(cx, "secondary-j", repo_view::ToggleDetails),
                binding(cx, "secondary-k", repo_view::FocusSearch),
                binding(cx, "secondary-o", repo_view::OpenRepoSwitcher),
                binding(cx, "secondary-shift-b", repo_view::OpenBranchSwitcher),
            ];
            cx.bind_keys(shortcuts);
            cx.set_global(AppSettings {
                path: config_path,
                settings: loaded,
            });
            let window_size = if path.is_some() {
                size(px(1200.), px(800.))
            } else {
                size(px(960.), px(620.))
            };
            let bounds = Bounds::centered(None, window_size, cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Always request client-side decorations: on GNOME Wayland
                    // server-side ones leave the window without any frame.
                    window_decorations: Some(WindowDecorations::Client),
                    window_background: WindowBackgroundAppearance::Transparent,
                    window_min_size: Some(size(px(620.), px(420.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("GitWarp".into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    app_id: Some("dev.v1rus.gitwarp".into()),
                    ..Default::default()
                },
                |window, cx| {
                    ui::theme::follow_system(window, cx);
                    cx.new(|cx| Workspace::new(path, git, window, cx))
                },
            )
            .expect("failed to open the main window");
            cx.activate(true);
        });
}

#[derive(Debug, PartialEq, Eq)]
enum Cli {
    Run {
        path: Option<PathBuf>,
        git: GitChoice,
    },
    Help,
}

/// `[--git <auto|system|bundled|PATH>] [repo]`. The flag wins over
/// `GITWARP_GIT`; without a repo the welcome screen opens.
fn parse_args(
    mut args: impl Iterator<Item = OsString>,
    env_git: Option<OsString>,
) -> Result<Cli, String> {
    let mut path = None;
    let mut git = env_git.map(|v| GitChoice::parse_os(&v));
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => return Ok(Cli::Help),
            Some("--git") => {
                let value = args.next().ok_or("--git needs a value")?;
                git = Some(GitChoice::parse_os(&value));
            }
            Some(a) if a.starts_with('-') => match a.strip_prefix("--git=") {
                Some(v) => git = Some(GitChoice::parse(v)),
                None => return Err(format!("unknown option {a}")),
            },
            _ => path = Some(PathBuf::from(arg)),
        }
    }
    Ok(Cli::Run {
        path,
        git: git.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str], env: Option<&str>) -> Result<Cli, String> {
        parse_args(args.iter().map(OsString::from), env.map(OsString::from))
    }

    fn run(path: Option<&str>, git: GitChoice) -> Result<Cli, String> {
        Ok(Cli::Run {
            path: path.map(Into::into),
            git,
        })
    }

    #[test]
    fn flag_wins_over_env() {
        assert_eq!(parse(&[], None), run(None, GitChoice::Auto));
        assert_eq!(
            parse(&["repo"], Some("system")),
            run(Some("repo"), GitChoice::System)
        );
        assert_eq!(parse(&[], Some("")), run(None, GitChoice::Auto));
        assert_eq!(
            parse(&["--git", "bundled", "repo"], Some("system")),
            run(Some("repo"), GitChoice::Bundled)
        );
        assert_eq!(
            parse(&["repo", "--git=/opt/git"], None),
            run(Some("repo"), GitChoice::Custom("/opt/git".into()))
        );
    }

    #[test]
    fn rejects_bad_options() {
        assert!(parse(&["--git"], None).is_err());
        assert!(parse(&["--frobnicate"], None).is_err());
        assert_eq!(parse(&["-h"], None), Ok(Cli::Help));
        assert_eq!(parse(&["x", "--help"], None), Ok(Cli::Help));
    }

    #[cfg(unix)]
    #[test]
    fn keeps_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;
        let raw = OsString::from_vec(vec![b'r', 0xff]);
        let cli = parse_args([raw.clone()].into_iter(), None);
        assert_eq!(
            cli,
            Ok(Cli::Run {
                path: Some(raw.into()),
                git: GitChoice::Auto
            })
        );
    }
}
