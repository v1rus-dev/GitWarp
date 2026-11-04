//! The conflict model against the real `git`: region building and rendering
//! are compared byte for byte with `git merge-file`, and the loader with
//! conflicted indexes and merges made in throw-away repositories.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use git_core::{
    ConflictKind, ConflictResolution, ConflictSide, Git, GitChoice, Operation, Repo, SaveOutcome,
    TextConflict,
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
        .args(["merge-file", "-p", "--diff-algorithm=histogram"]);
    command.args(flag);
    command.args(["o", "b", "t"]);
    let out = command.output().unwrap();
    (out.status.code().unwrap(), out.stdout)
}

fn git_command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    command
}

/// One file of a real merge: the three versions (`base` `None`: both sides
/// added the file).
struct Case {
    name: String,
    base: Option<Vec<u8>>,
    ours: Vec<u8>,
    theirs: Vec<u8>,
}

/// Merges `side` into `main` with real `git merge` (`merge.conflictStyle=merge`)
/// over all `cases` at once. Files that conflict stay conflicted in the repo
/// with git's marker file in the work tree.
fn real_merge(cases: &[Case]) -> (tempfile::TempDir, Repo) {
    let (tmp, repo) = new_repo();
    let dir = tmp.path();
    git(dir, &["config", "merge.conflictStyle", "merge"]);
    let write_all = |pick: &dyn Fn(&Case) -> Option<&Vec<u8>>| {
        for case in cases {
            if let Some(content) = pick(case) {
                fs::write(dir.join(&case.name), content).unwrap();
                git(dir, &["add", "--", &case.name]);
            }
        }
    };
    write_all(&|case| case.base.as_ref());
    git(dir, &["commit", "-q", "--allow-empty", "-m", "base"]);
    git(dir, &["branch", "side"]);
    write_all(&|case| Some(&case.ours));
    git(dir, &["commit", "-q", "--allow-empty", "-m", "ours"]);
    git(dir, &["switch", "-q", "side"]);
    write_all(&|case| Some(&case.theirs));
    git(dir, &["commit", "-q", "--allow-empty", "-m", "theirs"]);
    git(dir, &["switch", "-q", "main"]);
    git_command(dir)
        .args(["merge", "side", "-q", "--no-edit"])
        .output()
        .unwrap();
    (tmp, repo)
}

fn case(name: &str, base: Option<&str>, ours: &str, theirs: &str) -> Case {
    Case {
        name: name.to_string(),
        base: base.map(|base| base.as_bytes().to_vec()),
        ours: ours.as_bytes().to_vec(),
        theirs: theirs.as_bytes().to_vec(),
    }
}

/// The three whole-file resolutions read back from a marker file: every
/// conflict block replaced by ours, by theirs, and by both.
fn resolve_markers(markers: &str) -> [String; 3] {
    let (mut ours, mut theirs, mut union) = (String::new(), String::new(), String::new());
    let mut state = 0; // 0 outside, 1 ours, 2 theirs
    let mut sides = (String::new(), String::new());
    for line in markers.split_inclusive('\n') {
        if state == 0 && line.starts_with("<<<<<<<") {
            state = 1;
            sides = (String::new(), String::new());
        } else if state == 1 && line.starts_with("=======") {
            state = 2;
        } else if state == 2 && line.starts_with(">>>>>>>") {
            state = 0;
            ours.push_str(&sides.0);
            theirs.push_str(&sides.1);
            union.push_str(&sides.0);
            union.push_str(&sides.1);
        } else if state == 1 {
            sides.0.push_str(line);
        } else if state == 2 {
            sides.1.push_str(line);
        } else {
            for out in [&mut ours, &mut theirs, &mut union] {
                out.push_str(line);
            }
        }
    }
    [ours, theirs, union]
}

