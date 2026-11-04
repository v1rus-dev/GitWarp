use super::repos::list_repos_at;
use super::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<(String, HashMap<String, String>)>>>;

/// Canned response: status, extra headers, body.
type Canned = (u16, Vec<(&'static str, &'static str)>, &'static str);

/// Minimal HTTP/1.1 stub: serves canned responses per path, 404 otherwise,
/// and records the headers of every request.
struct Stub {
    base: String,
    seen: Seen,
}

impl Stub {
    fn start(routes: Vec<(&'static str, Canned)>) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen: Seen = Arc::default();
        let log = seen.clone();
        let base_for_thread = base.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut headers = HashMap::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = line.split_once(':') {
                        headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                    }
                }
                let path = request_line.split_whitespace().nth(1).unwrap_or("/");
                let target = path.to_string();
                let path = path.split('?').next().unwrap().to_string();
                log.lock().unwrap().push((path.clone(), headers));
                let (status, extra, body) = routes
                    .iter()
                    // A route may name the query too; it wins over the bare path.
                    .find(|(p, _)| *p == target)
                    .or_else(|| routes.iter().find(|(p, _)| *p == path))
                    .map(|(_, c)| c.clone())
                    .unwrap_or((404, vec![], ""));
                // `{base}` in a header or body stands for this stub's address.
                let body = body.replace("{base}", &base_for_thread);
                let mut out = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in extra {
                    out.push_str(&format!(
                        "{k}: {}\r\n",
                        v.replace("{base}", &base_for_thread)
                    ));
                }
                out.push_str("\r\n");
                out.push_str(&body);
                let _ = reader.get_mut().write_all(out.as_bytes());
            }
        });
        Stub { base, seen }
    }

    fn header(&self, path: &str, name: &str) -> Option<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .find(|(p, _)| p == path)
            .and_then(|(_, h)| h.get(name).cloned())
    }

    fn requested(&self, path: &str) -> bool {
        self.seen.lock().unwrap().iter().any(|(p, _)| p == path)
    }
}

fn ok(body: &'static str) -> Canned {
    (200, vec![], body)
}

fn instance(kind: ServiceKind, stub: &Stub) -> Instance {
    Instance::new(kind, &stub.base).unwrap()
}

fn check(kind: ServiceKind, stub: &Stub, kind_of_token: TokenKind) -> Result<TokenCheck, ApiError> {
    current_user(
        &Client::for_tests(),
        &instance(kind, stub),
        "sekret",
        kind_of_token,
    )
}

// ---- base64 ----

#[test]
fn base64_rfc4648_vectors() {
    for (input, expected) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
        (":pat", "OnBhdA=="),
    ] {
        assert_eq!(base64(input.as_bytes()), expected, "{input:?}");
    }
}

// ---- probe ----

fn probe_of(stub: &Stub) -> Result<Probe, ApiError> {
    probe(&Client::for_tests(), &stub.base)
}

#[test]
fn probe_public_host_needs_no_request() {
    let probe = probe(&Client::for_tests(), "https://github.com").unwrap();
    assert_eq!(probe.kind, Some(ServiceKind::GitHub));
    assert_eq!(probe.version, None);
}

