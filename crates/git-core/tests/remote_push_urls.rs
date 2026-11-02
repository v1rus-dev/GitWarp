//! `Repo::remote_push_urls`: one entry per `pushurl`, the fetch URL otherwise.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use git_core::{Git, GitChoice, Repo};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git is installed");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn open(path: &Path) -> Repo {
    Repo::open(Arc::new(Git::resolve(GitChoice::System).unwrap().git), path).unwrap()
}

#[test]
fn push_urls_list_every_pushurl_or_fall_back_to_the_fetch_url() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q"]);
    git(
        dir,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/repo.git",
        ],
    );
    git(
        dir,
        &[
            "remote",
            "add",
            "mirror",
            "https://github.com/acme/mirror.git",
        ],
    );
    git(
        dir,
        &[
            "remote",
            "set-url",
            "--add",
            "--push",
            "mirror",
            "https://github.com/acme/mirror.git",
        ],
    );
    git(
        dir,
        &[
            "remote",
            "set-url",
            "--add",
            "--push",
            "mirror",
            "git@gitlab.com:acme/mirror.git",
        ],
    );
    let repo = open(dir);

    // No pushurl: git answers with the fetch URL.
    assert_eq!(
        repo.remote_push_urls("origin").unwrap(),
        vec!["https://github.com/acme/repo.git".to_owned()]
    );
    // Two pushurls: both, in config order.
    assert_eq!(
        repo.remote_push_urls("mirror").unwrap(),
        vec![
            "https://github.com/acme/mirror.git".to_owned(),
            "git@gitlab.com:acme/mirror.git".to_owned()
        ]
    );
    // An unknown remote has no URLs.
    assert!(repo.remote_push_urls("nope").unwrap().is_empty());
}