/// Compares the model of every conflicted case with the work-tree file the
/// real merge wrote; returns how many cases conflicted. Disagreements are
/// collected so one run reports them all.
fn check_cases(dir: &Path, repo: &Repo, cases: &[Case]) -> (usize, Vec<String>) {
    let scratch = tempfile::tempdir().unwrap();
    let mut conflicted = 0;
    let mut failures = Vec::new();
    for case in cases {
        let unmerged = git(dir, &["ls-files", "-u", "--", &case.name]);
        if unmerged.is_empty() {
            continue;
        }
        conflicted += 1;
        let ConflictKind::Text(model) = repo.conflict(&case.name).unwrap() else {
            failures.push(format!("{}: not a text conflict", case.name));
            continue;
        };
        let markers = fs::read(dir.join(&case.name)).unwrap();
        let rendered = model.render_markers("HEAD", "side");
        let describe = || {
            format!(
                "{}\nbase={:?}\nours={:?}\ntheirs={:?}\ngit wrote={:?}\nmodel wrote={:?}\nsegments={:#?}",
                case.name,
                case.base.as_deref().map(String::from_utf8_lossy),
                String::from_utf8_lossy(&case.ours),
                String::from_utf8_lossy(&case.theirs),
                String::from_utf8_lossy(&markers),
                rendered,
                model.segments
            )
        };
        if rendered.as_bytes() != markers {
            failures.push(format!("markers differ: {}", describe()));
            continue;
        }
        let all = |sides: &[ConflictSide]| {
            vec![ConflictResolution::Picked(sides.to_vec()); model.conflict_count()]
        };
        let [ours, theirs, union] = resolve_markers(&rendered);
        // Markers always end their lines, so a side without a final newline
        // cannot be read back from them.
        let terminated = |bytes: &[u8]| bytes.is_empty() || bytes.ends_with(b"\n");
        let readable = terminated(&case.ours) && terminated(&case.theirs);
        for (name, sides, expected) in [
            ("ours", vec![ConflictSide::Ours], ours),
            ("theirs", vec![ConflictSide::Theirs], theirs),
            (
                "union",
                vec![ConflictSide::Ours, ConflictSide::Theirs],
                union,
            ),
        ] {
            if readable && model.render(&all(&sides)).as_deref() != Some(expected.as_str()) {
                failures.push(format!("--{name} differs: {}", describe()));
            }
        }
        // `git merge-file` has no "zealous" level, only "zealous alnum": it
        // can join more conflicts, but taking one whole side is the same.
        let base_bytes = case.base.clone().unwrap_or_default();
        for (flag, sides) in [
            ("--ours", vec![ConflictSide::Ours]),
            ("--theirs", vec![ConflictSide::Theirs]),
        ] {
            let (code, expected) = merge_file(
                scratch.path(),
                Some(flag),
                &base_bytes,
                &case.ours,
                &case.theirs,
            );
            if code >= 0
                && model.render(&all(&sides)).as_deref()
                    != Some(String::from_utf8_lossy(&expected).as_ref())
            {
                failures.push(format!("merge-file {flag} differs: {}", describe()));
            }
        }
    }
    (conflicted, failures)
}

