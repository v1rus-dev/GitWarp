use crate::*;

fn p(url: &str) -> RemoteRef {
    RemoteRef::parse(url).unwrap_or_else(|| panic!("should parse: {url}"))
}

fn inst(kind: ServiceKind, url: &str) -> Instance {
    Instance::new(kind, url).unwrap()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn account(id: &str, kind: ServiceKind, url: &str) -> AccountRef {
    AccountRef {
        id: id.into(),
        instance: inst(kind, url),
        login: id.into(),
    }
}

// ---- ServiceKind ----

#[test]
fn kind_ids_roundtrip_and_names() {
    for k in ServiceKind::ALL {
        assert_eq!(ServiceKind::from_id(k.id()), Some(k));
    }
    assert_eq!(ServiceKind::from_id("nope"), None);
    assert_eq!(ServiceKind::Gitea.display_name(), "Gitea / Forgejo");
    assert_eq!(ServiceKind::AzureDevOps.id(), "azure");
    assert_eq!(ServiceKind::GitLab.pr_noun(), "merge request");
    assert_eq!(ServiceKind::GitHub.pr_noun(), "pull request");
}

#[test]
fn git_username_per_service() {
    assert_eq!(ServiceKind::GitHub.git_username("mira"), "mira");
    assert_eq!(ServiceKind::Gitea.git_username("mira-k"), "mira-k");
    assert_eq!(
        ServiceKind::AzureDevOps.git_username("mira@acme"),
        "mira@acme"
    );
    // The login is irrelevant for these two: the service wants a fixed name.
    assert_eq!(ServiceKind::GitLab.git_username("mira"), "oauth2");
    assert_eq!(
        ServiceKind::Bitbucket.git_username("mira"),
        "x-bitbucket-api-token-auth"
    );
}

// ---- Instance ----

#[test]
fn instance_normalization() {
    let k = ServiceKind::GitLab;
    assert_eq!(
        inst(k, "gitlab.company.com").base_url,
        "https://gitlab.company.com"
    );
    assert_eq!(
        inst(k, "HTTPS://GitLab.Company.COM/").base_url,
        "https://gitlab.company.com"
    );
    assert_eq!(inst(k, "http://host:8080//").base_url, "http://host:8080");
    assert_eq!(
        inst(k, "example.com/gitlab/").base_url,
        "https://example.com/gitlab"
    );
    assert_eq!(
        inst(k, "  https://h.io/x?a=1#f ").base_url,
        "https://h.io/x"
    );
}

#[test]
fn instance_invalid_input() {
    let k = ServiceKind::GitHub;
    assert_eq!(Instance::new(k, "  "), Err(InstanceError::Empty));
    assert_eq!(
        Instance::new(k, "ftp://host"),
        Err(InstanceError::UnsupportedScheme("ftp".into()))
    );
    assert_eq!(
        Instance::new(k, "git@host:o/r"),
        Err(InstanceError::Unparsable)
    );
    assert_eq!(Instance::new(k, "https://"), Err(InstanceError::Unparsable));
    assert_eq!(
        Instance::new(k, "https://ho st"),
        Err(InstanceError::Unparsable)
    );
    assert_eq!(
        Instance::new(k, "https://h:99999"),
        Err(InstanceError::Unparsable)
    );
}

#[test]
fn instance_parts_and_public() {
    let i = inst(ServiceKind::GitLab, "https://Example.com:8443/gitlab");
    assert_eq!(i.host(), "example.com");
    assert_eq!(i.sub_path(), "/gitlab");
    assert!(!i.is_public());
    assert!(inst(ServiceKind::GitHub, "github.com").is_public());
    assert!(!inst(ServiceKind::GitLab, "github.com").is_public());
    assert!(!inst(ServiceKind::GitLab, "gitlab.com:8443").is_public());
    for (kind, host) in PUBLIC_INSTANCES {
        assert!(inst(kind, host).is_public(), "{host}");
    }
}

#[test]
fn api_bases() {
    assert_eq!(
        inst(ServiceKind::GitHub, "github.com").api_base(),
        "https://api.github.com"
    );
    assert_eq!(
        inst(ServiceKind::GitHub, "ghe.corp").api_base(),
        "https://ghe.corp/api/v3"
    );
    assert_eq!(
        inst(ServiceKind::GitLab, "gitlab.com").api_base(),
        "https://gitlab.com/api/v4"
    );
    assert_eq!(
        inst(ServiceKind::GitLab, "h/gitlab").api_base(),
        "https://h/gitlab/api/v4"
    );
    assert_eq!(
        inst(ServiceKind::Gitea, "codeberg.org").api_base(),
        "https://codeberg.org/api/v1"
    );
    assert_eq!(
        inst(ServiceKind::Bitbucket, "bitbucket.org").api_base(),
        "https://api.bitbucket.org/2.0"
    );
    assert_eq!(
        inst(ServiceKind::Bitbucket, "bb.corp").api_base(),
        "https://bb.corp/rest/api/1.0"
    );
    assert_eq!(
        inst(ServiceKind::AzureDevOps, "dev.azure.com").api_base(),
        "https://dev.azure.com"
    );
}

#[test]
fn known_kinds() {
    assert_eq!(known_kind_for_host("github.com"), Some(ServiceKind::GitHub));
    assert_eq!(
        known_kind_for_host("SSH.GitHub.com"),
        Some(ServiceKind::GitHub)
    );
    assert_eq!(
        known_kind_for_host("codeberg.org"),
        Some(ServiceKind::Gitea)
    );
    assert_eq!(
        known_kind_for_host("ssh.dev.azure.com"),
        Some(ServiceKind::AzureDevOps)
    );
    assert_eq!(
        known_kind_for_host("acme.visualstudio.com"),
        Some(ServiceKind::AzureDevOps)
    );
    assert_eq!(known_kind_for_host("git.corp"), None);
}

// ---- RemoteRef::parse ----

#[test]
fn parse_https_forms() {
    let r = p("https://github.com/o/r");
    assert_eq!(r.host, "github.com");
    assert_eq!(r.port, None);
    assert_eq!(r.path, s(&["o"]));
    assert_eq!(r.repo, "r");
    assert_eq!(r.transport, Transport::Https);
    assert_eq!(p("https://github.com/o/r.git"), r);
    assert_eq!(p("https://github.com/o/r.git/"), r);
    assert_eq!(p("https://github.com/o/r/"), r);
    assert_eq!(p("https://user@GitHub.com/o/r.git"), r);
    assert_eq!(p("https://user:pw@github.com/o/r.git"), r);
    let h = p("http://git.corp:8080/o/r.git");
    assert_eq!((h.transport, h.port), (Transport::Http, Some(8080)));
}

#[test]
fn parse_ssh_forms() {
    let a = p("ssh://git@host.io:2222/o/r.git");
    assert_eq!(
        (a.host.as_str(), a.port, a.transport),
        ("host.io", Some(2222), Transport::Ssh)
    );
    assert_eq!((a.owner_path(), a.repo.as_str()), ("o".to_string(), "r"));
    let b = p("git@host.io:o/r.git");
    assert_eq!(
        (b.host.as_str(), b.port, b.transport),
        ("host.io", None, Transport::Ssh)
    );
    assert_eq!(b.full_name(), "o/r");
    assert_eq!(p("host.io:o/r"), b);
    assert_eq!(p("git@host.io:/o/r.git"), b);
}

#[test]
fn parse_nested_and_subpath() {
    let r = p("git@gitlab.com:group/sub/deep/r.git");
    assert_eq!(r.path, s(&["group", "sub", "deep"]));
    assert_eq!(r.full_name(), "group/sub/deep/r");
    let r = p("https://example.com/gitlab/group/r.git");
    assert_eq!(r.path, s(&["gitlab", "group"]));
}

#[test]
fn parse_azure_forms() {
    for url in [
        "https://dev.azure.com/org/project/_git/repo",
        "https://org@dev.azure.com/org/project/_git/repo",
        "git@ssh.dev.azure.com:v3/org/project/repo",
        "https://org.visualstudio.com/project/_git/repo",
        "https://org.visualstudio.com/DefaultCollection/project/_git/repo",
        "org@vs-ssh.visualstudio.com:v3/org/project/repo",
    ] {
        let r = p(url);
        assert_eq!(r.path, s(&["org", "project"]), "{url}");
        assert_eq!(r.repo, "repo", "{url}");
    }
    // Project-less URL: the project is named like the repository.
    assert_eq!(
        p("https://dev.azure.com/org/_git/repo").path,
        s(&["org", "repo"])
    );
}

#[test]
fn parse_rejects_non_remotes() {
    for url in [
        "",
        "  ",
        "/tmp/x",
        "../x",
        "./x",
        "~/x",
        "C:\\x",
        "C:/x",
        "file:///tmp/x",
        "file://host/o/r",
        "garbage",
        "git://host/o/r",
        "https://",
        "https://host",
        "https://host/onlyone",
        "host:onlyone",
        "https://host/o/.git",
        "ftp://host/o/r",
    ] {
        assert_eq!(RemoteRef::parse(url), None, "{url}");
    }
}

#[test]
fn web_urls_with_delimiters_in_the_authority_are_not_remotes() {
    for url in [
        "https://github.com?@evil.test/o/r",
        "https://github.com#@evil.test/o/r",
        "https://github.com%2F@evil.test/o/r",
        "http://github.com%2f@evil.test/o/r.git",
        "https://us er@github.com/o/r",
    ] {
        assert_eq!(RemoteRef::parse(url), None, "{url}");
    }
    // A query or fragment after the path is still dropped.
    assert_eq!(p("https://github.com/o/r.git?x=1#f").repo, "r");
    // `@` inside the path is path.
    assert_eq!(p("https://u@evil.test/o/p@github.com").host, "evil.test");
}

// ---- matching ----

#[test]
fn matches_basic_and_aliases() {
    let gh = inst(ServiceKind::GitHub, "github.com");
    let c = gh.matches(&p("git@github.com:o/r.git")).unwrap();
    assert_eq!((c.namespace, c.repo), (s(&["o"]), "r".to_string()));
    assert!(gh
        .matches(&p("ssh://git@ssh.github.com:443/o/r.git"))
        .is_some());
    assert!(gh.matches(&p("https://github.com:443/o/r")).is_some());
    assert!(gh.matches(&p("https://gitlab.com/o/r")).is_none());
    assert!(gh.matches(&p("https://github.com:8443/o/r")).is_none());
}

#[test]
fn matches_ssh_host() {
    let mut i = inst(ServiceKind::GitLab, "https://gitlab.corp");
    assert!(i.matches(&p("git@git-ssh.corp:g/r.git")).is_none());
    i.ssh_host = Some("git-ssh.corp".into());
    assert!(i.matches(&p("git@git-ssh.corp:g/r.git")).is_some());
    assert!(i.matches(&p("git@GITLAB.corp:g/r.git")).is_some());
}

#[test]
fn matches_sub_path() {
    let i = inst(ServiceKind::GitLab, "https://example.com/gitlab");
    let c = i
        .matches(&p("https://example.com/gitlab/group/sub/r.git"))
        .unwrap();
    assert_eq!(c.namespace, s(&["group", "sub"]));
    assert!(i
        .matches(&p("https://example.com/other/group/r.git"))
        .is_none());
    // The sub-path must leave a namespace behind.
    assert!(i.matches(&p("https://example.com/gitlab/r.git")).is_none());
    // scp-like paths do not include the web sub-path.
    let c = i.matches(&p("git@example.com:group/r.git")).unwrap();
    assert_eq!(c.namespace, s(&["group"]));
}

#[test]
fn matches_azure() {
    let az = inst(ServiceKind::AzureDevOps, "dev.azure.com");
    for url in [
        "https://dev.azure.com/org/project/_git/repo",
        "git@ssh.dev.azure.com:v3/org/project/repo",
        "https://org.visualstudio.com/project/_git/repo",
    ] {
        let c = az.matches(&p(url)).unwrap();
        assert_eq!(
            (c.namespace, c.repo),
            (s(&["org", "project"]), "repo".to_string()),
            "{url}"
        );
    }
}

// ---- resolve ----

#[test]
fn resolve_one_many_none() {
    let accounts = vec![
        account("gh1", ServiceKind::GitHub, "github.com"),
        account("gh2", ServiceKind::GitHub, "github.com"),
        account("gl", ServiceKind::GitLab, "gitlab.corp"),
    ];
    match resolve(&accounts, &p("git@gitlab.corp:g/r.git")) {
        Resolution::One(a) => assert_eq!(a.id, "gl"),
        other => panic!("{other:?}"),
    }
    match resolve(&accounts, &p("https://github.com/o/r")) {
        Resolution::Many(v) => {
            assert_eq!(
                v.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
                ["gh1", "gh2"]
            )
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        resolve(&accounts, &p("https://codeberg.org/o/r")),
        Resolution::None {
            known_kind: Some(ServiceKind::Gitea)
        }
    );
    assert_eq!(
        resolve(&accounts, &p("https://git.unknown/o/r")),
        Resolution::None { known_kind: None }
    );
    assert_eq!(
        resolve(&[], &p("git@ssh.dev.azure.com:v3/o/p/r")),
        Resolution::None {
            known_kind: Some(ServiceKind::AzureDevOps)
        }
    );
}

#[test]
fn resolve_remembered_choice() {
    let accounts = vec![
        account("gh1", ServiceKind::GitHub, "github.com"),
        account("gh2", ServiceKind::GitHub, "github.com"),
        account("gl", ServiceKind::GitLab, "gitlab.com"),
    ];
    let remote = p("https://github.com/o/r");
    match resolve_with(&accounts, &remote, Some("gh2")) {
        Resolution::One(a) => assert_eq!(a.id, "gh2"),
        other => panic!("{other:?}"),
    }
    // Stale id (removed account) and an id that no longer matches the host.
    assert!(matches!(
        resolve_with(&accounts, &remote, Some("gone")),
        Resolution::Many(_)
    ));
    assert!(matches!(
        resolve_with(&accounts, &remote, Some("gl")),
        Resolution::Many(_)
    ));
}

// ---- web URLs ----

fn coords(ns: &[&str], repo: &str) -> RepoCoords {
    RepoCoords {
        namespace: s(ns),
        repo: repo.into(),
    }
}

fn file(rev: &str, path: &str) -> WebTarget {
    WebTarget::File {
        rev: rev.into(),
        path: path.into(),
    }
}

#[test]
fn web_urls_github() {
    let i = inst(ServiceKind::GitHub, "github.com");
    let c = coords(&["o"], "r");
    let u = |t| web_url(&i, &c, &t);
    assert_eq!(u(WebTarget::Repo), "https://github.com/o/r");
    assert_eq!(
        u(WebTarget::Commit("abc".into())),
        "https://github.com/o/r/commit/abc"
    );
    assert_eq!(
        u(WebTarget::Branch("main".into())),
        "https://github.com/o/r/tree/main"
    );
    assert_eq!(
        u(WebTarget::Tag("v1".into())),
        "https://github.com/o/r/releases/tag/v1"
    );
    assert_eq!(
        u(file("abc", "src/a.rs")),
        "https://github.com/o/r/blob/abc/src/a.rs"
    );
    assert_eq!(
        u(WebTarget::PullRequest(7)),
        "https://github.com/o/r/pull/7"
    );
}

#[test]
fn web_urls_gitlab_with_sub_path() {
    let i = inst(ServiceKind::GitLab, "https://example.com/gitlab");
    let c = coords(&["g", "sub"], "r");
    let u = |t| web_url(&i, &c, &t);
    let base = "https://example.com/gitlab/g/sub/r";
    assert_eq!(u(WebTarget::Repo), base);
    assert_eq!(
        u(WebTarget::Commit("abc".into())),
        format!("{base}/-/commit/abc")
    );
    assert_eq!(
        u(WebTarget::Branch("main".into())),
        format!("{base}/-/tree/main")
    );
    assert_eq!(u(WebTarget::Tag("v1".into())), format!("{base}/-/tags/v1"));
    assert_eq!(u(file("abc", "a.rs")), format!("{base}/-/blob/abc/a.rs"));
    assert_eq!(
        u(WebTarget::PullRequest(3)),
        format!("{base}/-/merge_requests/3")
    );
}

#[test]
fn web_urls_gitea() {
    let i = inst(ServiceKind::Gitea, "codeberg.org");
    let c = coords(&["o"], "r");
    let u = |t| web_url(&i, &c, &t);
    let base = "https://codeberg.org/o/r";
    assert_eq!(
        u(WebTarget::Commit("abc".into())),
        format!("{base}/commit/abc")
    );
    assert_eq!(
        u(WebTarget::Branch("main".into())),
        format!("{base}/src/branch/main")
    );
    assert_eq!(u(WebTarget::Tag("v1".into())), format!("{base}/src/tag/v1"));
    assert_eq!(
        u(file("abc", "a.rs")),
        format!("{base}/src/commit/abc/a.rs")
    );
    assert_eq!(u(WebTarget::PullRequest(2)), format!("{base}/pulls/2"));
}

#[test]
fn web_urls_bitbucket() {
    let i = inst(ServiceKind::Bitbucket, "bitbucket.org");
    let c = coords(&["w"], "r");
    let u = |t| web_url(&i, &c, &t);
    let base = "https://bitbucket.org/w/r";
    assert_eq!(
        u(WebTarget::Commit("abc".into())),
        format!("{base}/commits/abc")
    );
    assert_eq!(
        u(WebTarget::Branch("main".into())),
        format!("{base}/branch/main")
    );
    assert_eq!(u(WebTarget::Tag("v1".into())), format!("{base}/src/v1"));
    assert_eq!(u(file("abc", "a.rs")), format!("{base}/src/abc/a.rs"));
    assert_eq!(
        u(WebTarget::PullRequest(5)),
        format!("{base}/pull-requests/5")
    );
}

#[test]
fn web_urls_azure() {
    let i = inst(ServiceKind::AzureDevOps, "dev.azure.com");
    let c = coords(&["org", "project"], "repo");
    let u = |t| web_url(&i, &c, &t);
    let base = "https://dev.azure.com/org/project/_git/repo";
    assert_eq!(u(WebTarget::Repo), base);
    assert_eq!(
        u(WebTarget::Commit("abc".into())),
        format!("{base}/commit/abc")
    );
    assert_eq!(
        u(WebTarget::Branch("main".into())),
        format!("{base}?version=GBmain")
    );
    assert_eq!(
        u(WebTarget::Tag("v1".into())),
        format!("{base}?version=GTv1")
    );
    assert_eq!(
        u(file("abc", "src/a.rs")),
        format!("{base}?path=/src/a.rs&version=GCabc")
    );
    assert_eq!(
        u(WebTarget::PullRequest(9)),
        format!("{base}/pullrequest/9")
    );
}

#[test]
fn web_urls_encoding() {
    let gh = inst(ServiceKind::GitHub, "github.com");
    let c = coords(&["o"], "r");
    assert_eq!(
        web_url(&gh, &c, &WebTarget::Branch("feat/my branch#1".into())),
        "https://github.com/o/r/tree/feat/my%20branch%231"
    );
    assert_eq!(
        web_url(&gh, &c, &file("abc", "docs/файл?%.md")),
        "https://github.com/o/r/blob/abc/docs/%D1%84%D0%B0%D0%B9%D0%BB%3F%25.md"
    );
    let az = inst(ServiceKind::AzureDevOps, "dev.azure.com");
    let ac = coords(&["org", "project"], "repo");
    assert_eq!(
        web_url(&az, &ac, &WebTarget::Branch("feat/my branch#1".into())),
        "https://dev.azure.com/org/project/_git/repo?version=GBfeat/my%20branch%231"
    );
}

fn blame(rev: &str, path: &str) -> WebTarget {
    WebTarget::Blame {
        rev: rev.into(),
        path: path.into(),
    }
}

#[test]
fn blame_urls_per_service() {
    let t = |kind, host: &str, ns: &[&str]| {
        let i = inst(kind, host);
        let c = coords(ns, "r");
        try_web_url(&i, &c, &blame("abc", "src/a.rs"))
    };
    assert_eq!(
        t(ServiceKind::GitHub, "github.com", &["o"]).as_deref(),
        Some("https://github.com/o/r/blame/abc/src/a.rs")
    );
    assert_eq!(
        t(ServiceKind::GitLab, "gitlab.com", &["g", "s"]).as_deref(),
        Some("https://gitlab.com/g/s/r/-/blame/abc/src/a.rs")
    );
    assert_eq!(
        t(ServiceKind::Gitea, "codeberg.org", &["o"]).as_deref(),
        Some("https://codeberg.org/o/r/blame/commit/abc/src/a.rs")
    );
    assert_eq!(
        t(ServiceKind::Bitbucket, "bitbucket.org", &["w"]).as_deref(),
        Some("https://bitbucket.org/w/r/annotate/abc/src/a.rs")
    );
}

#[test]
fn blame_on_azure_has_no_page() {
    let i = inst(ServiceKind::AzureDevOps, "dev.azure.com");
    let c = coords(&["org", "project"], "repo");
    assert_eq!(try_web_url(&i, &c, &blame("abc", "a.rs")), None);
    // `web_url` stays total: the file page stands in.
    assert_eq!(
        web_url(&i, &c, &blame("abc", "a.rs")),
        web_url(&i, &c, &file("abc", "a.rs"))
    );
    assert!(try_web_url(&i, &c, &file("abc", "a.rs")).is_some());
}

#[test]
fn blame_urls_encode_the_path() {
    let gh = inst(ServiceKind::GitHub, "github.com");
    let c = coords(&["o"], "r");
    assert_eq!(
        web_url(&gh, &c, &blame("abc", "my dir/a#b/файл.rs")),
        "https://github.com/o/r/blame/abc/my%20dir/a%23b/%D1%84%D0%B0%D0%B9%D0%BB.rs"
    );
}

#[test]
fn web_urls_never_carry_remote_credentials() {
    let remote = RemoteRef::parse("https://user:pass@github.com/o/r.git").unwrap();
    let i = inst(ServiceKind::GitHub, "github.com");
    let c = i.matches(&remote).unwrap();
    let url = web_url(&i, &c, &WebTarget::Commit("abc".into()));
    assert_eq!(url, "https://github.com/o/r/commit/abc");
    assert!(!url.contains("user") && !url.contains("pass") && !url.contains('@'));
}
