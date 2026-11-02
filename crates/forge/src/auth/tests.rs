//! Tests of the device flow, the refresh and the secret format against a local
//! HTTP stub. No real network, no real tokens.

use super::*;
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// One request the stub saw: path and decoded form fields.
type Seen = Arc<Mutex<Vec<(String, HashMap<String, String>)>>>;

/// Minimal HTTP/1.1 stub: answers every POST with `(status, body)` computed by
/// `handler` from the path and the form fields, and records the requests.
struct Stub {
    base: String,
    seen: Seen,
}

impl Stub {
    fn start(
        handler: impl Fn(&str, &HashMap<String, String>) -> (u16, String) + Send + 'static,
    ) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen: Seen = Arc::default();
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    let header = header.trim_end();
                    if header.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let form: HashMap<String, String> = String::from_utf8_lossy(&body)
                    .split('&')
                    .filter_map(|pair| pair.split_once('='))
                    .map(|(k, v)| (k.to_owned(), v.replace("%3A", ":")))
                    .collect();
                log.lock().unwrap().push((path.clone(), form.clone()));
                let (status, text) = handler(&path, &form);
                let out = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = reader.get_mut().write_all(out.as_bytes());
            }
        });
        Stub { base, seen }
    }

    /// A stub that answers every request the same.
    fn fixed(status: u16, body: serde_json::Value) -> Stub {
        let text = body.to_string();
        Stub::start(move |_, _| (status, text.clone()))
    }

    fn requests(&self) -> Vec<(String, HashMap<String, String>)> {
        self.seen.lock().unwrap().clone()
    }
}

fn client() -> Client {
    Client::for_tests()
}

fn tokens() -> OAuthTokens {
    OAuthTokens {
        access: "ghu_access".into(),
        refresh: "ghr_refresh".into(),
        access_expires_at: 1_000 + 28_800,
        refresh_expires_at: 1_000 + 15_638_400,
    }
}

// ---- secret format ----

#[test]
fn secret_round_trips() {
    let secret = tokens().to_secret();
    assert!(secret.starts_with('{'));
    assert_eq!(parse_secret(&secret), StoredSecret::OAuth(tokens()));
}

#[test]
fn secret_json_has_the_documented_keys() {
    let value: serde_json::Value = serde_json::from_str(&tokens().to_secret()).unwrap();
    assert_eq!(
        value,
        json!({"v": 1, "access": "ghu_access", "refresh": "ghr_refresh",
               "access_exp": 29_800, "refresh_exp": 15_639_400})
    );
}

#[test]
fn raw_string_is_a_personal_access_token() {
    for raw in ["ghp_abc123", "glpat-xyz", " ghp_padded "] {
        assert_eq!(
            parse_secret(raw),
            StoredSecret::Pat(raw.to_owned()),
            "{raw:?}"
        );
    }
}

#[test]
fn damaged_or_foreign_json_secret_is_unusable() {
    for secret in [
        "{",
        "{}",
        r#"{"v":2,"access":"a","refresh":"r","access_exp":1,"refresh_exp":2}"#,
        r#"{"v":1,"access":"","refresh":"r","access_exp":1,"refresh_exp":2}"#,
        r#"{"v":1,"access":"a"}"#,
    ] {
        assert_eq!(parse_secret(secret), StoredSecret::Unusable, "{secret}");
    }
}

#[test]
fn debug_output_hides_secrets() {
    let debug = format!(
        "{:?} {:?} {:?}",
        tokens(),
        StoredSecret::OAuth(tokens()),
        StoredSecret::Pat("ghp_secret".into())
    );
    for secret in ["ghu_access", "ghr_refresh", "ghp_secret"] {
        assert!(!debug.contains(secret), "{debug}");
    }
}

// ---- device start ----

const START_BODY: &str = r#"{"device_code":"dc-secret","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":899,"interval":5}"#;

#[test]
fn device_start_parses_the_answer_and_sends_client_and_scope() {
    let stub = Stub::start(|_, _| (200, START_BODY.to_owned()));
    let code = github_device_start(&client(), &stub.base).unwrap();
    assert_eq!(code.device_code(), "dc-secret");
    assert_eq!(code.user_code, "WDJB-MJHT");
    assert_eq!(code.verification_uri, "https://github.com/login/device");
    assert_eq!(code.expires_in, 899);
    assert_eq!(code.interval, 5);
    let requests = stub.requests();
    assert_eq!(requests[0].0, "/login/device/code");
    assert_eq!(requests[0].1["client_id"], GITHUB_CLIENT_ID);
    // Form encoding turns the spaces of the scope list into `+` or `%20`.
    assert_eq!(
        requests[0].1["scope"].replace('+', " ").replace("%20", " "),
        "repo read:user workflow"
    );
}

#[test]
fn device_code_debug_hides_the_device_code_but_shows_the_user_code() {
    let stub = Stub::start(|_, _| (200, START_BODY.to_owned()));
    let code = github_device_start(&client(), &stub.base).unwrap();
    let debug = format!("{code:?}");
    assert!(!debug.contains("dc-secret"));
    assert!(debug.contains("WDJB-MJHT"));
}

#[test]
fn device_start_never_uses_a_non_https_verification_page() {
    let stub = Stub::fixed(
        200,
        json!({"device_code":"d","user_code":"U","verification_uri":"http://evil.example/x",
               "expires_in":10,"interval":0}),
    );
    let code = github_device_start(&client(), &stub.base).unwrap();
    assert_eq!(code.verification_uri, format!("{}/login/device", stub.base));
    assert_eq!(code.interval, 1, "a zero interval would poll in a loop");
}