#[test]
fn oracle_fixtures_match_git_merge() {
    let fixtures: Vec<Case> = vec![
        case("single", Some("a\nb\nc\n"), "a\nX\nc\n", "a\nY\nc\n"),
        case(
            "adjacent",
            Some("1\n2\n3\n4\n5\n6\n7\n"),
            "1\n2\n3\n4\nX\n6\n7\n",
            "1\n2\n3\n4\n5\nY\n7\n",
        ),
        case(
            "multiple",
            Some("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n"),
            "a\nX\nc\nd\ne\nf\ng\nh\ni\nj\nK\nl\n",
            "a\nY\nc\nd\ne\nf\ng\nh\ni\nj\nZ\nl\n",
        ),
        case(
            "one-sided-around-conflict",
            Some("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n"),
            "A\nb\nc\nd\ne\nf\nX\nh\ni\nj\nk\nl\n",
            "a\nb\nc\nd\ne\nf\nY\nh\ni\nj\nk\nL\n",
        ),
        case(
            "identical-plus-conflict",
            Some("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n"),
            "a\nSAME\nc\nd\ne\nf\nX\nh\ni\nj\nk\nl\n",
            "a\nSAME\nc\nd\ne\nf\nY\nh\ni\nj\nk\nl\n",
        ),
        case(
            "insert-at-same-point",
            Some("a\nb\n"),
            "a\nX\nb\n",
            "a\nY\nb\n",
        ),
        case("deletion-vs-edit", Some("a\nb\nc\n"), "a\nc\n", "a\nX\nc\n"),
        case(
            "crlf",
            Some("a\r\nb\r\nc\r\n"),
            "a\r\nX\r\nc\r\n",
            "a\r\nY\r\nc\r\n",
        ),
        case("no-final-newline", Some("a\nb\nc"), "a\nb\nX", "a\nb\nY"),
        case(
            "final-newline-on-one-side",
            Some("a\nb\nc\n"),
            "a\nX",
            "a\nY\nc\n",
        ),
        case("both-added", None, "a\nb\nc\n", "a\nX\nc\n"),
        case("both-added-different", None, "a\nb\n", "c\nd\n"),
        case(
            "refined-with-shared-middle",
            Some("0\nb\nc\nd\ne\nf\ng\nh\n9\n"),
            "0\nA\nx\ny\nz\nw\nB\nh\n9\n",
            "0\nC\nx\ny\nz\nw\nD\nh\n9\n",
        ),
        case(
            "mixed-endings",
            Some("a\nb\r\nc\n"),
            "a\nX\r\nc\n",
            "a\nY\nc\n",
        ),
        case(
            "close-conflicts-join",
            Some("a\nb\nc\nd\ne\n"),
            "a\nX\nc\nZ\ne\n",
            "a\nY\nc\nW\ne\n",
        ),
        // `git merge` does not join conflicts over lines without letters and
        // digits the way `git merge-file` does: two blocks.
        case(
            "gap-without-alnum",
            Some("0\nb\n-\n-\n-\n-\nf\n9\n"),
            "0\nA\n-\n-\n-\n-\nB\n9\n",
            "0\nC\n-\n-\n-\n-\nD\n9\n",
        ),
        // Histogram finds a conflict where Myers (and so `merge-file`'s
        // default) finds none.
        case(
            "histogram-only",
            Some(items("b a b \"\" \"\" } c \"\" x } a c").as_str()),
            items("b a b \"\" \"\" c \"\" } a c x {").as_str(),
            items("b a { { a b \"\" \"\" } c \"\" x } c").as_str(),
        ),
    ];
    let (tmp, repo) = real_merge(&fixtures);
    let (conflicted, failures) = check_cases(tmp.path(), &repo, &fixtures);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    // Every fixture is meant to conflict, except those git merges cleanly.
    assert!(
        conflicted >= fixtures.len() - 1,
        "only {conflicted} conflicted"
    );
    for name in ["histogram-only", "gap-without-alnum"] {
        let markers = fs::read_to_string(tmp.path().join(name)).unwrap();
        assert!(
            markers.contains("<<<<<<<"),
            "{name} must conflict in git merge"
        );
    }
    // The gap case: git merge writes two blocks.
    let gap = fs::read_to_string(tmp.path().join("gap-without-alnum")).unwrap();
    assert_eq!(gap.matches("<<<<<<<").count(), 2);
}

/// One item per line; `""` is an empty line.
fn items(text: &str) -> String {
    text.split(' ')
        .map(|item| if item == "\"\"" { "" } else { item })
        .map(|item| format!("{item}\n"))
        .collect()
}

