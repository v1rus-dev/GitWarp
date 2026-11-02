//! Tests of the pull request / CI / create-PR calls: recorded JSON shapes
//! (from the services' REST docs), request building and a local HTTP stub.
//! No real network.

use super::checks::{aggregate, github_run_state, gitlab_state, parse_github_runs, status_state};
use super::net::{cache_key, parse_rfc3339};
use super::pulls::{
    branch_url, list_url, parse_gitea, parse_github, parse_gitlab, review_from_list,
};
use super::*;
use crate::{Instance, RepoCoords, ServiceKind};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------------ stub

#[derive(Clone, Debug)]
struct Req {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: String,
}

struct Resp {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn resp(status: u16, body: &str) -> Resp {
    Resp {
        status,
        headers: vec![],
        body: body.to_string(),
    }
}

struct Stub {
    base: String,
    seen: Arc<Mutex<Vec<Req>>>,
}

impl Stub {
    fn start(handler: impl Fn(&Req) -> Resp + Send + 'static) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Req>>> = Arc::default();
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let target = parts.next().unwrap_or("/").to_string();
                let mut headers = HashMap::new();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                    }
                }
                let len: usize = headers
                    .get("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let req = Req {
                    method,
                    target,
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                };
                log.lock().unwrap().push(req.clone());
                let r = handler(&req);
                let mut out = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    r.status,
                    r.body.len()
                );
                for (k, v) in r.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                out.push_str(&r.body);
                let _ = reader.get_mut().write_all(out.as_bytes());
            }
        });
        Stub { base, seen }
    }

    fn requests(&self) -> Vec<Req> {
        self.seen.lock().unwrap().clone()
    }

    fn targets(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|r| format!("{} {}", r.method, r.target))
            .collect()
    }
}

fn coords() -> RepoCoords {
    RepoCoords {
        namespace: vec!["octo".into()],
        repo: "demo".into(),
    }
}

fn inst(kind: ServiceKind, stub: &Stub) -> Instance {
    Instance::new(kind, &stub.base).unwrap()
}

fn tok(kind: TokenKind) -> Auth<'static> {
    Auth::new(Some("sekret"), kind)
}

// ------------------------------------------------------------------ fixtures

const GH_LIST: &str = r#"[
 {"number":7,"title":"Add thing","state":"open","draft":false,"merged_at":null,
  "updated_at":"2024-05-02T10:00:00Z","html_url":"https://github.com/octo/demo/pull/7",
  "user":{"login":"alice"},"requested_reviewers":[{"login":"bob"}],
  "head":{"ref":"feature/a#1","repo":{"full_name":"octo/demo"}},"base":{"ref":"main"}},
 {"number":6,"title":"WIP thing","state":"open","draft":true,"merged_at":null,
  "updated_at":"2024-05-01T10:00:00Z","html_url":"https://github.com/octo/demo/pull/6",
  "user":{"login":"carol"},"requested_reviewers":[],
  "head":{"ref":"wip","repo":{"full_name":"octo/demo"}},"base":{"ref":"main"}},
 {"number":5,"title":"Done","state":"closed","draft":false,"merged_at":"2024-04-30T09:00:00Z",
  "updated_at":"2024-04-30T09:00:00Z","html_url":"https://github.com/octo/demo/pull/5",
  "user":{"login":"alice"},"requested_reviewers":[],
  "head":{"ref":"feature/a#1","repo":{"full_name":"octo/demo"}},"base":{"ref":"main"}},
 {"number":4,"title":"Rejected","state":"closed","draft":false,"merged_at":null,
  "updated_at":"2024-04-29T09:00:00Z","html_url":"https://github.com/octo/demo/pull/4",
  "user":{"login":"dave"},"requested_reviewers":[],
  "head":{"ref":"main","repo":{"full_name":"dave/demo"}},"base":{"ref":"main"}}
]"#;

const GL_LIST: &str = r#"[
 {"iid":12,"title":"Draft: Work","state":"opened","draft":true,"merged_at":null,
  "updated_at":"2024-05-02T10:00:00.123+02:00","web_url":"https://gitlab.com/octo/demo/-/merge_requests/12",
  "author":{"username":"alice"},"source_branch":"feature/x","target_branch":"main",
  "source_project_id":1,"target_project_id":1},
 {"iid":11,"title":"Merged","state":"merged","draft":false,
  "updated_at":"2024-05-01T10:00:00Z","web_url":"https://gitlab.com/octo/demo/-/merge_requests/11",
  "author":{"username":"bob"},"source_branch":"feature/x","target_branch":"main",
  "source_project_id":1,"target_project_id":1},
 {"iid":10,"title":"Closed","state":"closed","draft":false,
  "updated_at":"2024-04-01T10:00:00Z","web_url":"https://gitlab.com/octo/demo/-/merge_requests/10",
  "author":{"username":"bob"},"source_branch":"fork","target_branch":"main",
  "source_project_id":9,"target_project_id":1},
 {"iid":9,"title":"Opened","state":"opened","draft":false,
  "updated_at":"2024-03-01T10:00:00Z","web_url":"https://gitlab.com/octo/demo/-/merge_requests/9",
  "author":{"username":"bob"},"source_branch":"b","target_branch":"main",
  "source_project_id":1,"target_project_id":1}
]"#;

