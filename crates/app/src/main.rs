mod changes_view;
mod details;
mod diff_model;
mod format;
mod graph_paint;
mod history_search;
mod history_state;
mod history_view;
mod init_flow;
mod open_dialog;
mod repo_view;
mod settings;
mod sidebar;
mod syntax;
#[cfg(test)]
mod test_support;
mod theme;
mod welcome;
mod workspace;

use gpui::{
    prelude::*, px, size, App, Bounds, Global, KeyBinding, TitlebarOptions,
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
    /// A save is running on the background executor.
    save_in_flight: bool,
    /// The settings changed while that save ran: save once more after it.
    dirty: bool,
}

impl Global for AppSettings {}

/// Applies `f` to the loaded settings and saves them on the background
/// executor. The in-memory copy changes at once; a save that is already in
/// flight makes this one just mark the state dirty, so a burst of changes
/// costs one more write, not one write each.
fn update_settings(cx: &mut App, f: impl FnOnce(&mut Settings)) {
    let (path, snapshot) = {
        if cx.try_global::<AppSettings>().is_none() {
            return;
        }
        let state = cx.global_mut::<AppSettings>();
        f(&mut state.settings);
        let Some(path) = state.path.clone() else {
            return;
        };
        if state.save_in_flight {
            state.dirty = true;
            return;
        }
        state.save_in_flight = true;
        (path, state.settings.clone())
    };
    save_settings(cx, path, snapshot);
}

/// Writes one snapshot off the UI thread, then saves again if the settings
/// changed while it ran.
fn save_settings(cx: &mut App, path: PathBuf, snapshot: Settings) {
    cx.spawn(async move |cx| {
        let save_path = path.clone();
        let saved = cx
            .background_executor()
            .spawn(async move { SettingsStore::new(save_path).save(&snapshot) })
            .await;
        if let Err(e) = saved {
            log::warn!("failed to save settings to {}: {e}", path.display());
        }
        cx.update(finish_save);
    })
    .detach();
}

fn finish_save(cx: &mut App) {
    let next = {
        if cx.try_global::<AppSettings>().is_none() {
            return;
        }
        let state = cx.global_mut::<AppSettings>();
        state.save_in_flight = false;
        if !state.dirty {
            return;
        }
        state.dirty = false;
        let Some(path) = state.path.clone() else {
            return;
        };
        state.save_in_flight = true;
        Some((path, state.settings.clone()))
    };
    if let Some((path, snapshot)) = next {
        save_settings(cx, path, snapshot);
    }
}

/// Writes the settings right now when a save is in flight or a change is
/// still dirty: on quit there is no executor left to finish the background
/// save, and the last change would otherwise be lost.
fn flush_settings(cx: &mut App) {
    if cx.try_global::<AppSettings>().is_none() {
        return;
    }
    let pending = {
        let state = cx.global_mut::<AppSettings>();
        if !(state.save_in_flight || state.dirty) {
            return;
        }
        state.save_in_flight = false;
        state.dirty = false;
        let Some(path) = state.path.clone() else {
            return;
        };
        (path, state.settings.clone())
    };
    if let Err(e) = SettingsStore::new(pending.0.clone()).save(&pending.1) {
        log::warn!("failed to save settings to {}: {e}", pending.0.display());
    }
}

const USAGE: &str = "Usage: gitwarp [--git <auto|system|bundled|PATH>] [REPO]

Options:
  --git <VALUE>  Which git to run (default: auto; env: GITWARP_GIT, the flag wins)
  -h, --help     Print this help

Without REPO the welcome screen opens.";

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let (path, git_choice) =
        match parse_args(std::env::args_os().skip(1), std::env::var_os("GITWARP_GIT")) {
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
    let git: Result<Arc<Git>, Arc<git_core::Error>> = Git::resolve(git_choice.clone())
        .map(|r| {
            if let Some(reason) = r.fallback {
                log::warn!("bundled git unusable, using system git: {reason}");
            }
            Arc::new(r.git)
        })
        .map_err(Arc::new);

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
            let shortcuts = vec![
                ui::actions::key_binding(cx, "secondary-b", repo_view::ToggleSidebar, None),
                ui::actions::key_binding(cx, "secondary-j", repo_view::ToggleDetails, None),
                ui::actions::key_binding(cx, "secondary-k", repo_view::FocusSearch, None),
                // Ctrl O opens the "Open from folder" dialog everywhere: on
                // the welcome screen and in a repository window alike.
                ui::actions::key_binding(cx, "secondary-o", workspace::OpenRepositoryDialog, None),
                ui::actions::key_binding(
                    cx,
                    "secondary-shift-b",
                    repo_view::OpenBranchSwitcher,
                    None,
                ),
                // Welcome screen: Ctrl N opens the Initialize dialog and Del
                // drops a recent entry.
                ui::actions::key_binding(
                    cx,
                    "secondary-n",
                    welcome::NewRepository,
                    Some("welcome"),
                ),
                KeyBinding::new("delete", welcome::RemoveSelected, Some("welcome")),
            ];
            cx.bind_keys(shortcuts);
            // The search field's own keys (Shift+Enter, F3, Esc, ↓) live in
            // its key context.
            repo_view::bind_search_keys(cx);
            // The history list's extra keys (j/k, PgUp/PgDn, Ctrl+C) live in
            // the list's own context.
            history_view::bind_history_keys(cx);
            changes_view::bind_keys(cx);
            cx.set_global(AppSettings {
                path: config_path,
                settings: loaded,
                save_in_flight: false,
                dirty: false,
            });
            // A background save may still be in flight when the user quits;
            // closing the last window does reach `on_app_quit` on Linux, so
            // this is the one place that has to write the last change.
            cx.on_app_quit(|cx| {
                flush_settings(cx);
                async {}
            })
            .detach();
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
                    cx.new(|cx| Workspace::new(path, git, git_choice, window, cx))
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

    /// The last change is still dirty (or a save is still running) when the
    /// app quits: quitting has to write it instead of dropping it.
    #[gpui::test]
    fn quitting_writes_the_change_a_background_save_did_not_finish(cx: &mut gpui::TestAppContext) {
        let dir = std::env::temp_dir().join(format!("gitwarp-quit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        cx.update(|cx| {
            cx.set_global(AppSettings {
                path: Some(path.clone()),
                settings: Settings {
                    theme: ThemePref::Dark,
                    ..Default::default()
                },
                save_in_flight: true,
                dirty: true,
            });
            flush_settings(cx);
        });

        assert_eq!(SettingsStore::new(&path).load().theme, ThemePref::Dark);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