#[test]
fn device_start_errors() {
    let stub = Stub::fixed(200, json!({"error": "device_flow_disabled"}));
    assert_eq!(
        github_device_start(&client(), &stub.base),
        Err(AuthError::Rejected("device_flow_disabled".into()))
    );
    let stub = Stub::fixed(500, json!({}));
    assert!(matches!(
        github_device_start(&client(), &stub.base),
        Err(AuthError::Unexpected(_))
    ));
    let stub = Stub::fixed(200, json!({"user_code": "U"}));
    assert!(matches!(
        github_device_start(&client(), &stub.base),
        Err(AuthError::Unexpected(_))
    ));
}

#[test]
fn unreachable_service_is_a_network_error() {
    // Nothing listens on a freshly closed port.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let result = github_device_start(&client(), &format!("http://127.0.0.1:{port}"));
    assert!(matches!(result, Err(AuthError::Network(_))), "{result:?}");
}

// ---- device poll ----

fn poll(stub: &Stub) -> Result<PollStep, AuthError> {
    github_device_poll_at(&client(), &stub.base, "dc-secret", 1_000)
}

#[test]
fn poll_maps_every_documented_error() {
    for (code, step) in [
        ("authorization_pending", PollStep::Pending),
        ("slow_down", PollStep::SlowDown),
        ("access_denied", PollStep::Denied),
        ("expired_token", PollStep::Expired),
    ] {
        // GitHub answers these with HTTP 200 and an `error` field.
        let stub = Stub::fixed(200, json!({"error": code}));
        assert_eq!(poll(&stub), Ok(step), "{code}");
    }
}

#[test]
fn poll_other_error_codes_are_rejections() {
    let stub = Stub::fixed(200, json!({"error": "incorrect_device_code"}));
    assert_eq!(
        poll(&stub),
        Err(AuthError::Rejected("incorrect_device_code".into()))
    );
}

#[test]
fn poll_success_yields_tokens_with_absolute_expiry() {
    let stub = Stub::fixed(
        200,
        json!({"access_token":"ghu_access","expires_in":28800,"refresh_token":"ghr_refresh",
               "refresh_token_expires_in":15638400,"scope":"repo,workflow","token_type":"bearer"}),
    );
    assert_eq!(poll(&stub), Ok(PollStep::Done(tokens())));
    let (path, form) = &stub.requests()[0];
    assert_eq!(path, "/login/oauth/access_token");
    assert_eq!(form["client_id"], GITHUB_CLIENT_ID);
    assert_eq!(form["device_code"], "dc-secret");
    assert_eq!(
        form["grant_type"],
        "urn:ietf:params:oauth:grant-type:device_code"
    );
}

#[test]
fn poll_success_without_expiry_is_unexpected() {
    let stub = Stub::fixed(200, json!({"access_token": "ghu_access"}));
    assert!(matches!(poll(&stub), Err(AuthError::Unexpected(_))));
}

#[test]
fn poll_server_error_is_unexpected() {
    let stub = Stub::fixed(502, json!({}));
    assert!(matches!(poll(&stub), Err(AuthError::Unexpected(_))));
}

// ---- refresh ----

#[test]
fn refresh_rotates_both_tokens() {
    let stub = Stub::fixed(
        200,
        json!({"access_token":"ghu_new","expires_in":28800,"refresh_token":"ghr_new",
               "refresh_token_expires_in":15638400,"token_type":"bearer"}),
    );
    let new = github_refresh_at(&client(), &stub.base, "ghr_old", 2_000).unwrap();
    assert_eq!(new.access, "ghu_new");
    assert_eq!(new.refresh, "ghr_new");
    assert_eq!(new.access_expires_at, 2_000 + 28_800);
    assert_eq!(new.refresh_expires_at, 2_000 + 15_638_400);
    let (path, form) = &stub.requests()[0];
    assert_eq!(path, "/login/oauth/access_token");
    assert_eq!(form["grant_type"], "refresh_token");
    assert_eq!(form["refresh_token"], "ghr_old");
    assert_eq!(form["client_id"], GITHUB_CLIENT_ID);
    assert!(!form.contains_key("client_secret"));
}

#[test]
fn refresh_with_a_bad_token_is_rejected() {
    let stub = Stub::fixed(
        200,
        json!({"error":"bad_refresh_token","error_description":"The refresh token passed is incorrect or expired."}),
    );
    assert_eq!(
        github_refresh_at(&client(), &stub.base, "ghr_old", 2_000),
        Err(AuthError::Rejected("bad_refresh_token".into()))
    );
}

#[test]
fn refresh_server_error_is_unexpected_not_a_rejection() {
    let stub = Stub::fixed(503, json!({}));
    assert!(matches!(
        github_refresh_at(&client(), &stub.base, "ghr_old", 2_000),
        Err(AuthError::Unexpected(_))
    ));
}

#[test]
fn errors_do_not_leak_tokens() {
    let stub = Stub::fixed(500, json!({"echo": "ghr_old"}));
    let text = github_refresh_at(&client(), &stub.base, "ghr_old", 2_000)
        .unwrap_err()
        .to_string();
    assert!(!text.contains("ghr_old"), "{text}");
}

#[test]
fn huge_lifetimes_do_not_overflow() {
    let body = json!({"access_token":"a","refresh_token":"r","expires_in":u64::MAX,
                      "refresh_token_expires_in":u64::MAX});
    let tokens = tokens_from(&body, 1_000).unwrap();
    assert_eq!(tokens.access_expires_at, u64::MAX);
    assert_eq!(tokens.refresh_expires_at, u64::MAX);
}