const GITEA_LIST: &str = r#"[
 {"number":3,"title":"Add","state":"open","draft":false,"merged":false,
  "updated_at":"2024-05-02T10:00:00Z","html_url":"https://codeberg.org/octo/demo/pulls/3",
  "user":{"login":"alice"},"requested_reviewers":[{"login":"bob"}],
  "head":{"ref":"feat","repo":{"full_name":"octo/demo"}},"base":{"ref":"main"}},
 {"number":2,"title":"WIP: later","state":"open","draft":false,"merged":false,
  "updated_at":"2024-05-01T10:00:00Z","html_url":"https://codeberg.org/octo/demo/pulls/2",
  "user":{"login":"bob"},"requested_reviewers":[],
  "head":{"ref":"later","repo":{"full_name":"octo/demo"}},"base":{"ref":"main"}},
 {"number":1,"title":"Old","state":"closed","draft":false,"merged":true,
  "updated_at":"2024-04-01T10:00:00Z","html_url":"https://codeberg.org/octo/demo/pulls/1",
  "user":{"login":"bob"},"requested_reviewers":null,
  "head":{"ref":"old","repo":null},"base":{"ref":"main"}}
]"#;

fn arr(s: &str) -> Vec<Value> {
    serde_json::from_str(s).unwrap()
}

// ------------------------------------------------------------------ time

#[test]
fn rfc3339_parses_z_fraction_and_offsets() {
    assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_rfc3339("2024-05-01T12:34:56Z"), Some(1_714_566_896));
    assert_eq!(
        parse_rfc3339("2024-05-01T12:34:56.789Z"),
        Some(1_714_566_896)
    );
    assert_eq!(
        parse_rfc3339("2024-05-01T14:34:56+02:00"),
        Some(1_714_566_896)
    );
    assert_eq!(
        parse_rfc3339("2024-05-01T07:34:56-05:00"),
        Some(1_714_566_896)
    );
    assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    assert_eq!(parse_rfc3339("garbage"), None);
    assert_eq!(parse_rfc3339("2024-13-01T00:00:00Z"), None);
}

// ------------------------------------------------------------------ parsers

#[test]
fn github_pull_requests_states() {
    let list = arr(GH_LIST);
    let prs: Vec<_> = list.iter().map(|v| parse_github(v).unwrap()).collect();
    assert_eq!(prs[0].state, PrState::Open);
    assert_eq!(prs[0].number, 7);
    assert_eq!(prs[0].author, "alice");
    assert_eq!(prs[0].head_branch, "feature/a#1");
    assert_eq!(prs[0].head_repo_full_name.as_deref(), Some("octo/demo"));
    assert_eq!(prs[0].base_branch, "main");
    assert_eq!(prs[0].updated_at, 1_714_644_000);
    assert_eq!(prs[0].web_url, "https://github.com/octo/demo/pull/7");
    assert_eq!(prs[0].review, ReviewState::None);
    assert_eq!(prs[1].state, PrState::Draft);
    assert_eq!(prs[2].state, PrState::Merged);
    assert_eq!(prs[3].state, PrState::Closed);
    assert_eq!(prs[3].head_repo_full_name.as_deref(), Some("dave/demo"));
}

#[test]
fn gitlab_merge_requests_states() {
    let list = arr(GL_LIST);
    let prs: Vec<_> = list
        .iter()
        .map(|v| parse_gitlab(v, &coords()).unwrap())
        .collect();
    assert_eq!(prs[0].state, PrState::Draft);
    assert_eq!(prs[0].number, 12);
    assert_eq!(prs[0].head_repo_full_name.as_deref(), Some("octo/demo"));
    // 12:34 at +02:00 offset input: 2024-05-02T08:00:00.123Z
    assert_eq!(prs[0].updated_at, 1_714_636_800);
    assert_eq!(prs[1].state, PrState::Merged);
    assert_eq!(prs[2].state, PrState::Closed);
    assert_eq!(
        prs[2].head_repo_full_name, None,
        "fork: head project path unknown"
    );
    assert_eq!(prs[3].state, PrState::Open);
    assert_eq!(prs[3].author, "bob");
}

#[test]
fn gitea_pull_requests_states() {
    let list = arr(GITEA_LIST);
    let prs: Vec<_> = list.iter().map(|v| parse_gitea(v).unwrap()).collect();
    assert_eq!(prs[0].state, PrState::Open);
    assert_eq!(prs[1].state, PrState::Draft, "WIP: prefix marks a draft");
    assert_eq!(prs[2].state, PrState::Merged);
    assert_eq!(prs[2].head_repo_full_name, None, "deleted fork");
    let draft = json!({"number":1,"title":"x","state":"open","draft":true,"head":{"ref":"a"},
        "base":{"ref":"b"},"html_url":"u"});
    assert_eq!(parse_gitea(&draft).unwrap().state, PrState::Draft);
}

#[test]
fn unparsable_items_are_skipped() {
    let raw = vec![json!({"nope": 1})];
    assert!(parse_list_for(ServiceKind::GitHub, &raw).is_empty());
}

fn parse_list_for(kind: ServiceKind, raw: &[Value]) -> Vec<PullRequest> {
    super::pulls::parse_list(kind, &coords(), raw)
}

#[test]
fn review_state_from_reviews() {
    let r = |user: &str, state: &str| json!({"user":{"login":user},"state":state});
    assert_eq!(review_from_list(&[]), ReviewState::None);
    assert_eq!(
        review_from_list(&[r("a", "COMMENTED"), r("b", "PENDING")]),
        ReviewState::None
    );
    assert_eq!(
        review_from_list(&[r("a", "APPROVED")]),
        ReviewState::Approved
    );
    assert_eq!(
        review_from_list(&[r("a", "APPROVED"), r("b", "CHANGES_REQUESTED")]),
        ReviewState::ChangesRequested
    );
    // the same user changed their mind: the latest decision counts
    assert_eq!(
        review_from_list(&[r("a", "CHANGES_REQUESTED"), r("a", "APPROVED")]),
        ReviewState::Approved
    );
    // a comment does not retract a decision
    assert_eq!(
        review_from_list(&[r("a", "APPROVED"), r("a", "COMMENTED")]),
        ReviewState::Approved
    );
    // Gitea spelling and dismissed reviews
    assert_eq!(
        review_from_list(&[r("a", "REQUEST_CHANGES")]),
        ReviewState::ChangesRequested
    );
    let dismissed = json!({"user":{"login":"a"},"state":"REQUEST_CHANGES","dismissed":true});
    assert_eq!(review_from_list(&[dismissed]), ReviewState::None);
}

