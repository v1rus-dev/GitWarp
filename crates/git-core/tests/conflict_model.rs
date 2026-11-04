//! The conflict model against the real `git`: region building and rendering
//! are compared byte for byte with `git merge-file`, and the loader with
//! conflicted indexes and merges made in throw-away repositories.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use git_core::{
    ConflictKind, ConflictResolution, ConflictSide, Git, GitChoice, Operation, Repo, TextConflict,
};

fn git_raw(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Vec<u8> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git is installed");
    let mut pipe = child.stdin.take().unwrap();
    if let Some(bytes) = stdin {
        pipe.write_all(bytes).unwrap();
    }
    drop(pipe);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
    out.stdout
}

fn git(dir: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git_raw(dir, args, None))
        .trim()
        .to_owned()
}

fn open(path: &Path) -> Repo {
    Repo::open(Arc::new(Git::resolve(GitChoice::System).unwrap().git), path).unwrap()
}

fn new_repo() -> (tempfile::TempDir, Repo) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    fs::write(dir.join("seed.txt"), "seed\n").unwrap();
    git(dir, &["add", "seed.txt"]);
    git(dir, &["commit", "-q", "-m", "seed"]);
    let repo = open(dir);
    (tmp, repo)
}

/// Puts `path` into the index as a conflict: `(stage, mode, content)` entries.
fn index_conflict(dir: &Path, path: &str, entries: &[(u8, &str, &[u8])]) {
    let mut info = String::new();
    for (stage, mode, content) in entries {
        let oid = String::from_utf8(git_raw(
            dir,
            &["hash-object", "-w", "--stdin"],
            Some(content),
        ))
        .unwrap()
        .trim()
        .to_owned();
        info.push_str(&format!("{mode} {oid} {stage}\t{path}\n"));
    }
    git_raw(
        dir,
        &["update-index", "--index-info"],
        Some(info.as_bytes()),
    );
}

fn text(kind: ConflictKind) -> TextConflict {
    match kind {
        ConflictKind::Text(text) => text,
        other => panic!("expected a text conflict, got {other:?}"),
    }
}

/// `git merge-file -p [flag] ours base theirs`: (exit code, output).
fn merge_file(
    scratch: &Path,
    flag: Option<&str>,
    base: &[u8],
    ours: &[u8],
    theirs: &[u8],
) -> (i32, Vec<u8>) {
    fs::write(scratch.join("o"), ours).unwrap();
    fs::write(scratch.join("b"), base).unwrap();
    fs::write(scratch.join("t"), theirs).unwrap();
    let mut command = Command::new("git");
    command
        .current_dir(scratch)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["merge-file", "-p"]);
    command.args(flag);
    command.args(["o", "b", "t"]);
    let out = command.output().unwrap();
    (out.status.code().unwrap(), out.stdout)
}

/// Loads the conflict for the three versions and compares it with git.
/// Returns the conflict count.
fn check_against_git(
    dir: &Path,
    repo: &Repo,
    name: &str,
    base: Option<&str>,
    ours: &str,
    theirs: &str,
) -> usize {
    let mut entries = vec![
        (2, "100644", ours.as_bytes()),
        (3, "100644", theirs.as_bytes()),
    ];
    if let Some(base) = base {
        entries.insert(0, (1, "100644", base.as_bytes()));
    }
    index_conflict(dir, name, &entries);
    let model = text(repo.conflict(name).unwrap());
    let scratch = tempfile::tempdir().unwrap();
    let base_bytes = base.unwrap_or("").as_bytes();
    let (code, _) = merge_file(
        scratch.path(),
        None,
        base_bytes,
        ours.as_bytes(),
        theirs.as_bytes(),
    );
    assert_eq!(
        model.conflict_count() as i32,
        code,
        "{name}: conflict count\nbase={base:?}\nours={ours:?}\ntheirs={theirs:?}\nsegments={:#?}",
        model.segments
    );
    let all = |sides: &[ConflictSide]| {
        vec![ConflictResolution::Picked(sides.to_vec()); model.conflict_count()]
    };
    for (flag, sides) in [
        ("--ours", vec![ConflictSide::Ours]),
        ("--theirs", vec![ConflictSide::Theirs]),
        ("--union", vec![ConflictSide::Ours, ConflictSide::Theirs]),
    ] {
        let (_, expected) = merge_file(
            scratch.path(),
            Some(flag),
            base_bytes,
            ours.as_bytes(),
            theirs.as_bytes(),
        );
        let rendered = model.render(&all(&sides)).unwrap();
        assert_eq!(
            rendered,
            String::from_utf8(expected).unwrap(),
            "{name} {flag}\nbase={base:?}\nours={ours:?}\ntheirs={theirs:?}\nsegments={:#?}",
            model.segments
        );
    }
    model.conflict_count()
}