/// Deterministic pseudo-random triples, merged by real git as well.
#[test]
fn oracle_random_triples_match_git_merge() {
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move |bound: u64| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) % bound
    };
    let words = ["a", "b", "c", "d", "-", "{", "x1", "y2"];
    let mut cases = Vec::new();
    for index in 0..400 {
        let len = next(30) as usize;
        let base: Vec<String> = (0..len)
            .map(|_| words[next(4) as usize].to_string())
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
            // Mixed line endings and missing final newlines, by length.
            let crlf = lines.len().is_multiple_of(3);
            let nl = if crlf { "\r\n" } else { "\n" };
            let mut text = lines.join(nl);
            if !lines.is_empty() && lines.len() % 4 != 1 {
                text.push_str(nl);
            }
            text
        };
        // A file name unlike any other, so rename detection has nothing to pair.
        cases.push(case(
            &format!("r{index}.txt"),
            Some(join(&base).as_str()),
            join(&ours).as_str(),
            join(&theirs).as_str(),
        ));
    }
    let (tmp, repo) = real_merge(&cases);
    let (conflicted, failures) = check_cases(tmp.path(), &repo, &cases);
    assert!(conflicted > 40, "only {conflicted} conflicting cases");
    assert!(
        failures.is_empty(),
        "{} of {conflicted} disagree with git merge:\n{}",
        failures.len(),
        failures.join("\n\n")
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

    // Added by one side only, or deleted by both.
    index_conflict(dir, "mine.txt", &[(2, "100644", b"a\n")]);
    assert_eq!(repo.conflict("mine.txt").unwrap(), ConflictKind::AddedByUs);
    index_conflict(dir, "theirs.txt", &[(3, "100644", b"a\n")]);
    assert_eq!(
        repo.conflict("theirs.txt").unwrap(),
        ConflictKind::AddedByThem
    );
    index_conflict(dir, "both-gone.txt", &[(1, "100644", b"a\n")]);
    assert_eq!(
        repo.conflict("both-gone.txt").unwrap(),
        ConflictKind::DeletedByBoth
    );
}

#[test]
fn text_that_git_cannot_hand_back_as_utf8_is_not_edited_line_by_line() {
    let (tmp, repo) = new_repo();
    let dir = tmp.path();
    // 0xE9 is "é" in Latin-1 and invalid UTF-8.
    index_conflict(
        dir,
        "latin1.txt",
        &[
            (1, "100644", b"caf\xe9\n"),
            (2, "100644", b"caf\xe9 ours\n"),
            (3, "100644", b"caf\xe9 theirs\n"),
        ],
    );
    assert_eq!(repo.conflict("latin1.txt").unwrap(), ConflictKind::NotText);

    // Only the attribute matters: the index of a filtered path holds the
    // clean form (an LFS pointer), which the work-tree file does not show.
    fs::write(dir.join(".gitattributes"), "*.lfs filter=lfs\n").unwrap();
    index_conflict(
        dir,
        "big.lfs",
        &[
            (1, "100644", b"version a\n"),
            (2, "100644", b"version b\n"),
            (3, "100644", b"version c\n"),
        ],
    );
    assert_eq!(repo.conflict("big.lfs").unwrap(), ConflictKind::NotText);
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
    assert_eq!(
        repo.save_resolution("f.txt", &resolved, &model.guard)
            .unwrap(),
        SaveOutcome::Saved
    );
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
    assert!(repo
        .save_resolution("../escape.txt", "x", &model.guard)
        .is_err());
}