// ------------------------------------------------------------------ state maps

#[test]
fn github_run_state_mapping() {
    use CiState::{Cancelled, Failed, Passed, Pending, Running};
    for (status, conclusion, expected) in [
        ("queued", None, Pending),
        ("waiting", None, Pending),
        ("requested", None, Pending),
        ("pending", None, Pending),
        ("in_progress", None, Running),
        ("completed", Some("success"), Passed),
        ("completed", Some("neutral"), Passed),
        ("completed", Some("skipped"), Passed),
        ("completed", Some("failure"), Failed),
        ("completed", Some("timed_out"), Failed),
        ("completed", Some("action_required"), Failed),
        ("completed", Some("cancelled"), Cancelled),
        ("completed", None, Pending),
        ("something_new", None, Pending),
    ] {
        assert_eq!(
            github_run_state(status, conclusion),
            expected,
            "{status} {conclusion:?}"
        );
    }
}

#[test]
fn status_and_gitlab_state_mapping() {
    use CiState::*;
    for (s, e) in [
        ("pending", Pending),
        ("success", Passed),
        ("warning", Passed),
        ("failure", Failed),
        ("error", Failed),
        ("new", Pending),
    ] {
        assert_eq!(status_state(s), e, "{s}");
    }
    for (s, e) in [
        ("created", Pending),
        ("waiting_for_resource", Pending),
        ("preparing", Pending),
        ("waiting_for_callback", Pending),
        ("pending", Pending),
        ("scheduled", Pending),
        ("manual", Pending),
        ("running", Running),
        ("canceling", Running),
        ("success", Passed),
        ("failed", Failed),
        ("canceled", Cancelled),
        ("skipped", Cancelled),
    ] {
        assert_eq!(gitlab_state(s), e, "{s}");
    }
}

#[test]
fn overall_aggregation() {
    use CiState::*;
    let agg = |v: &[CiState]| aggregate(v.iter().copied());
    assert_eq!(agg(&[]), None);
    assert_eq!(agg(&[Passed, Passed]), Passed);
    assert_eq!(agg(&[Passed, Failed, Running]), Failed);
    assert_eq!(agg(&[Passed, Running, Pending]), Running);
    assert_eq!(agg(&[Passed, Pending]), Pending);
    assert_eq!(agg(&[Passed, Cancelled]), Cancelled);
}

#[test]
fn github_check_runs_fixture() {
    let v = json!({"total_count":2,"check_runs":[
        {"name":"build","status":"completed","conclusion":"success",
         "started_at":"2024-05-01T10:00:00Z","completed_at":"2024-05-01T10:02:05Z",
         "html_url":"https://github.com/octo/demo/runs/1"},
        {"name":"lint","status":"in_progress","conclusion":null,
         "started_at":"2024-05-01T10:00:00Z","completed_at":null,"html_url":"https://x/2"}]});
    let checks = parse_github_runs(&v);
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].name, "build");
    assert_eq!(checks[0].state, CiState::Passed);
    assert_eq!(checks[0].duration_secs, Some(125));
    assert_eq!(checks[1].state, CiState::Running);
    assert_eq!(checks[1].duration_secs, None);
}

// ------------------------------------------------------------------ request building

#[test]
fn list_urls_per_service() {
    let gh = Instance::new(ServiceKind::GitHub, "https://github.com").unwrap();
    assert_eq!(
        list_url(&gh, &coords(), PrStateFilter::Open, &PrFilter::All, 2, 50).unwrap(),
        "https://api.github.com/repos/octo/demo/pulls?state=open&sort=updated&direction=desc&per_page=50&page=2"
    );
    let gl = Instance::new(ServiceKind::GitLab, "https://gitlab.com").unwrap();
    let nested = RepoCoords {
        namespace: vec!["grp".into(), "sub".into()],
        repo: "r".into(),
    };
    assert_eq!(
        list_url(
            &gl,
            &nested,
            PrStateFilter::All,
            &PrFilter::Mine { login: "al ice".into() },
            1,
            50
        )
        .unwrap(),
        "https://gitlab.com/api/v4/projects/grp%2Fsub%2Fr/merge_requests?state=all&order_by=updated_at&sort=desc&per_page=50&page=1&author_username=al%20ice"
    );
    assert!(list_url(
        &gl,
        &coords(),
        PrStateFilter::Open,
        &PrFilter::ReviewRequested {
            login: "bob".into()
        },
        1,
        50
    )
    .unwrap()
    .ends_with(
        "state=opened&order_by=updated_at&sort=desc&per_page=50&page=1&reviewer_username=bob"
    ));
    let gt = Instance::new(ServiceKind::Gitea, "https://codeberg.org").unwrap();
    assert_eq!(
        list_url(&gt, &coords(), PrStateFilter::All, &PrFilter::All, 3, 50).unwrap(),
        "https://codeberg.org/api/v1/repos/octo/demo/pulls?state=all&sort=recentupdate&limit=50&page=3"
    );
}

#[test]
fn branch_names_with_slash_and_hash_are_encoded() {
    assert_eq!(
        branch_url("https://gl/api/v4/projects/1", "feature/a#1"),
        "https://gl/api/v4/projects/1/merge_requests?source_branch=feature%2Fa%231&state=all&order_by=updated_at&sort=desc&per_page=20"
    );
}

