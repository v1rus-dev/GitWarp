//! Shared harness of the app's gpui tests: a real throw-away repository and a
//! window whose root view is the workspace.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    point, px, size, AppContext, Bounds, Entity, TestAppContext, VisualTestContext,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions,
};

use git_core::{Git, GitChoice, Repo};

use crate::workspace::Workspace;

/// The system git, for tests that need a real repository.
pub(crate) fn git() -> Arc<Git> {
    Arc::new(
        Git::resolve(GitChoice::System)
            .expect("git is installed")
            .git,
    )
}

/// A real repository in a unique temp folder, removed on drop.
pub(crate) struct TempRepo(PathBuf);

impl TempRepo {
    pub(crate) fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("gitwarp-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Repo::init(git(), &path).expect("init the temp repository");
        // The app's own git calls read this repository's config, so the identity and the
        // `main` branch must not depend on the machine's global config (CI runners have none).
        for args in [
            &["config", "user.name", "Test"][..],
            &["config", "user.email", "test@example.com"],
            &["config", "commit.gpgsign", "false"],
            &["symbolic-ref", "HEAD", "refs/heads/main"],
        ] {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Opens the real window with the workspace (on its welcome screen) as the
/// root view.
pub(crate) fn open_workspace(
    git: Arc<Git>,
    cx: &mut TestAppContext,
) -> (VisualTestContext, Entity<Workspace>) {
    cx.update(ui::init).expect("ui::init");
    let handle = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(960.), px(620.)),
                ))),
                window_decorations: Some(WindowDecorations::Client),
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| Workspace::new(None, Ok(git.clone()), GitChoice::Auto, window, cx))
            },
        )
        .expect("open window")
    });

    let mut cx = VisualTestContext::from_window(handle.into(), cx);
    cx.run_until_parked();
    cx.update(|window, _| window.activate_window());
    let workspace = cx
        .update(|window, _| window.root::<Workspace>().flatten())
        .expect("the window's root view is the workspace");
    (cx, workspace)
}