#[test]
fn oracle_fixtures_match_git_merge_file() {
    let (tmp, repo) = new_repo();
    let dir = tmp.path();
    let fixtures: &[(&str, Option<&str>, &str, &str)] = &[
        ("single", Some("a\nb\nc\n"), "a\nX\nc\n", "a\nY\nc\n"),
        (
            "adjacent",
            Some("1\n2\n3\n4\n5\n6\n7\n"),
            "1\n2\n3\n4\nX\n6\n7\n",
            "1\n2\n3\n4\n5\nY\n7\n",
        ),
        (
            "multiple",
            Some("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n"),
            "a\nX\nc\nd\ne\nf\ng\nh\ni\nj\nK\nl\n",
            "a\nY\nc\nd\ne\nf\ng\nh\ni\nj\nZ\nl\n",
        ),
        (
            "one-sided-around-conflict",
            Some("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n"),
            "A\nb\nc\nd\ne\nf\nX\nh\ni\nj\nk\nl\n",
            "a\nb\nc\nd\ne\nf\nY\nh\ni\nj\nk\nL\n",
        ),
        (
            "identical-plus-conflict",
            Some("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n"),
            "a\nSAME\nc\nd\ne\nf\nX\nh\ni\nj\nk\nl\n",
            "a\nSAME\nc\nd\ne\nf\nY\nh\ni\nj\nk\nl\n",
        ),
        (
            "insert-at-same-point",
            Some("a\nb\n"),
            "a\nX\nb\n",
            "a\nY\nb\n",
        ),
        ("deletion-vs-edit", Some("a\nb\nc\n"), "a\nc\n", "a\nX\nc\n"),
        (
            "crlf",
            Some("a\r\nb\r\nc\r\n"),
            "a\r\nX\r\nc\r\n",
            "a\r\nY\r\nc\r\n",
        ),
        ("no-final-newline", Some("a\nb\nc"), "a\nb\nX", "a\nb\nY"),
        (
            "final-newline-on-one-side",
            Some("a\nb\nc\n"),
            "a\nX",
            "a\nY\nc\n",
        ),
        ("both-added", None, "a\nb\nc\n", "a\nX\nc\n"),
        ("both-added-different", None, "a\nb\n", "c\nd\n"),
        (
            "refined-with-shared-middle",
            Some("0\nb\nc\nd\ne\nf\ng\nh\n9\n"),
            "0\nA\nx\ny\nz\nw\nB\nh\n9\n",
            "0\nC\nx\ny\nz\nw\nD\nh\n9\n",
        ),
        (
            "mixed-endings",
            Some("a\nb\r\nc\n"),
            "a\nX\r\nc\n",
            "a\nY\nc\n",
        ),
        (
            "close-conflicts-join",
            Some("a\nb\nc\nd\ne\n"),
            "a\nX\nc\nZ\ne\n",
            "a\nY\nc\nW\ne\n",
        ),
    ];
    for (name, base, ours, theirs) in fixtures {
        let count = check_against_git(dir, &repo, &format!("{name}.txt"), *base, ours, theirs);
        assert!(count > 0, "{name} must conflict");
    }
}

/// Deterministic pseudo-random triples, compared with git as well.
#[test]
fn oracle_random_triples_match_git_merge_file() {
    let (tmp, repo) = new_repo();
    let dir = tmp.path();
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move |bound: u64| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) % bound
    };
    let words = ["a", "b", "c", "d", "-", "{", "x1", "y2"];
    let mut checked = 0;
    for case in 0..150 {
        let len = next(12) as usize;
        let base: Vec<String> = (0..len)
            .map(|_| words[next(5) as usize].to_string())
            .collect();
        let mutate = |lines: &Vec<String>, next: &mut dyn FnMut(u64) -> u64| {
            let mut lines = lines.clone();
            for _ in 0..1 + next(3) {
                let at = next(lines.len() as u64 + 1) as usize;
                match next(3) {
                    0 if at < lines.len() => {
                        lines.remove(at);
                    }
                    1 if at < lines.len() => lines[at] = words[next(8) as usize].to_string(),
                    _ => lines.insert(at, words[next(8) as usize].to_string()),
                }
            }
            lines
        };
        let ours = mutate(&base, &mut next);
        let theirs = mutate(&base, &mut next);
        let join = |lines: &Vec<String>| {
            let mut text = lines.join("\n");
            if !lines.is_empty() {
                text.push('\n');
            }
            text
        };
        let (b, o, t) = (join(&base), join(&ours), join(&theirs));
        let scratch = tempfile::tempdir().unwrap();
        let (code, _) = merge_file(
            scratch.path(),
            None,
            b.as_bytes(),
            o.as_bytes(),
            t.as_bytes(),
        );
        if code < 1 {
            continue;
        }
        check_against_git(dir, &repo, &format!("r{case}.txt"), Some(&b), &o, &t);
        checked += 1;
    }
    assert!(
        checked > 20,
        "only {checked} conflicting cases were generated"
    );
}