#[test]
fn auth_headers_per_service() {
    let h = |kind, auth: Auth| {
        auth.headers(kind)
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<HashMap<_, _>>()
    };
    let gh = h(ServiceKind::GitHub, tok(TokenKind::Pat));
    assert_eq!(gh["Authorization"], "Bearer sekret");
    assert_eq!(gh["Accept"], "application/vnd.github+json");
    assert_eq!(gh["X-GitHub-Api-Version"], "2022-11-28");
    let gl = h(ServiceKind::GitLab, tok(TokenKind::Pat));
    assert_eq!(gl["PRIVATE-TOKEN"], "sekret");
    assert!(!gl.contains_key("Authorization"));
    let gl = h(ServiceKind::GitLab, tok(TokenKind::OAuth));
    assert_eq!(gl["Authorization"], "Bearer sekret");
    let gt = h(ServiceKind::Gitea, tok(TokenKind::Pat));
    assert_eq!(gt["Authorization"], "token sekret");
    let anon = h(ServiceKind::GitHub, Auth::anonymous());
    assert!(!anon.contains_key("Authorization"));
    assert!(
        !h(ServiceKind::GitHub, Auth::new(Some(""), TokenKind::Pat)).contains_key("Authorization")
    );
}

#[test]
fn auth_debug_hides_the_token() {
    assert!(!format!("{:?}", tok(TokenKind::Pat)).contains("sekret"));
    assert!(!format!("{:?}", Client::for_tests()).contains("sekret"));
}

// ------------------------------------------------------------------ list over HTTP

fn list_stub(body: &'static str, extra: Vec<(&'static str, String)>) -> Stub {
    Stub::start(move |_| Resp {
        status: 200,
        headers: extra.clone(),
        body: body.to_string(),
    })
}

#[test]
fn github_list_sends_bearer_filters_mine_and_reads_link() {
    let stub = list_stub(
        GH_LIST,
        vec![(
            "Link",
            "<https://x/next>; rel=\"next\", <https://x/last>; rel=\"last\"".into(),
        )],
    );
    let page = list_pull_requests(
        &Client::for_tests(),
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &PrFilter::Mine {
            login: "Alice".into(),
        },
        PrStateFilter::All,
        1,
    )
    .unwrap();
    assert!(page.has_next);
    assert_eq!(
        page.items.iter().map(|p| p.number).collect::<Vec<_>>(),
        [7, 5]
    );
    let req = &stub.requests()[0];
    assert!(req
        .target
        .starts_with("/api/v3/repos/octo/demo/pulls?state=all"));
    assert_eq!(req.headers["authorization"], "Bearer sekret");
}

#[test]
fn github_list_review_requested_filter_and_anonymous() {
    let stub = list_stub(GH_LIST, vec![]);
    let page = list_pull_requests(
        &Client::for_tests(),
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &Auth::anonymous(),
        &PrFilter::ReviewRequested {
            login: "bob".into(),
        },
        PrStateFilter::Open,
        1,
    )
    .unwrap();
    assert!(!page.has_next);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].number, 7);
    assert!(!stub.requests()[0].headers.contains_key("authorization"));
}

