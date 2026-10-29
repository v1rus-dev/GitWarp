mod graph_paint;
mod history_view;
mod theme;

use gpui::{
    prelude::*, px, size, App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use git_core::{Git, GitChoice};
use history_view::HistoryView;

const USAGE: &str = "Usage: gitwarp [--git <auto|system|bundled|PATH>] [REPO]

Options:
  --git <VALUE>  Which git to run (default: auto; env: GITWARP_GIT, the flag wins)
  -h, --help     Print this help";

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

    Application::new().run(move |cx: &mut App| {
        ui::theme::init(cx);
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("GitWarp".into()),
                    ..Default::default()
                }),
                app_id: Some("dev.v1rus.gitwarp".into()),
                ..Default::default()
            },
            |window, cx| {
                ui::theme::follow_system(window, cx);
                cx.new(|cx| HistoryView::open(path.clone(), git.clone(), cx))
            },
        )
        .expect("failed to open the main window");
        cx.activate(true);
    });
}

#[derive(Debug, PartialEq, Eq)]
enum Cli {
    Run { path: PathBuf, git: GitChoice },
    Help,
}

/// `[--git <auto|system|bundled|PATH>] [repo]`. The flag wins over
/// `GITWARP_GIT`; the repo defaults to the current directory.
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
        path: path.unwrap_or_else(|| ".".into()),
        git: git.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str], env: Option<&str>) -> Result<Cli, String> {
        parse_args(args.iter().map(OsString::from), env.map(OsString::from))
    }

    fn run(path: &str, git: GitChoice) -> Result<Cli, String> {
        Ok(Cli::Run {
            path: path.into(),
            git,
        })
    }

    #[test]
    fn flag_wins_over_env() {
        assert_eq!(parse(&[], None), run(".", GitChoice::Auto));
        assert_eq!(
            parse(&["repo"], Some("system")),
            run("repo", GitChoice::System)
        );
        assert_eq!(parse(&[], Some("")), run(".", GitChoice::Auto));
        assert_eq!(
            parse(&["--git", "bundled", "repo"], Some("system")),
            run("repo", GitChoice::Bundled)
        );
        assert_eq!(
            parse(&["repo", "--git=/opt/git"], None),
            run("repo", GitChoice::Custom("/opt/git".into()))
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
                path: raw.into(),
                git: GitChoice::Auto
            })
        );
    }
}
