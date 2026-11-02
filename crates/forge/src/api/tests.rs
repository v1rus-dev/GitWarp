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
                let path = path.split('?').next().unwrap().to_string();
                log.lock().unwrap().push((path.clone(), headers));
                let (status, extra, body) = routes
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, c)| c.clone())
                    .unwrap_or((404, vec![], ""));
                let mut out = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in extra {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                out.push_str(body);
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