#[test]
fn gitlab_list_uses_private_token_and_server_side_filter() {
    let stub = list_stub(GL_LIST, vec![("X-Next-Page", "2".into())]);
    let page = list_pull_requests(
        &Client::for_tests(),
        &inst(ServiceKind::GitLab, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &PrFilter::Mine {
            login: "alice".into(),
        },
        PrStateFilter::Open,
        1,
    )
    .unwrap();
    assert!(page.has_next);
    assert_eq!(
        page.items.len(),
        4,
        "GitLab filters on the server, not on the client"
    );
    let req = &stub.requests()[0];
    assert!(req
        .target
        .starts_with("/api/v4/projects/octo%2Fdemo/merge_requests?state=opened"));
    assert!(req.target.contains("author_username=alice"));
    assert_eq!(req.headers["private-token"], "sekret");
}

#[test]
fn gitea_list_uses_token_scheme() {
    let stub = list_stub(GITEA_LIST, vec![]);
    let page = list_pull_requests(
        &Client::for_tests(),
        &inst(ServiceKind::Gitea, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &PrFilter::All,
        PrStateFilter::All,
        1,
    )
    .unwrap();
    assert_eq!(page.items.len(), 3);
    assert_eq!(stub.requests()[0].headers["authorization"], "token sekret");
    assert!(stub.requests()[0]
        .target
        .starts_with("/api/v1/repos/octo/demo/pulls?state=all"));
}

#[test]
fn pull_requests_for_branches_github_picks_open_and_ignores_forks() {
    let stub = list_stub(GH_LIST, vec![]);
    let branches = vec![
        "feature/a#1".to_string(),
        "main".to_string(),
        "none".to_string(),
    ];
    let map = pull_requests_for_branches(
        &Client::for_tests(),
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &Auth::anonymous(),
        &branches,
    )
    .unwrap();
    assert_eq!(stub.requests().len(), 1, "one list call");
    assert_eq!(
        map["feature/a#1"].number, 7,
        "open beats the older merged one"
    );
    assert!(
        !map.contains_key("main"),
        "a fork's `main` is not our `main`"
    );
    assert!(!map.contains_key("none"));
}

#[test]
fn pull_requests_for_branches_gitlab_one_request_per_branch() {
    let stub = Stub::start(|req| {
        if req.target.contains("source_branch=feature%2Fx") {
            resp(200, GL_LIST)
        } else {
            resp(200, "[]")
        }
    });
    let branches = vec!["feature/x".to_string(), "other#1".to_string()];
    let map = pull_requests_for_branches(
        &Client::for_tests(),
        &inst(ServiceKind::GitLab, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &branches,
    )
    .unwrap();
    let targets = stub.targets();
    assert_eq!(targets.len(), 2);
    assert!(targets[0].contains("source_branch=feature%2Fx&"));
    assert!(targets[1].contains("source_branch=other%231&"));
    assert_eq!(map["feature/x"].number, 12, "open draft beats merged");
    assert_eq!(map["feature/x"].state, PrState::Draft);
    assert_eq!(map.len(), 1);
}

#[test]
fn pull_requests_for_branches_gitea_prefers_latest_finished_when_none_open() {
    let body = r#"[
     {"number":2,"title":"b","state":"closed","merged":true,"updated_at":"2024-05-01T00:00:00Z","html_url":"u2","user":{"login":"a"},"head":{"ref":"f","repo":{"full_name":"octo/demo"}},"base":{"ref":"main"}},
     {"number":1,"title":"a","state":"closed","merged":false,"updated_at":"2024-04-01T00:00:00Z","html_url":"u1","user":{"login":"a"},"head":{"ref":"f","repo":{"full_name":"octo/demo"}},"base":{"ref":"main"}}]"#;
    let stub = list_stub(Box::leak(body.to_string().into_boxed_str()), vec![]);
    let map = pull_requests_for_branches(
        &Client::for_tests(),
        &inst(ServiceKind::Gitea, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &["f".to_string()],
    )
    .unwrap();
    assert_eq!(map["f"].number, 2);
    assert_eq!(map["f"].state, PrState::Merged);
}

#[test]
fn review_over_http() {
    let stub = Stub::start(|req| {
        if req.target.contains("/approvals") {
            resp(200, r#"{"approved":true,"approved_by":[]}"#)
        } else {
            resp(
                200,
                r#"[{"user":{"login":"a"},"state":"CHANGES_REQUESTED"}]"#,
            )
        }
    });
    let c = Client::for_tests();
    let gl = pull_request_review(
        &c,
        &inst(ServiceKind::GitLab, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        3,
    )
    .unwrap();
    assert_eq!(gl, ReviewState::Approved);
    let gh = pull_request_review(
        &c,
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        3,
    )
    .unwrap();
    assert_eq!(gh, ReviewState::ChangesRequested);
    assert!(stub.targets()[0].contains("/merge_requests/3/approvals"));
    assert!(stub.targets()[1].contains("/pulls/3/reviews"));
}

// ------------------------------------------------------------------ checks over HTTP

#[test]
fn github_commit_checks_combine_runs_and_statuses() {
    let stub = Stub::start(|req| {
        if req.target.contains("/check-runs") {
            resp(
                200,
                r#"{"total_count":1,"check_runs":[{"name":"build","status":"completed","conclusion":"success","started_at":"2024-05-01T10:00:00Z","completed_at":"2024-05-01T10:01:00Z","html_url":"https://r/1"}]}"#,
            )
        } else {
            resp(
                200,
                r#"{"state":"failure","statuses":[{"context":"ci/legacy","state":"failure","target_url":"https://legacy/1"}]}"#,
            )
        }
    });
    let out = commit_checks(
        &Client::for_tests(),
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        "abc123",
    )
    .unwrap();
    assert_eq!(out.overall, CiState::Failed);
    assert_eq!(out.checks.len(), 2);
    assert_eq!(out.checks[0].duration_secs, Some(60));
    assert_eq!(out.checks[1].name, "ci/legacy");
    assert_eq!(out.checks[1].web_url.as_deref(), Some("https://legacy/1"));
    let t = stub.targets();
    assert!(t[0].starts_with("GET /api/v3/repos/octo/demo/commits/abc123/check-runs"));
    assert!(t[1].starts_with("GET /api/v3/repos/octo/demo/commits/abc123/status"));
}

#[test]
fn github_commit_without_checks_is_none() {
    // GitHub reports a combined `pending` for a commit with no statuses.
    let stub = Stub::start(|req| {
        if req.target.contains("/check-runs") {
            resp(200, r#"{"total_count":0,"check_runs":[]}"#)
        } else {
            resp(200, r#"{"state":"pending","statuses":[],"total_count":0}"#)
        }
    });
    let out = commit_checks(
        &Client::for_tests(),
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &Auth::anonymous(),
        "abc",
    )
    .unwrap();
    assert_eq!(
        out,
        CommitChecks {
            overall: CiState::None,
            checks: vec![]
        }
    );
}

#[test]
fn gitlab_commit_checks_use_latest_pipeline_and_its_jobs() {
    let stub = Stub::start(|req| {
        if req.target.contains("/pipelines/77/jobs") {
            resp(
                200,
                r#"[{"name":"rspec","status":"failed","duration":12.6,"web_url":"https://gl/j/1"},
                    {"name":"lint","status":"success","duration":null,"web_url":"https://gl/j/2"}]"#,
            )
        } else {
            resp(
                200,
                r#"[{"id":77,"status":"failed","sha":"abc","web_url":"https://gl/p/77"}]"#,
            )
        }
    });
    let out = commit_checks(
        &Client::for_tests(),
        &inst(ServiceKind::GitLab, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        "abc",
    )
    .unwrap();
    assert_eq!(out.overall, CiState::Failed);
    assert_eq!(out.checks[0].duration_secs, Some(13));
    assert_eq!(out.checks[1].duration_secs, None);
    assert_eq!(out.checks[1].state, CiState::Passed);
    assert!(stub.targets()[0].contains("/pipelines?sha=abc&"));
}

#[test]
fn gitlab_commit_without_pipeline_is_none() {
    let stub = list_stub("[]", vec![]);
    let out = commit_checks(
        &Client::for_tests(),
        &inst(ServiceKind::GitLab, &stub),
        &coords(),
        &Auth::anonymous(),
        "abc",
    )
    .unwrap();
    assert_eq!(out.overall, CiState::None);
    assert_eq!(stub.requests().len(), 1);
}

#[test]
fn gitea_commit_checks_read_status_field() {
    let stub = list_stub(
        r#"{"state":"success","statuses":[{"context":"woodpecker","status":"success","target_url":"https://ci/1"},{"context":"lint","status":"pending","target_url":""}]}"#,
        vec![],
    );
    let out = commit_checks(
        &Client::for_tests(),
        &inst(ServiceKind::Gitea, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        "abc",
    )
    .unwrap();
    assert_eq!(out.overall, CiState::Pending);
    assert_eq!(out.checks[0].state, CiState::Passed);
    assert_eq!(out.checks[1].web_url, None);
    assert!(stub.targets()[0].starts_with("GET /api/v1/repos/octo/demo/commits/abc/status"));
}

// ------------------------------------------------------------------ create

fn new_pr() -> NewPullRequest {
    NewPullRequest {
        head: "feature/a".into(),
        base: "main".into(),
        title: "Add a".into(),
        body: "Body".into(),
        draft: true,
        reviewers: vec!["bob".into()],
    }
}

const GH_CREATED: &str = r#"{"number":8,"title":"Add a","state":"open","draft":true,"merged_at":null,
  "updated_at":"2024-05-02T10:00:00Z","html_url":"https://github.com/octo/demo/pull/8",
  "user":{"login":"alice"},"head":{"ref":"feature/a","repo":{"full_name":"octo/demo"}},"base":{"ref":"main"}}"#;

fn body_of(req: &Req) -> Value {
    serde_json::from_str(&req.body).unwrap()
}

#[test]
fn github_create_sends_draft_and_requests_reviewers() {
    let stub = Stub::start(|req| {
        if req.target.ends_with("/requested_reviewers") {
            resp(201, "{}")
        } else {
            resp(201, GH_CREATED)
        }
    });
    let out = create_pull_request(
        &Client::for_tests(),
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &new_pr(),
    )
    .unwrap();
    assert_eq!(out.pull_request.number, 8);
    assert_eq!(out.pull_request.state, PrState::Draft);
    assert_eq!(out.reviewers_error, None);
    let reqs = stub.requests();
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(reqs[0].target, "/api/v3/repos/octo/demo/pulls");
    assert_eq!(reqs[0].headers["authorization"], "Bearer sekret");
    assert_eq!(
        body_of(&reqs[0]),
        json!({"title":"Add a","head":"feature/a","base":"main","body":"Body","draft":true})
    );
    assert_eq!(
        reqs[1].target,
        "/api/v3/repos/octo/demo/pulls/8/requested_reviewers"
    );
    assert_eq!(body_of(&reqs[1]), json!({"reviewers":["bob"]}));
}

#[test]
fn github_reviewer_failure_does_not_lose_the_created_request() {
    let stub = Stub::start(|req| {
        if req.target.ends_with("/requested_reviewers") {
            resp(
                422,
                r#"{"message":"Review cannot be requested from pull request author."}"#,
            )
        } else {
            resp(201, GH_CREATED)
        }
    });
    let out = create_pull_request(
        &Client::for_tests(),
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &new_pr(),
    )
    .unwrap();
    assert_eq!(out.pull_request.number, 8);
    assert!(matches!(out.reviewers_error, Some(ApiError::Rejected(m)) if m.contains("author")));
}

#[test]
fn github_422_already_exists_is_mapped_with_number_and_url() {
    let stub = Stub::start(|req| {
        if req.method == "POST" {
            resp(
                422,
                r#"{"message":"Validation Failed","errors":[{"resource":"PullRequest","code":"custom","message":"A pull request already exists for octo:feature/a."}]}"#,
            )
        } else {
            resp(200, &format!("[{GH_CREATED}]"))
        }
    });
    let err = create_pull_request(
        &Client::for_tests(),
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &new_pr(),
    )
    .unwrap_err();
    assert_eq!(
        err,
        ApiError::AlreadyExists {
            number: 8,
            web_url: "https://github.com/octo/demo/pull/8".into()
        }
    );
    assert!(stub.targets()[1].contains("state=open&head=octo%3Afeature%2Fa"));
}

#[test]
fn github_other_422_is_rejected_with_the_service_message() {
    let stub = Stub::start(|_| {
        resp(
            422,
            r#"{"message":"Validation Failed","errors":[{"message":"No commits between main and feature/a"}]}"#,
        )
    });
    let err = create_pull_request(
        &Client::for_tests(),
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &new_pr(),
    )
    .unwrap_err();
    match err {
        ApiError::Rejected(m) => assert!(m.contains("No commits between"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(stub.requests().len(), 1);
}

#[test]
fn create_needs_a_token() {
    let stub = Stub::start(|_| resp(201, GH_CREATED));
    let err = create_pull_request(
        &Client::for_tests(),
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &Auth::anonymous(),
        &new_pr(),
    )
    .unwrap_err();
    assert_eq!(err, ApiError::Unauthorized);
    assert!(stub.requests().is_empty());
}

const GL_CREATED: &str = r#"{"iid":13,"title":"Draft: Add a","state":"opened","draft":true,
  "updated_at":"2024-05-02T10:00:00Z","web_url":"https://gl/mr/13","author":{"username":"alice"},
  "source_branch":"feature/a","target_branch":"main","source_project_id":1,"target_project_id":1}"#;

#[test]
fn gitlab_create_prefixes_draft_and_resolves_reviewer_ids() {
    let stub = Stub::start(|req| {
        if req.target.starts_with("/api/v4/users?") {
            if req.target.ends_with("username=bob") {
                resp(200, r#"[{"id":42,"username":"bob"}]"#)
            } else {
                resp(200, "[]")
            }
        } else {
            resp(201, GL_CREATED)
        }
    });
    let mut new = new_pr();
    new.reviewers = vec!["bob".into(), "ghost".into()];
    let out = create_pull_request(
        &Client::for_tests(),
        &inst(ServiceKind::GitLab, &stub),
        &coords(),
        &tok(TokenKind::OAuth),
        &new,
    )
    .unwrap();
    assert_eq!(out.pull_request.number, 13);
    assert_eq!(out.pull_request.state, PrState::Draft);
    assert_eq!(
        out.reviewers_error,
        Some(ApiError::NotFound),
        "unknown reviewer reported"
    );
    let post = stub
        .requests()
        .into_iter()
        .find(|r| r.method == "POST")
        .unwrap();
    assert_eq!(post.target, "/api/v4/projects/octo%2Fdemo/merge_requests");
    assert_eq!(post.headers["authorization"], "Bearer sekret");
    assert_eq!(
        body_of(&post),
        json!({"title":"Draft: Add a","source_branch":"feature/a","target_branch":"main",
               "description":"Body","reviewer_ids":[42]})
    );
}

#[test]
fn gitlab_title_already_marked_draft_is_not_prefixed_twice() {
    let stub = Stub::start(|_| resp(201, GL_CREATED));
    let mut new = new_pr();
    new.title = "Draft: Add a".into();
    new.reviewers.clear();
    create_pull_request(
        &Client::for_tests(),
        &inst(ServiceKind::GitLab, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &new,
    )
    .unwrap();
    assert_eq!(body_of(&stub.requests()[0])["title"], "Draft: Add a");
}

#[test]
fn gitlab_409_is_already_exists() {
    let stub = Stub::start(|req| {
        if req.method == "POST" {
            resp(
                409,
                r#"{"message":["Another open merge request already exists for this source branch: !13"]}"#,
            )
        } else {
            resp(200, &format!("[{GL_CREATED}]"))
        }
    });
    let mut new = new_pr();
    new.reviewers.clear();
    let err = create_pull_request(
        &Client::for_tests(),
        &inst(ServiceKind::GitLab, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &new,
    )
    .unwrap_err();
    assert_eq!(
        err,
        ApiError::AlreadyExists {
            number: 13,
            web_url: "https://gl/mr/13".into()
        }
    );
    assert!(stub.targets()[1].contains("source_branch=feature%2Fa&target_branch=main"));
}

const GITEA_CREATED: &str = r#"{"number":4,"title":"WIP: Add a","state":"open","draft":false,"merged":false,
  "updated_at":"2024-05-02T10:00:00Z","html_url":"https://codeberg.org/octo/demo/pulls/4",
  "user":{"login":"alice"},"head":{"ref":"feature/a","repo":{"full_name":"octo/demo"}},"base":{"ref":"main"}}"#;

#[test]
fn gitea_create_prefixes_wip_and_requests_reviewers() {
    let stub = Stub::start(|req| {
        if req.target.ends_with("/requested_reviewers") {
            resp(201, "[]")
        } else {
            resp(201, GITEA_CREATED)
        }
    });
    let out = create_pull_request(
        &Client::for_tests(),
        &inst(ServiceKind::Gitea, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &new_pr(),
    )
    .unwrap();
    assert_eq!(out.pull_request.state, PrState::Draft);
    assert_eq!(out.reviewers_error, None);
    let reqs = stub.requests();
    assert_eq!(reqs[0].target, "/api/v1/repos/octo/demo/pulls");
    assert_eq!(reqs[0].headers["authorization"], "token sekret");
    assert_eq!(
        body_of(&reqs[0]),
        json!({"title":"WIP: Add a","head":"feature/a","base":"main","body":"Body"})
    );
    assert_eq!(
        reqs[1].target,
        "/api/v1/repos/octo/demo/pulls/4/requested_reviewers"
    );
}

#[test]
fn gitea_409_is_already_exists() {
    let stub = Stub::start(|req| {
        if req.method == "POST" {
            resp(
                409,
                r#"{"message":"pull request already exists for these targets"}"#,
            )
        } else {
            resp(200, &format!("[{GITEA_CREATED}]"))
        }
    });
    let err = create_pull_request(
        &Client::for_tests(),
        &inst(ServiceKind::Gitea, &stub),
        &coords(),
        &tok(TokenKind::Pat),
        &new_pr(),
    )
    .unwrap_err();
    assert_eq!(
        err,
        ApiError::AlreadyExists {
            number: 4,
            web_url: "https://codeberg.org/octo/demo/pulls/4".into()
        }
    );
}

#[test]
fn checkout_refs() {
    assert_eq!(
        pull_request_head_ref(ServiceKind::GitHub, 5).unwrap(),
        "refs/pull/5/head"
    );
    assert_eq!(
        pull_request_head_ref(ServiceKind::Gitea, 5).unwrap(),
        "refs/pull/5/head"
    );
    assert_eq!(
        pull_request_head_ref(ServiceKind::GitLab, 5).unwrap(),
        "refs/merge-requests/5/head"
    );
}

#[test]
fn bitbucket_and_azure_are_unsupported_without_a_request() {
    let c = Client::for_tests();
    for kind in [ServiceKind::Bitbucket, ServiceKind::AzureDevOps] {
        // Unroutable address: any request would fail as Unreachable, not Unsupported.
        let i = Instance::new(kind, "http://127.0.0.1:9").unwrap();
        let a = Auth::anonymous();
        let t = tok(TokenKind::Pat);
        assert_eq!(pull_request_head_ref(kind, 1), Err(ApiError::Unsupported));
        assert_eq!(
            list_pull_requests(
                &c,
                &i,
                &coords(),
                &a,
                &PrFilter::All,
                PrStateFilter::Open,
                1
            )
            .unwrap_err(),
            ApiError::Unsupported
        );
        assert_eq!(
            pull_requests_for_branches(&c, &i, &coords(), &a, &["x".into()]).unwrap_err(),
            ApiError::Unsupported
        );
        assert_eq!(
            commit_checks(&c, &i, &coords(), &a, "abc").unwrap_err(),
            ApiError::Unsupported
        );
        assert_eq!(
            pull_request_review(&c, &i, &coords(), &a, 1).unwrap_err(),
            ApiError::Unsupported
        );
        assert_eq!(
            create_pull_request(&c, &i, &coords(), &t, &new_pr()).unwrap_err(),
            ApiError::Unsupported
        );
    }
}

// ------------------------------------------------------------------ cache & limits

#[test]
fn etag_304_reuses_the_cached_body() {
    let stub = Stub::start(|req| {
        if req.headers.get("if-none-match").map(String::as_str) == Some("\"v1\"") {
            Resp {
                status: 304,
                headers: vec![],
                body: String::new(),
            }
        } else {
            Resp {
                status: 200,
                headers: vec![
                    ("ETag", "\"v1\"".into()),
                    ("Link", "<n>; rel=\"next\"".into()),
                ],
                body: GH_LIST.to_string(),
            }
        }
    });
    let c = Client::for_tests();
    let i = inst(ServiceKind::GitHub, &stub);
    let list = |c: &Client| {
        list_pull_requests(
            c,
            &i,
            &coords(),
            &tok(TokenKind::Pat),
            &PrFilter::All,
            PrStateFilter::All,
            1,
        )
        .unwrap()
    };
    let first = list(&c);
    let second = list(&c);
    assert_eq!(first, second);
    assert!(second.has_next, "next-page flag survives a 304");
    let reqs = stub.requests();
    assert_eq!(reqs.len(), 2);
    assert!(!reqs[0].headers.contains_key("if-none-match"));
    assert_eq!(reqs[1].headers["if-none-match"], "\"v1\"");
    // clones share the cache
    list(&c.clone());
    assert_eq!(stub.requests()[2].headers["if-none-match"], "\"v1\"");
}

#[test]
fn etag_cache_is_per_credentials() {
    let h1 = [("Authorization", "Bearer a".to_string())];
    let h2 = [("Authorization", "Bearer b".to_string())];
    assert_ne!(cache_key("u", &h1), cache_key("u", &h2));
    assert_eq!(cache_key("u", &h1), cache_key("u", &h1));
    assert!(!cache_key("u", &h1).contains("Bearer"));
}

#[test]
fn etag_cache_is_size_capped_first_in_first_out() {
    let cache = ETagCache::new(2);
    let entry = |n: &str| super::net::Entry {
        etag: n.into(),
        body: n.into(),
        has_next: false,
    };
    cache.put("a".into(), entry("a"));
    cache.put("b".into(), entry("b"));
    cache.put("c".into(), entry("c"));
    assert_eq!(cache.len(), 2);
    assert!(cache.get("a").is_none());
    assert!(cache.get("b").is_some() && cache.get("c").is_some());
    // oversized bodies are not kept
    let big = super::net::Entry {
        etag: "e".into(),
        body: "x".repeat(3 * 1024 * 1024),
        has_next: false,
    };
    cache.put("big".into(), big);
    assert!(cache.get("big").is_none());
}

#[test]
fn no_etag_means_nothing_cached() {
    let stub = list_stub("[]", vec![]);
    let c = Client::for_tests();
    let i = inst(ServiceKind::GitHub, &stub);
    for _ in 0..2 {
        list_pull_requests(
            &c,
            &i,
            &coords(),
            &Auth::anonymous(),
            &PrFilter::All,
            PrStateFilter::All,
            1,
        )
        .unwrap();
    }
    assert!(c.etags.is_empty());
}

#[test]
fn rate_limit_exhaustion_reports_reset_time() {
    let stub = Stub::start(|_| Resp {
        status: 403,
        headers: vec![
            ("x-ratelimit-remaining", "0".into()),
            ("x-ratelimit-reset", "1700000000".into()),
        ],
        body: r#"{"message":"API rate limit exceeded"}"#.into(),
    });
    let err = list_pull_requests(
        &Client::for_tests(),
        &inst(ServiceKind::GitHub, &stub),
        &coords(),
        &Auth::anonymous(),
        &PrFilter::All,
        PrStateFilter::Open,
        1,
    )
    .unwrap_err();
    assert_eq!(
        err,
        ApiError::RateLimited {
            reset_at: Some(1_700_000_000)
        }
    );
}

#[test]
fn gitlab_429_uses_ratelimit_reset_and_secondary_limit_uses_retry_after() {
    let stub = Stub::start(|req| {
        if req.target.contains("/api/v4/") {
            Resp {
                status: 429,
                headers: vec![("RateLimit-Reset", "1700000123".into())],
                body: "{}".into(),
            }
        } else {
            Resp {
                status: 403,
                headers: vec![("Retry-After", "60".into())],
                body: "{}".into(),
            }
        }
    });
    let c = Client::for_tests();
    let call = |kind| {
        list_pull_requests(
            &c,
            &inst(kind, &stub),
            &coords(),
            &Auth::anonymous(),
            &PrFilter::All,
            PrStateFilter::Open,
            1,
        )
        .unwrap_err()
    };
    assert_eq!(
        call(ServiceKind::GitLab),
        ApiError::RateLimited {
            reset_at: Some(1_700_000_123)
        }
    );
    match call(ServiceKind::GitHub) {
        ApiError::RateLimited { reset_at: Some(t) } => {
            let now = super::net::now_unix();
            assert!((now + 55..=now + 65).contains(&t), "{t} vs {now}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn error_texts_never_contain_the_token() {
    let errors = [
        ApiError::Unsupported,
        ApiError::Rejected("No commits".into()),
        ApiError::AlreadyExists {
            number: 1,
            web_url: "u".into(),
        },
    ];
    for e in errors {
        assert!(!format!("{e} {e:?}").contains("sekret"));
    }
}