#[test]
fn save_goes_through_git_so_eol_attributes_apply() {
    let attrs = ".gitattributes";
    let (tmp, repo) = conflicted_merge(&[
        (
            attrs,
            "* text eol=crlf\n",
            "* text eol=crlf\n",
            "* text eol=crlf\n",
        ),
        ("f.txt", "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n"),
    ]);
    let dir = tmp.path();
    // git wrote the marker file with CRLF; it is what the merge left.
    assert!(fs::read(dir.join("f.txt")).unwrap().contains(&b'\r'));
    let model = text(repo.conflict("f.txt").unwrap());
    assert!(!model.guard.changed_after_merge);
    let resolved = model
        .render(&[ConflictResolution::Picked(vec![
            ConflictSide::Theirs,
            ConflictSide::Ours,
        ])])
        .unwrap();
    assert_eq!(resolved, "a\nY\nX\nc\n", "the index form is LF");
    assert_eq!(
        repo.save_resolution("f.txt", &resolved, &model.guard)
            .unwrap(),
        SaveOutcome::Saved
    );
    assert_eq!(
        fs::read(dir.join("f.txt")).unwrap(),
        b"a\r\nY\r\nX\r\nc\r\n"
    );
    // Staged, and the work-tree file is not modified against the index.
    assert_eq!(git(dir, &["diff", "--name-only"]), "");
    assert_eq!(git(dir, &["diff", "--cached", "--name-only"]), "f.txt");
    assert!(git(dir, &["ls-files", "-u"]).is_empty());
}

#[test]
fn save_keeps_the_mode_of_the_side_and_creates_a_missing_file() {
    use std::os::unix::fs::PermissionsExt;
    let (tmp, repo) = new_repo();
    let dir = tmp.path();
    index_conflict(
        dir,
        "bin/run.sh",
        &[
            (1, "100755", b"a\nb\nc\n"),
            (2, "100755", b"a\nX\nc\n"),
            (3, "100755", b"a\nY\nc\n"),
        ],
    );
    let model = text(repo.conflict("bin/run.sh").unwrap());
    assert_eq!(
        model.guard.work_tree_digest, None,
        "no file in the work tree"
    );
    let resolved = model
        .render(&[ConflictResolution::Picked(vec![ConflictSide::Ours])])
        .unwrap();
    assert_eq!(
        repo.save_resolution("bin/run.sh", &resolved, &model.guard)
            .unwrap(),
        SaveOutcome::Saved
    );
    let meta = fs::metadata(dir.join("bin/run.sh")).unwrap();
    assert_eq!(meta.permissions().mode() & 0o111, 0o111);
    assert!(git(dir, &["ls-files", "-s", "bin/run.sh"]).starts_with("100755"));
}

#[test]
fn save_refuses_paths_outside_the_work_tree_and_inside_git() {
    let (_tmp, repo) = conflicted_merge(&[("f.txt", "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n")]);
    let model = text(repo.conflict("f.txt").unwrap());
    for path in [
        "../escape.txt",
        ".git/hooks/post-checkout",
        ".GIT/config",
        "/abs",
        "a/.Git/x",
    ] {
        assert!(
            repo.save_resolution(path, "x", &model.guard).is_err(),
            "{path}"
        );
    }
}

#[test]
fn a_file_changed_outside_the_editor_is_noticed_at_load_and_at_save() {
    let (tmp, repo) = conflicted_merge(&[("f.txt", "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n")]);
    let dir = tmp.path();
    let model = text(repo.conflict("f.txt").unwrap());
    assert!(!model.guard.changed_after_merge, "git's own marker file");
    let resolved = model
        .render(&[ConflictResolution::Picked(vec![ConflictSide::Ours])])
        .unwrap();

    // Changed since the load: nothing is written, the conflict stays.
    fs::write(dir.join("f.txt"), "edited elsewhere\n").unwrap();
    assert_eq!(
        repo.save_resolution("f.txt", &resolved, &model.guard)
            .unwrap(),
        SaveOutcome::WorkTreeChanged
    );
    assert_eq!(
        fs::read_to_string(dir.join("f.txt")).unwrap(),
        "edited elsewhere\n"
    );
    assert!(!git(dir, &["ls-files", "-u"]).is_empty());

    // Loading it now reports that it is not the file the merge wrote.
    let reloaded = text(repo.conflict("f.txt").unwrap());
    assert!(reloaded.guard.changed_after_merge);
    // ... and saving from that load is allowed (the user confirmed).
    assert_eq!(
        repo.save_resolution("f.txt", &resolved, &reloaded.guard)
            .unwrap(),
        SaveOutcome::Saved
    );
    assert_eq!(fs::read_to_string(dir.join("f.txt")).unwrap(), resolved);
}