#[test]
fn kinds_binary_deleted_and_symlink() {
    let (tmp, repo) = new_repo();
    let dir = tmp.path();

    index_conflict(
        dir,
        "bin.dat",
        &[
            (1, "100644", b"a\0b"),
            (2, "100644", b"a\0c"),
            (3, "100644", b"a\0d"),
        ],
    );
    assert_eq!(repo.conflict("bin.dat").unwrap(), ConflictKind::Binary);

    index_conflict(
        dir,
        "gone-ours.txt",
        &[(1, "100644", b"a\n"), (3, "100644", b"b\n")],
    );
    assert_eq!(
        repo.conflict("gone-ours.txt").unwrap(),
        ConflictKind::DeletedByUs
    );

    index_conflict(
        dir,
        "gone-theirs.txt",
        &[(1, "100644", b"a\n"), (2, "100644", b"b\n")],
    );
    assert_eq!(
        repo.conflict("gone-theirs.txt").unwrap(),
        ConflictKind::DeletedByThem
    );

    index_conflict(
        dir,
        "link",
        &[
            (1, "120000", b"target"),
            (2, "120000", b"one"),
            (3, "120000", b"two"),
        ],
    );
    assert_eq!(repo.conflict("link").unwrap(), ConflictKind::Symlink);

    // A gitlink entry needs no object in the store.
    let info = "160000 1111111111111111111111111111111111111111 2\tsub\n\
                160000 2222222222222222222222222222222222222222 3\tsub\n";
    git_raw(
        dir,
        &["update-index", "--index-info"],
        Some(info.as_bytes()),
    );
    assert_eq!(repo.conflict("sub").unwrap(), ConflictKind::Submodule);

    assert!(repo.conflict("seed.txt").is_err(), "not conflicted");
}

/// Both sides change the same file; `main` and `side` end up in a conflicted merge.
fn conflicted_merge(files: &[(&str, &str, &str, &str)]) -> (tempfile::TempDir, Repo) {
    let (tmp, repo) = new_repo();
    let dir = tmp.path();
    for (name, base, _, _) in files {
        fs::write(dir.join(name), base).unwrap();
        git(dir, &["add", "--", name]);
    }
    git(dir, &["commit", "-q", "-m", "base"]);
    git(dir, &["branch", "side"]);
    for (name, _, ours, _) in files {
        fs::write(dir.join(name), ours).unwrap();
        git(dir, &["add", "--", name]);
    }
    git(dir, &["commit", "-q", "-m", "ours"]);
    git(dir, &["switch", "-q", "side"]);
    for (name, _, _, theirs) in files {
        fs::write(dir.join(name), theirs).unwrap();
        git(dir, &["add", "--", name]);
    }
    git(dir, &["commit", "-q", "-m", "theirs"]);
    git(dir, &["switch", "-q", "main"]);
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["merge", "side", "-q", "--no-edit"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(!out.status.success(), "the merge must conflict");
    (tmp, repo)
}

#[test]
fn pathspec_looking_names_load_the_right_file() {
    let (_tmp, repo) = conflicted_merge(&[
        (
            "a[1].txt",
            "x\ny\nz\n",
            "x\nBRACKET-ours\nz\n",
            "x\nBRACKET-theirs\nz\n",
        ),
        (
            "a1.txt",
            "x\ny\nz\n",
            "x\nPLAIN-ours\nz\n",
            "x\nPLAIN-theirs\nz\n",
        ),
    ]);
    let bracket = text(repo.conflict("a[1].txt").unwrap());
    assert_eq!(bracket.ours, vec!["x", "BRACKET-ours", "z"]);
    let plain = text(repo.conflict("a1.txt").unwrap());
    assert_eq!(plain.theirs, vec!["x", "PLAIN-theirs", "z"]);
}

#[test]
fn save_resolution_stages_the_file_and_the_merge_can_finish() {
    let (tmp, repo) = conflicted_merge(&[("f.txt", "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n")]);
    let model = text(repo.conflict("f.txt").unwrap());
    let resolved = model
        .render(&[ConflictResolution::Picked(vec![
            ConflictSide::Theirs,
            ConflictSide::Ours,
        ])])
        .unwrap();
    assert_eq!(resolved, "a\nY\nX\nc\n");
    assert_eq!(repo.operation().unwrap(), Operation::Merge);
    repo.save_resolution("f.txt", &resolved).unwrap();
    assert_eq!(
        fs::read_to_string(tmp.path().join("f.txt")).unwrap(),
        resolved
    );
    assert!(repo.working_status().unwrap().conflicted.is_empty());
    assert_eq!(
        git(tmp.path(), &["diff", "--cached", "--name-only"]),
        "f.txt"
    );
    git(tmp.path(), &["commit", "-q", "--no-edit"]);
    assert_eq!(repo.operation().unwrap(), Operation::None);
    assert!(repo.save_resolution("../escape.txt", "x").is_err());
}
