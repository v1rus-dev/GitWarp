mod graph_paint;
mod history_view;
mod theme;

use gpui::{
    prelude::*, px, size, App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};

use git_core::GitChoice;
use history_view::HistoryView;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let (path, git) = parse_args(std::env::args().skip(1), std::env::var("GITLANE_GIT").ok());

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("gitlane".into()),
                    ..Default::default()
                }),
                app_id: Some("dev.v1rus.gitlane".into()),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| HistoryView::open(path.clone(), git.clone(), cx)),
        )
        .expect("failed to open the main window");
        cx.activate(true);
    });
}

/// `[--git <auto|system|bundled|PATH>] [repo]`. The flag wins over
/// `GITLANE_GIT`; the repo defaults to the current directory.
fn parse_args(args: impl Iterator<Item = String>, env_git: Option<String>) -> (String, GitChoice) {
    let mut path = None;
    let mut git = env_git.map(|v| GitChoice::parse(&v));
    let mut args = args;
    while let Some(arg) = args.next() {
        if arg == "--git" {
            git = args.next().map(|v| GitChoice::parse(&v)).or(git);
        } else if let Some(v) = arg.strip_prefix("--git=") {
            git = Some(GitChoice::parse(v));
        } else {
            path = Some(arg);
        }
    }
    (path.unwrap_or_else(|| ".".into()), git.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str], env: Option<&str>) -> (String, GitChoice) {
        parse_args(args.iter().map(|s| s.to_string()), env.map(str::to_owned))
    }

    #[test]
    fn flag_wins_over_env() {
        assert_eq!(parse(&[], None), (".".into(), GitChoice::Auto));
        assert_eq!(
            parse(&["repo"], Some("system")),
            ("repo".into(), GitChoice::System)
        );
        assert_eq!(
            parse(&["--git", "bundled", "repo"], Some("system")),
            ("repo".into(), GitChoice::Bundled)
        );
        assert_eq!(
            parse(&["repo", "--git=/opt/git"], None).1,
            GitChoice::Custom("/opt/git".into())
        );
    }
}
