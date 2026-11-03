//! `update_submodule` against a local submodule, so no network is needed.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::network::{self, CancelToken, NetworkContext};
use git_core::{Git, GitChoice, Repo, SubmoduleState};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
            // git refuses file:// submodule clones by default.
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .expect("git is installed");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

#[test]
fn update_submodule_initialises_it() {
    let base = tempfile::tempdir().unwrap();
    let lib = base.path().join("lib");
    fs::create_dir(&lib).unwrap();
    git(&lib, &["init", "-q"]);
    fs::write(lib.join("l.txt"), "lib\n").unwrap();
    git(&lib, &["add", "l.txt"]);
    git(&lib, &["commit", "-q", "-m", "lib"]);

    let main = base.path().join("main");
    fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q"]);
    // A file:// URL uses forward slashes; git stores it that way, so build it
    // with forward slashes too (on Windows `display()` would yield backslashes).
    let url = format!("file://{}", lib.display().to_string().replace('\\', "/"));
    git(&main, &["submodule", "add", "-q", &url, "vendor/lib"]);
    git(&main, &["commit", "-q", "-m", "add submodule"]);
    // Drop the checkout, keeping the recorded submodule: the state a fresh
    // clone is in.
    git(&main, &["submodule", "deinit", "-q", "--all", "--force"]);

    // The app runs git without the test's `-c`, so allow file:// in the repo config.
    git(&main, &["config", "protocol.file.allow", "always"]);

    let repo = Repo::open(
        Arc::new(Git::resolve(GitChoice::System).unwrap().git),
        &main,
    )
    .unwrap();
    assert_eq!(
        repo.submodules().unwrap()[0].state,
        SubmoduleState::Uninitialized
    );
    assert_eq!(
        network::submodule_url(&repo, "vendor/lib").unwrap(),
        Some(url)
    );

    network::update_submodule(
        &repo,
        "vendor/lib",
        &NetworkContext::default(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();

    assert!(main.join("vendor/lib/l.txt").exists());
    assert_eq!(repo.submodules().unwrap()[0].state, SubmoduleState::Clean);
}