#[test]
fn probe_gitlab_version() {
    let stub = Stub::start(vec![(
        "/api/v4/version",
        ok(r#"{"version":"16.9.1","revision":"x"}"#),
    )]);
    let p = probe_of(&stub).unwrap();
    assert_eq!(p.kind, Some(ServiceKind::GitLab));
    assert_eq!(p.version.as_deref(), Some("16.9.1"));
}

#[test]
fn probe_gitlab_401_with_json_body() {
    let stub = Stub::start(vec![(
        "/api/v4/version",
        (401, vec![], r#"{"message":"401 Unauthorized"}"#),
    )]);
    let p = probe_of(&stub).unwrap();
    assert_eq!(p.kind, Some(ServiceKind::GitLab));
    assert_eq!(p.version, None);
}

#[test]
fn probe_gitea() {
    let stub = Stub::start(vec![("/api/v1/version", ok(r#"{"version":"1.22.0"}"#))]);
    let p = probe_of(&stub).unwrap();
    assert_eq!(p.kind, Some(ServiceKind::Gitea));
    assert_eq!(p.version.as_deref(), Some("1.22.0"));
}

#[test]
fn probe_github_enterprise_meta() {
    let stub = Stub::start(vec![(
        "/api/v3/meta",
        ok(r#"{"verifiable_password_authentication":true,"installed_version":"3.12.0"}"#),
    )]);
    let p = probe_of(&stub).unwrap();
    assert_eq!(p.kind, Some(ServiceKind::GitHub));
    assert_eq!(p.version.as_deref(), Some("3.12.0"));
}

#[test]
fn probe_bitbucket_server() {
    let stub = Stub::start(vec![(
        "/rest/api/1.0/application-properties",
        ok(r#"{"version":"8.19.1","displayName":"Bitbucket"}"#),
    )]);
    let p = probe_of(&stub).unwrap();
    assert_eq!(p.kind, Some(ServiceKind::Bitbucket));
    assert_eq!(p.version.as_deref(), Some("8.19.1"));
}

#[test]
fn probe_unrecognized() {
    let stub = Stub::start(vec![("/", ok("<html></html>"))]);
    assert_eq!(
        probe_of(&stub).unwrap(),
        Probe {
            kind: None,
            version: None
        }
    );
}

#[test]
fn probe_unreachable_when_port_is_closed() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let err = probe(&Client::for_tests(), &format!("http://127.0.0.1:{port}")).unwrap_err();
    assert!(matches!(err, ApiError::Unreachable(_)), "{err:?}");
}

// ---- current_user ----

#[test]
fn github_scopes_header_reports_missing() {
    let stub = Stub::start(vec![(
        "/api/v3/user",
        (
            200,
            vec![("x-oauth-scopes", "repo, gist")],
            r#"{"login":"octo","name":"Octo Cat","avatar_url":"http://a/1"}"#,
        ),
    )]);
    let c = check(ServiceKind::GitHub, &stub, TokenKind::Pat).unwrap();
    assert_eq!(c.user.login, "octo");
    assert_eq!(c.user.name.as_deref(), Some("Octo Cat"));
    assert_eq!(c.user.avatar_url.as_deref(), Some("http://a/1"));
    assert_eq!(c.scopes, Some(vec!["repo".to_string(), "gist".to_string()]));
    assert_eq!(
        c.missing,
        vec!["read:user".to_string(), "workflow".to_string()]
    );
    assert_eq!(
        stub.header("/api/v3/user", "authorization").as_deref(),
        Some("Bearer sekret")
    );
}

#[test]
fn github_user_scope_implies_read_user() {
    let stub = Stub::start(vec![(
        "/api/v3/user",
        (
            200,
            vec![("x-oauth-scopes", "repo,user,workflow")],
            r#"{"login":"o"}"#,
        ),
    )]);
    let c = check(ServiceKind::GitHub, &stub, TokenKind::Pat).unwrap();
    assert!(c.missing.is_empty(), "{:?}", c.missing);
}

#[test]
fn github_fine_grained_token_has_no_scopes() {
    let stub = Stub::start(vec![("/api/v3/user", ok(r#"{"login":"octo"}"#))]);
    let c = check(ServiceKind::GitHub, &stub, TokenKind::Pat).unwrap();
    assert_eq!(c.scopes, None);
    assert!(c.missing.is_empty());
}

#[test]
fn unauthorized_maps_to_error() {
    let stub = Stub::start(vec![(
        "/api/v3/user",
        (401, vec![], r#"{"message":"Bad"}"#),
    )]);
    assert_eq!(
        check(ServiceKind::GitHub, &stub, TokenKind::Pat),
        Err(ApiError::Unauthorized)
    );
}

#[test]
fn github_403_with_exhausted_limit_is_rate_limited() {
    let stub = Stub::start(vec![(
        "/api/v3/user",
        (
            403,
            vec![
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", "1700000000"),
            ],
            "{}",
        ),
    )]);
    assert_eq!(
        check(ServiceKind::GitHub, &stub, TokenKind::Pat),
        Err(ApiError::RateLimited {
            reset_at: Some(1_700_000_000)
        })
    );
}

#[test]
fn plain_403_is_forbidden_and_404_not_found() {
    let stub = Stub::start(vec![("/api/v3/user", (403, vec![], "{}"))]);
    assert_eq!(
        check(ServiceKind::GitHub, &stub, TokenKind::Pat),
        Err(ApiError::Forbidden)
    );
    let stub = Stub::start(vec![]);
    assert_eq!(
        check(ServiceKind::GitHub, &stub, TokenKind::Pat),
        Err(ApiError::NotFound)
    );
    let stub = Stub::start(vec![("/api/v3/user", (500, vec![], "{}"))]);
    assert_eq!(
        check(ServiceKind::GitHub, &stub, TokenKind::Pat),
        Err(ApiError::Status(500))
    );
}

#[test]
fn undecodable_body_is_decode_error() {
    let stub = Stub::start(vec![("/api/v3/user", ok("not json"))]);
    assert!(matches!(
        check(ServiceKind::GitHub, &stub, TokenKind::Pat),
        Err(ApiError::Decode(_))
    ));
}

fn gitlab_routes() -> Vec<(&'static str, Canned)> {
    vec![
        (
            "/api/v4/user",
            ok(r#"{"username":"lab","name":"Lab User","avatar_url":"http://a/2"}"#),
        ),
        (
            "/api/v4/personal_access_tokens/self",
            ok(r#"{"scopes":["api"]}"#),
        ),
    ]
}

#[test]
fn gitlab_pat_uses_private_token_header_and_reads_scopes() {
    let stub = Stub::start(gitlab_routes());
    let c = check(ServiceKind::GitLab, &stub, TokenKind::Pat).unwrap();
    assert_eq!(c.user.login, "lab");
    assert_eq!(c.scopes, Some(vec!["api".to_string()]));
    assert!(c.missing.is_empty());
    assert_eq!(
        stub.header("/api/v4/user", "private-token").as_deref(),
        Some("sekret")
    );
    assert_eq!(stub.header("/api/v4/user", "authorization"), None);
}

#[test]
fn gitlab_oauth_uses_bearer_and_tolerates_missing_scope_endpoint() {
    let stub = Stub::start(vec![(
        "/api/v4/user",
        ok(r#"{"username":"lab","name":"Lab User"}"#),
    )]);
    let c = check(ServiceKind::GitLab, &stub, TokenKind::OAuth).unwrap();
    assert_eq!(c.scopes, None);
    assert_eq!(
        stub.header("/api/v4/user", "authorization").as_deref(),
        Some("Bearer sekret")
    );
    assert_eq!(stub.header("/api/v4/user", "private-token"), None);
}

#[test]
fn gitlab_read_only_scopes_report_missing() {
    let mut routes = gitlab_routes();
    routes[1] = (
        "/api/v4/personal_access_tokens/self",
        ok(r#"{"scopes":["read_api","read_repository"]}"#),
    );
    let stub = Stub::start(routes);
    let c = check(ServiceKind::GitLab, &stub, TokenKind::Pat).unwrap();
    assert_eq!(
        c.missing,
        vec!["api".to_string(), "write_repository".to_string()]
    );
}

#[test]
fn gitea_uses_token_scheme() {
    let stub = Stub::start(vec![(
        "/api/v1/user",
        ok(r#"{"login":"tea","full_name":"","avatar_url":"http://a/3"}"#),
    )]);
    let c = check(ServiceKind::Gitea, &stub, TokenKind::Pat).unwrap();
    assert_eq!(c.user.login, "tea");
    assert_eq!(c.user.name, None);
    assert_eq!(c.scopes, None);
    assert_eq!(
        stub.header("/api/v1/user", "authorization").as_deref(),
        Some("token sekret")
    );
}

#[test]
fn bitbucket_cloud_user_shape() {
    // Non-public Bitbucket instances use the Server base path, so call the
    // helper directly with a stub-based API root.
    let stub = Stub::start(vec![(
        "/user",
        ok(
            r#"{"nickname":"bb","display_name":"Bit Bucket","links":{"avatar":{"href":"http://a/4"}}}"#,
        ),
    )]);
    let c = bitbucket_user(&Client::for_tests(), &stub.base, "sekret").unwrap();
    assert_eq!(c.user.login, "bb");
    assert_eq!(c.user.name.as_deref(), Some("Bit Bucket"));
    assert_eq!(c.user.avatar_url.as_deref(), Some("http://a/4"));
    assert_eq!(
        stub.header("/user", "authorization").as_deref(),
        Some("Bearer sekret")
    );
}

#[test]
fn azure_uses_basic_auth_with_empty_user() {
    let stub = Stub::start(vec![(
        "/_apis/profile/profiles/me",
        ok(r#"{"displayName":"Az User","emailAddress":"az@example.com","id":"1"}"#),
    )]);
    let url = format!("{}/_apis/profile/profiles/me?api-version=7.1", stub.base);
    let c = azure_user(&Client::for_tests(), &url, "pat").unwrap();
    assert_eq!(c.user.login, "az@example.com");
    assert_eq!(c.user.name.as_deref(), Some("Az User"));
    assert_eq!(
        stub.header("/_apis/profile/profiles/me", "authorization")
            .as_deref(),
        Some(format!("Basic {}", base64(b":pat")).as_str())
    );
    assert!(stub.requested("/_apis/profile/profiles/me"));
}

#[test]
fn errors_never_contain_the_token() {
    let stub = Stub::start(vec![("/api/v3/user", (401, vec![], "{}"))]);
    let err = check(ServiceKind::GitHub, &stub, TokenKind::Pat).unwrap_err();
    assert!(!err.to_string().contains("sekret"));
    assert!(!format!("{err:?}").contains("sekret"));
}

// ---- repo_is_public ----

fn coords(namespace: &[&str], repo: &str) -> RepoCoords {
    RepoCoords {
        namespace: namespace.iter().map(|s| s.to_string()).collect(),
        repo: repo.to_string(),
    }
}

fn is_public(kind: ServiceKind, stub: &Stub, c: &RepoCoords) -> Result<Option<bool>, ApiError> {
    repo_is_public(&Client::for_tests(), &instance(kind, stub), c)
}

#[test]
fn public_flag_reads_private_and_visibility() {
    let json = |s: &str| serde_json::from_str::<Value>(s).unwrap();
    assert_eq!(
        public_flag(ServiceKind::GitHub, &json(r#"{"private":false}"#)),
        Some(true)
    );
    assert_eq!(
        public_flag(ServiceKind::Gitea, &json(r#"{"private":true}"#)),
        Some(false)
    );
    assert_eq!(
        public_flag(ServiceKind::GitLab, &json(r#"{"visibility":"public"}"#)),
        Some(true)
    );
    assert_eq!(
        public_flag(ServiceKind::GitLab, &json(r#"{"visibility":"internal"}"#)),
        Some(false)
    );
    assert_eq!(public_flag(ServiceKind::GitHub, &json(r#"{}"#)), None);
}

#[test]
fn github_public_and_private_repositories() {
    let stub = Stub::start(vec![
        ("/api/v3/repos/acme/open", ok(r#"{"private":false}"#)),
        ("/api/v3/repos/acme/closed", ok(r#"{"private":true}"#)),
    ]);
    let github = |c: &RepoCoords| is_public(ServiceKind::GitHub, &stub, c);
    assert_eq!(github(&coords(&["acme"], "open")), Ok(Some(true)));
    assert_eq!(github(&coords(&["acme"], "closed")), Ok(Some(false)));
    // The service answers 404 for a private repository it hides.
    assert_eq!(github(&coords(&["acme"], "hidden")), Ok(Some(false)));
    // No token is ever sent.
    assert_eq!(
        stub.header("/api/v3/repos/acme/open", "authorization"),
        None
    );
}

#[test]
fn gitlab_asks_for_the_encoded_full_path() {
    let stub = Stub::start(vec![(
        "/api/v4/projects/group%2Fsub%2Frepo",
        ok(r#"{"visibility":"public"}"#),
    )]);
    let c = coords(&["group", "sub"], "repo");
    assert_eq!(is_public(ServiceKind::GitLab, &stub, &c), Ok(Some(true)));
}

#[test]
fn gitea_private_flag() {
    let stub = Stub::start(vec![("/api/v1/repos/acme/repo", ok(r#"{"private":true}"#))]);
    let c = coords(&["acme"], "repo");
    assert_eq!(is_public(ServiceKind::Gitea, &stub, &c), Ok(Some(false)));
}

#[test]
fn unauthorized_and_forbidden_mean_not_public_but_rate_limit_is_an_error() {
    let stub = Stub::start(vec![
        ("/api/v3/repos/acme/a", (401, vec![], "{}")),
        ("/api/v3/repos/acme/b", (403, vec![], "{}")),
        (
            "/api/v3/repos/acme/c",
            (403, vec![("x-ratelimit-remaining", "0")], "{}"),
        ),
    ]);
    let github = |r: &str| is_public(ServiceKind::GitHub, &stub, &coords(&["acme"], r));
    assert_eq!(github("a"), Ok(Some(false)));
    assert_eq!(github("b"), Ok(Some(false)));
    assert!(matches!(github("c"), Err(ApiError::RateLimited { .. })));
}

#[test]
fn bitbucket_and_azure_have_no_anonymous_answer() {
    let stub = Stub::start(vec![]);
    let c = coords(&["acme"], "repo");
    assert_eq!(is_public(ServiceKind::Bitbucket, &stub, &c), Ok(None));
    assert_eq!(is_public(ServiceKind::AzureDevOps, &stub, &c), Ok(None));
    assert!(stub.seen.lock().unwrap().is_empty());
}

#[test]
fn a_body_without_visibility_is_a_decode_error() {
    let stub = Stub::start(vec![("/api/v3/repos/acme/odd", ok(r#"{"name":"odd"}"#))]);
    let c = coords(&["acme"], "odd");
    assert!(matches!(
        is_public(ServiceKind::GitHub, &stub, &c),
        Err(ApiError::Decode(_))
    ));
}

#[test]
fn a_redirect_is_unknown_not_private() {
    let stub = Stub::start(vec![(
        "/api/v3/repos/acme/moved",
        (301, vec![("location", "http://127.0.0.1:1/elsewhere")], ""),
    )]);
    let c = coords(&["acme"], "moved");
    assert_eq!(is_public(ServiceKind::GitHub, &stub, &c), Ok(None));
}

// ---- list_user_repos ----

fn list(kind: ServiceKind, stub: &Stub) -> Result<Vec<RepoSummary>, ApiError> {
    list_listing(kind, stub).map(|listing| listing.repos)
}

fn list_listing(kind: ServiceKind, stub: &Stub) -> Result<RepoListing, ApiError> {
    list_user_repos(&Client::for_tests(), &instance(kind, stub), "sekret")
}

const GH_FIRST: &str =
    "/api/v3/user/repos?per_page=100&sort=updated&affiliation=owner,collaborator,organization_member";

#[test]
fn github_repos_are_parsed_sorted_and_use_the_token() {
    let stub = Stub::start(vec![(
        "/api/v3/user/repos",
        ok(r#"[
          {"name":"old","owner":{"login":"mira"},"description":null,"private":true,"fork":false,
           "archived":true,"updated_at":"2020-01-01T00:00:00Z","clone_url":"https://h/mira/old.git"},
          {"name":"new","owner":{"login":"acme"},"description":" A tool ","private":false,"fork":true,
           "archived":false,"updated_at":"2024-05-01T10:00:00Z","clone_url":"https://h/acme/new.git"},
          {"owner":{"login":"x"},"clone_url":"broken"}
        ]"#),
    )]);
    let repos = list(ServiceKind::GitHub, &stub).unwrap();
    assert_eq!(repos.len(), 2, "an item without a name is skipped");
    assert_eq!(
        repos[0],
        RepoSummary {
            owner: "acme".into(),
            name: "new".into(),
            description: Some("A tool".into()),
            private: false,
            fork: true,
            archived: false,
            updated_at: 1_714_557_600,
            clone_https: "https://h/acme/new.git".into(),
        }
    );
    assert_eq!(repos[1].name, "old");
    assert!(repos[1].private && repos[1].archived && repos[1].description.is_none());
    assert_eq!(
        stub.header("/api/v3/user/repos", "authorization")
            .as_deref(),
        Some("Bearer sekret")
    );
}

#[test]
fn github_follows_the_link_header_to_the_last_page() {
    let item = |n: &str, t: &str| {
        format!(
            r#"{{"name":"{n}","owner":{{"login":"a"}},"private":false,"fork":false,"archived":false,"updated_at":"{t}","clone_url":"https://h/a/{n}.git"}}"#
        )
    };
    let page1: &'static str =
        Box::leak(format!("[{}]", item("one", "2024-01-01T00:00:00Z")).into_boxed_str());
    let page2: &'static str =
        Box::leak(format!("[{}]", item("two", "2024-02-01T00:00:00Z")).into_boxed_str());
    let stub = Stub::start(vec![
        (
            GH_FIRST,
            (
                200,
                vec![(
                    "Link",
                    r#"<{base}/api/v3/user/repos?page=2>; rel="next", <{base}/api/v3/user/repos?page=2>; rel="last""#,
                )],
                page1,
            ),
        ),
        ("/api/v3/user/repos?page=2", ok(page2)),
    ]);
    let repos = list(ServiceKind::GitHub, &stub).unwrap();
    let names: Vec<_> = repos.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["two", "one"]);
}

#[test]
fn a_next_link_to_another_origin_is_not_followed() {
    let stub = Stub::start(vec![(
        "/api/v3/user/repos",
        (
            200,
            vec![("Link", r#"<http://127.0.0.1:1/steal>; rel="next""#)],
            "[]",
        ),
    )]);
    assert_eq!(list(ServiceKind::GitHub, &stub), Ok(vec![]));
    assert_eq!(stub.seen.lock().unwrap().len(), 1);
}

#[test]
fn gitlab_repos_follow_x_next_page() {
    let stub = Stub::start(vec![
        (
            "/api/v4/projects?membership=true&order_by=last_activity_at&per_page=100",
            (
                200,
                vec![("X-Next-Page", "2")],
                r#"[{"path":"tool","namespace":{"full_path":"grp/sub"},"description":"",
                    "visibility":"internal","archived":false,"last_activity_at":"2024-03-01T00:00:00.000Z",
                    "http_url_to_repo":"https://gl/grp/sub/tool.git"}]"#,
            ),
        ),
        (
            "/api/v4/projects?membership=true&order_by=last_activity_at&per_page=100&page=2",
            (
                200,
                vec![("X-Next-Page", "")],
                r#"[{"path":"priv","namespace":{"full_path":"me"},"visibility":"private","archived":true,
                    "forked_from_project":{"id":1},"last_activity_at":"2024-04-01T12:00:00+02:00",
                    "http_url_to_repo":"https://gl/me/priv.git"}]"#,
            ),
        ),
    ]);
    let repos = list(ServiceKind::GitLab, &stub).unwrap();
    assert_eq!(repos.len(), 2);
    let priv_repo = &repos[0];
    assert_eq!(
        (priv_repo.owner.as_str(), priv_repo.name.as_str()),
        ("me", "priv")
    );
    assert!(priv_repo.private && priv_repo.fork && priv_repo.archived);
    assert_eq!(
        priv_repo.updated_at, 1_711_965_600,
        "+02:00 is converted to UTC"
    );
    let tool = &repos[1];
    assert_eq!(tool.owner, "grp/sub");
    assert!(!tool.private, "internal is visible to every signed-in user");
    assert!(tool.description.is_none());
    assert_eq!(
        stub.header("/api/v4/projects", "private-token").as_deref(),
        Some("sekret"),
        "a personal access token goes the way every other GitLab call sends it"
    );
}

#[test]
fn gitea_repos_use_the_token_scheme_and_link_paging() {
    let stub = Stub::start(vec![
        (
            "/api/v1/user/repos?limit=50&page=1",
            (
                200,
                vec![(
                    "Link",
                    r#"<{base}/api/v1/user/repos?limit=50&page=2>; rel="next""#,
                )],
                r#"[{"name":"a","owner":{"login":"me"},"private":true,"fork":false,"archived":false,
                    "updated_at":"2024-01-01T00:00:00Z","clone_url":"https://g/me/a.git"}]"#,
            ),
        ),
        (
            "/api/v1/user/repos?limit=50&page=2",
            ok(
                r#"[{"name":"b","owner":{"login":"me"},"private":false,"fork":true,"archived":false,
                    "updated_at":"2024-06-01T00:00:00Z","clone_url":"https://g/me/b.git"}]"#,
            ),
        ),
    ]);
    let repos = list(ServiceKind::Gitea, &stub).unwrap();
    assert_eq!(
        repos.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["b", "a"]
    );
    assert_eq!(
        stub.header("/api/v1/user/repos", "authorization")
            .as_deref(),
        Some("token sekret")
    );
}

#[test]
fn bitbucket_cloud_repos_follow_next_and_drop_the_user_from_the_clone_url() {
    let stub = Stub::start(vec![
        (
            "/2.0/repositories?role=member&pagelen=100&sort=-updated_on",
            ok(
                r#"{"values":[{"full_name":"ws/one","description":"d","is_private":true,
                  "updated_on":"2024-05-01T10:00:00.123456+00:00",
                  "links":{"clone":[{"name":"ssh","href":"git@bitbucket.org:ws/one.git"},
                                     {"name":"https","href":"https://mira@bitbucket.org/ws/one.git"}]}}],
                  "next":"{base}/2.0/repositories?page=2"}"#,
            ),
        ),
        (
            "/2.0/repositories?page=2",
            ok(
                r#"{"values":[{"full_name":"ws/two","is_private":false,"parent":{"full_name":"x/two"},
                  "updated_on":"2023-01-01T00:00:00+00:00",
                  "links":{"clone":[{"name":"https","href":"https://bitbucket.org/ws/two.git"}]}}]}"#,
            ),
        ),
    ]);
    let api = format!("{}/2.0", stub.base);
    let repos = list_repos_at(&Client::for_tests(), ServiceKind::Bitbucket, &api, "sekret")
        .unwrap()
        .repos;
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0].clone_https, "https://bitbucket.org/ws/one.git");
    assert!(repos[0].private && !repos[0].fork && !repos[0].archived);
    assert_eq!(repos[0].updated_at, 1_714_557_600);
    assert!(repos[1].fork);
    assert_eq!(
        stub.header("/2.0/repositories", "authorization").as_deref(),
        Some("Bearer sekret")
    );
}

#[test]
fn listing_stops_at_the_cap() {
    let page = |from: usize| -> &'static str {
        let items: Vec<String> = (from..from + 300)
            .map(|n| {
                format!(
                    r#"{{"name":"r{n}","owner":{{"login":"a"}},"private":false,"fork":false,"archived":false,"updated_at":"2024-01-01T00:00:00Z","clone_url":"https://h/a/r{n}.git"}}"#
                )
            })
            .collect();
        Box::leak(format!("[{}]", items.join(",")).into_boxed_str())
    };
    let link = |n: u8| -> Vec<(&'static str, &'static str)> {
        let value: &'static str = Box::leak(
            format!(r#"<{{base}}/api/v3/user/repos?page={n}>; rel="next""#).into_boxed_str(),
        );
        vec![("Link", value)]
    };
    let stub = Stub::start(vec![
        (GH_FIRST, (200, link(2), page(0))),
        ("/api/v3/user/repos?page=2", (200, link(3), page(300))),
        ("/api/v3/user/repos?page=3", ok("[]")),
    ]);
    let listing = list_listing(ServiceKind::GitHub, &stub).unwrap();
    assert_eq!(listing.repos.len(), MAX_REPOS);
    assert!(listing.truncated);
    assert!(!stub.requested("/api/v3/user/repos?page=3"));
}

#[test]
fn an_empty_account_is_an_empty_list() {
    let stub = Stub::start(vec![("/api/v3/user/repos", ok("[]"))]);
    assert_eq!(
        list_listing(ServiceKind::GitHub, &stub),
        Ok(RepoListing {
            repos: vec![],
            truncated: false
        })
    );
}

#[test]
fn listing_maps_http_errors() {
    for (status, extra, expected) in [
        (401, vec![], ApiError::Unauthorized),
        (403, vec![], ApiError::Forbidden),
        (
            403,
            vec![("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "99")],
            ApiError::RateLimited { reset_at: Some(99) },
        ),
        (500, vec![], ApiError::Status(500)),
    ] {
        let stub = Stub::start(vec![("/api/v3/user/repos", (status, extra, "{}"))]);
        assert_eq!(list(ServiceKind::GitHub, &stub), Err(expected));
    }
}

#[test]
fn listing_rejects_a_body_that_is_not_a_list() {
    let stub = Stub::start(vec![("/api/v3/user/repos", ok(r#"{"message":"hi"}"#))]);
    assert!(matches!(
        list(ServiceKind::GitHub, &stub),
        Err(ApiError::Decode(_))
    ));
}

#[test]
fn azure_and_bitbucket_server_cannot_list() {
    let stub = Stub::start(vec![]);
    assert_eq!(
        list(ServiceKind::AzureDevOps, &stub),
        Err(ApiError::Unsupported)
    );
    // A self-hosted address is Bitbucket Server, whose API differs.
    assert_eq!(
        list(ServiceKind::Bitbucket, &stub),
        Err(ApiError::Unsupported)
    );
    assert!(stub.seen.lock().unwrap().is_empty());
}

// ---- releases ----

#[test]
fn list_releases_decodes_and_sends_github_headers() {
    let stub = Stub::start(vec![(
        // The route names the query: any other query would answer 404.
        "/repos/o/r/releases?per_page=100",
        ok(r#"[
            {"tag_name":"v0.2.0","html_url":"https://example.test/r/v0.2.0","prerelease":false,"draft":false,"id":1},
            {"tag_name":"v0.3.0-rc.1","html_url":"https://example.test/r/v0.3.0-rc.1","prerelease":true,"draft":true,"name":"x"}
        ]"#),
    )]);
    let releases = Client::for_tests()
        .list_releases_at(&stub.base, "o", "r")
        .unwrap();
    assert_eq!(releases.len(), 2);
    assert_eq!(releases[0].tag_name, "v0.2.0");
    assert_eq!(releases[0].html_url, "https://example.test/r/v0.2.0");
    assert!(!releases[0].prerelease && !releases[0].draft);
    assert!(releases[1].prerelease && releases[1].draft);
    assert_eq!(
        stub.header("/repos/o/r/releases", "accept").as_deref(),
        Some("application/vnd.github+json")
    );
    assert_eq!(
        stub.header("/repos/o/r/releases", "x-github-api-version")
            .as_deref(),
        Some("2022-11-28")
    );
    assert!(stub
        .header("/repos/o/r/releases", "authorization")
        .is_none());
}