#[test]
fn saving_after_the_merge_was_aborted_writes_nothing() {
    let (tmp, repo) = conflicted_merge(&[("f.txt", "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n")]);
    let dir = tmp.path();
    let model = text(repo.conflict("f.txt").unwrap());
    let resolved = model
        .render(&[ConflictResolution::Picked(vec![ConflictSide::Theirs])])
        .unwrap();
    git(dir, &["merge", "--abort"]);
    let after_abort = fs::read(dir.join("f.txt")).unwrap();
    assert_eq!(
        repo.save_resolution("f.txt", &resolved, &model.guard)
            .unwrap(),
        SaveOutcome::StagesChanged
    );
    assert_eq!(fs::read(dir.join("f.txt")).unwrap(), after_abort);
    assert_eq!(git(dir, &["status", "--porcelain"]), "");
}

#[test]
fn saving_after_the_sides_changed_writes_nothing() {
    let (tmp, repo) = conflicted_merge(&[("f.txt", "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n")]);
    let dir = tmp.path();
    let model = text(repo.conflict("f.txt").unwrap());
    // Someone else resolved the path and put the conflict back with other sides.
    git(dir, &["checkout", "--conflict=merge", "--", "f.txt"]);
    index_conflict(
        dir,
        "f.txt",
        &[
            (1, "100644", b"a\nb\nc\n"),
            (2, "100644", b"a\nOTHER\nc\n"),
            (3, "100644", b"a\nY\nc\n"),
        ],
    );
    let before = fs::read(dir.join("f.txt")).unwrap();
    assert_eq!(
        repo.save_resolution("f.txt", "a\nZ\nc\n", &model.guard)
            .unwrap(),
        SaveOutcome::StagesChanged
    );
    assert_eq!(fs::read(dir.join("f.txt")).unwrap(), before);
}

/// What the editor's cards do for the one-sided kinds: "Keep the file" takes
/// the side that has it, "Delete the file" takes the side that does not (git
/// then removes the path).
#[test]
fn one_sided_kinds_keep_or_delete_through_resolve_take() {
    let (tmp, repo) = new_repo();
    let dir = tmp.path();
    let unmerged = |path: &str| git(dir, &["ls-files", "-u", "--", path]);
    let tracked = |path: &str| git(dir, &["ls-files", "--", path]);

    index_conflict(dir, "mine.txt", &[(2, "100644", b"a\n")]);
    fs::write(dir.join("mine.txt"), "a\n").unwrap();
    repo.resolve_take("mine.txt", ConflictSide::Ours).unwrap();
    assert_eq!(unmerged("mine.txt"), "");
    assert_eq!(tracked("mine.txt"), "mine.txt");

    index_conflict(dir, "mine-del.txt", &[(2, "100644", b"a\n")]);
    fs::write(dir.join("mine-del.txt"), "a\n").unwrap();
    repo.resolve_take("mine-del.txt", ConflictSide::Theirs)
        .unwrap();
    assert_eq!(tracked("mine-del.txt"), "");
    assert!(!dir.join("mine-del.txt").exists());

    index_conflict(dir, "theirs.txt", &[(3, "100644", b"a\n")]);
    fs::write(dir.join("theirs.txt"), "a\n").unwrap();
    repo.resolve_take("theirs.txt", ConflictSide::Theirs)
        .unwrap();
    assert_eq!(unmerged("theirs.txt"), "");
    assert_eq!(tracked("theirs.txt"), "theirs.txt");

    index_conflict(dir, "both-gone.txt", &[(1, "100644", b"a\n")]);
    repo.resolve_take("both-gone.txt", ConflictSide::Ours)
        .unwrap();
    assert_eq!(unmerged("both-gone.txt"), "");
    assert_eq!(tracked("both-gone.txt"), "");
}

#[test]
fn save_takes_the_mode_the_way_git_does() {
    use std::os::unix::fs::PermissionsExt;
    let (tmp, repo) = new_repo();
    let dir = tmp.path();
    // Only theirs made it executable: that wins, though ours is stage 2.
    index_conflict(
        dir,
        "run.sh",
        &[
            (1, "100644", b"a\nb\nc\n"),
            (2, "100644", b"a\nX\nc\n"),
            (3, "100755", b"a\nb\nY\n"),
        ],
    );
    let model = text(repo.conflict("run.sh").unwrap());
    assert_eq!(
        repo.save_resolution("run.sh", "a\nX\nY\n", &model.guard)
            .unwrap(),
        SaveOutcome::Saved
    );
    assert!(git(dir, &["ls-files", "-s", "run.sh"]).starts_with("100755"));
    let meta = fs::metadata(dir.join("run.sh")).unwrap();
    assert_eq!(meta.permissions().mode() & 0o111, 0o111);
}

#[test]
fn a_failed_checkout_puts_the_conflict_back() {
    use std::os::unix::fs::PermissionsExt;
    let (tmp, repo) = new_repo();
    let dir = tmp.path();
    fs::create_dir(dir.join("sub")).unwrap();
    index_conflict(
        dir,
        "sub/f.txt",
        &[
            (1, "100644", b"a\nb\nc\n"),
            (2, "100644", b"a\nX\nc\n"),
            (3, "100644", b"a\nY\nc\n"),
        ],
    );
    let model = text(repo.conflict("sub/f.txt").unwrap());
    let before = git(dir, &["ls-files", "-u", "--", "sub/f.txt"]);
    // git cannot create the file in a read-only directory.
    fs::set_permissions(dir.join("sub"), fs::Permissions::from_mode(0o555)).unwrap();
    let result = repo.save_resolution("sub/f.txt", "a\nZ\nc\n", &model.guard);
    fs::set_permissions(dir.join("sub"), fs::Permissions::from_mode(0o755)).unwrap();
    if fs::write(dir.join("sub/probe"), "x").is_err() {
        return; // not enforced (running as root)
    }
    assert!(result.is_err(), "checkout-index must fail");
    assert_eq!(git(dir, &["ls-files", "-u", "--", "sub/f.txt"]), before);
}

#[test]
fn diff3_and_zdiff3_files_are_compared_by_their_sides() {
    for style in ["diff3", "zdiff3"] {
        let (tmp, repo) = new_repo();
        let dir = tmp.path();
        git(dir, &["config", "merge.conflictStyle", style]);
        {
            let (name, base, ours, theirs) = ("f.txt", "1\nb\n2\n", "1\nX\n2\n", "1\nY\n2\n");
            fs::write(dir.join(name), base).unwrap();
            git(dir, &["add", name]);
            git(dir, &["commit", "-q", "-m", "base"]);
            git(dir, &["branch", "side"]);
            fs::write(dir.join(name), ours).unwrap();
            git(dir, &["commit", "-q", "-am", "ours"]);
            git(dir, &["switch", "-q", "side"]);
            fs::write(dir.join(name), theirs).unwrap();
            git(dir, &["commit", "-q", "-am", "theirs"]);
            git(dir, &["switch", "-q", "main"]);
        }
        git_command(dir)
            .args(["merge", "side", "-q", "--no-edit"])
            .output()
            .unwrap();
        let written = fs::read_to_string(dir.join("f.txt")).unwrap();
        assert!(
            written.contains("|||||||"),
            "{style}: git wrote a base part"
        );
        let model = text(repo.conflict("f.txt").unwrap());
        assert!(!model.guard.changed_after_merge, "{style}: untouched");
        fs::write(dir.join("f.txt"), written.replace("X", "Z")).unwrap();
        let model = text(repo.conflict("f.txt").unwrap());
        assert!(model.guard.changed_after_merge, "{style}: edited");
    }
}
