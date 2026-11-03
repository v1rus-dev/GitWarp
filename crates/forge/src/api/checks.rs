//! CI status of a commit: check runs, pipelines and commit statuses.
//!
//! Endpoints:
//! - GitHub check runs `GET /repos/{owner}/{repo}/commits/{ref}/check-runs`
//!   <https://docs.github.com/en/rest/checks/runs#list-check-runs-for-a-git-reference>
//!   and combined status `GET /repos/{owner}/{repo}/commits/{ref}/status`
//!   <https://docs.github.com/en/rest/commits/statuses#get-the-combined-status-for-a-specific-reference>
//! - GitLab pipelines `GET /projects/:id/pipelines?sha=`
//!   <https://docs.gitlab.com/api/pipelines/#list-project-pipelines> and jobs
//!   `GET /projects/:id/pipelines/:pipeline_id/jobs`
//!   <https://docs.gitlab.com/api/jobs/#list-pipeline-jobs>
//! - Gitea / Forgejo combined status `GET /repos/{owner}/{repo}/commits/{ref}/status`
//!   (swagger: <https://codeberg.org/swagger.v1.json>)
//!
//! State mapping (service state → [`CiState`]):
//!
//! | CiState   | GitHub check run (`status` / `conclusion`)            | GitHub / Gitea status | GitLab pipeline / job                                                   |
//! |-----------|--------------------------------------------------------|-----------------------|-------------------------------------------------------------------------|
//! | Pending   | queued, waiting, requested, pending, completed + null  | pending               | created, waiting_for_resource, preparing, waiting_for_callback, pending, scheduled, manual |
//! | Running   | in_progress                                            | –                     | running, canceling                                                      |
//! | Passed    | success, neutral, skipped                              | success, warning      | success                                                                 |
//! | Failed    | failure, timed_out, action_required, startup_failure   | failure, error        | failed                                                                  |
//! | Cancelled | cancelled, stale                                       | –                     | canceled, skipped                                                       |
//!
//! Unknown values map to Pending. The overall state of a list is Failed if
//! any item failed, else Running, else Pending, else Cancelled if any was
//! cancelled, else Passed; no items at all is `None`.

use super::net::{parse_rfc3339, q, repo_url, Auth};
use super::pulls::text;
use super::{ApiError, Client};
use crate::{Instance, RepoCoords, ServiceKind};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiState {
    Pending,
    Running,
    Passed,
    Failed,
    Cancelled,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub state: CiState,
    pub duration_secs: Option<u64>,
    pub web_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitChecks {
    pub overall: CiState,
    pub checks: Vec<Check>,
}

/// Checks of the commit `sha`. GitHub combines check runs and the combined
/// commit status (two requests), GitLab reads the newest pipeline of the commit
/// and its jobs (two requests; its status is the overall state), Gitea the
/// combined status. Bitbucket / Azure: [`ApiError::Unsupported`].
pub fn commit_checks(
    client: &Client,
    instance: &Instance,
    coords: &RepoCoords,
    auth: &Auth<'_>,
    sha: &str,
) -> Result<CommitChecks, ApiError> {
    let repo = repo_url(instance, coords)?;
    let headers = auth.headers(instance.kind);
    let sha = q(sha);
    match instance.kind {
        ServiceKind::GitHub => {
            let (runs, _) = client.get_json_cached::<Value>(
                &format!("{repo}/commits/{sha}/check-runs?per_page=100"),
                &headers,
            )?;
            let (status, _) = client.get_json_cached::<Value>(
                &format!("{repo}/commits/{sha}/status?per_page=100"),
                &headers,
            )?;
            let mut checks = parse_github_runs(&runs);
            checks.extend(parse_statuses(&status));
            Ok(with_overall(checks))
        }
        ServiceKind::Gitea => {
            let (status, _) = client.get_json_cached::<Value>(
                &format!("{repo}/commits/{sha}/status?limit=50"),
                &headers,
            )?;
            Ok(with_overall(parse_statuses(&status)))
        }
        ServiceKind::GitLab => {
            let (pipelines, _) = client.get_json_cached::<Vec<Value>>(
                &format!("{repo}/pipelines?sha={sha}&order_by=id&sort=desc&per_page=1"),
                &headers,
            )?;
            let Some(pipeline) = pipelines.first() else {
                return Ok(CommitChecks {
                    overall: CiState::None,
                    checks: Vec::new(),
                });
            };
            let id = pipeline
                .get("id")
                .and_then(Value::as_u64)
                .ok_or_else(|| ApiError::Decode("pipeline without id".into()))?;
            let overall = gitlab_state(text(pipeline, "/status").as_deref().unwrap_or(""));
            let (jobs, _) = client.get_json_cached::<Vec<Value>>(
                &format!("{repo}/pipelines/{id}/jobs?per_page=100"),
                &headers,
            )?;
            Ok(CommitChecks {
                overall,
                checks: parse_gitlab_jobs(&jobs),
            })
        }
        ServiceKind::Bitbucket | ServiceKind::AzureDevOps => Err(ApiError::Unsupported),
    }
}

pub(crate) fn with_overall(checks: Vec<Check>) -> CommitChecks {
    CommitChecks {
        overall: aggregate(checks.iter().map(|c| c.state)),
        checks,
    }
}

pub(crate) fn aggregate(states: impl Iterator<Item = CiState>) -> CiState {
    let states: Vec<CiState> = states.collect();
    let any = |s| states.contains(&s);
    if states.is_empty() {
        CiState::None
    } else if any(CiState::Failed) {
        CiState::Failed
    } else if any(CiState::Running) {
        CiState::Running
    } else if any(CiState::Pending) {
        CiState::Pending
    } else if any(CiState::Cancelled) {
        CiState::Cancelled
    } else {
        CiState::Passed
    }
}

pub(crate) fn github_run_state(status: &str, conclusion: Option<&str>) -> CiState {
    match status {
        "in_progress" => CiState::Running,
        "completed" => match conclusion {
            Some("success" | "neutral" | "skipped") => CiState::Passed,
            Some("failure" | "timed_out" | "action_required" | "startup_failure") => {
                CiState::Failed
            }
            Some("cancelled" | "stale") => CiState::Cancelled,
            _ => CiState::Pending,
        },
        // queued, waiting, requested, pending and anything new
        _ => CiState::Pending,
    }
}

pub(crate) fn status_state(state: &str) -> CiState {
    match state {
        "success" | "warning" => CiState::Passed,
        "failure" | "error" => CiState::Failed,
        _ => CiState::Pending,
    }
}

pub(crate) fn gitlab_state(status: &str) -> CiState {
    match status {
        "running" | "canceling" => CiState::Running,
        "success" => CiState::Passed,
        "failed" => CiState::Failed,
        "canceled" | "skipped" => CiState::Cancelled,
        // created, waiting_for_resource, preparing, waiting_for_callback,
        // pending, scheduled, manual and anything new
        _ => CiState::Pending,
    }
}

pub(crate) fn parse_github_runs(v: &Value) -> Vec<Check> {
    let Some(runs) = v.get("check_runs").and_then(Value::as_array) else {
        return Vec::new();
    };
    runs.iter()
        .filter_map(|r| {
            let status = text(r, "/status")?;
            let conclusion = text(r, "/conclusion");
            let duration_secs = match (
                text(r, "/started_at").and_then(|t| parse_rfc3339(&t)),
                text(r, "/completed_at").and_then(|t| parse_rfc3339(&t)),
            ) {
                (Some(a), Some(b)) => Some(b.saturating_sub(a)),
                _ => None,
            };
            Some(Check {
                name: text(r, "/name")?,
                state: github_run_state(&status, conclusion.as_deref()),
                duration_secs,
                web_url: text(r, "/html_url"),
            })
        })
        .collect()
}

/// Statuses of GitHub (`state`) and Gitea (`status`) combined-status answers.
/// GitHub reports a combined `pending` for a commit without any status, which
/// is not a check, hence only the listed statuses count.
pub(crate) fn parse_statuses(v: &Value) -> Vec<Check> {
    let Some(list) = v.get("statuses").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|s| {
            let state = text(s, "/state").or_else(|| text(s, "/status"))?;
            Some(Check {
                name: text(s, "/context").unwrap_or_default(),
                state: status_state(&state),
                duration_secs: None,
                web_url: text(s, "/target_url").filter(|u| !u.is_empty()),
            })
        })
        .collect()
}

pub(crate) fn parse_gitlab_jobs(jobs: &[Value]) -> Vec<Check> {
    jobs.iter()
        .filter_map(|j| {
            Some(Check {
                name: text(j, "/name")?,
                state: gitlab_state(&text(j, "/status")?),
                duration_secs: j
                    .get("duration")
                    .and_then(Value::as_f64)
                    .filter(|d| *d >= 0.0)
                    .map(|d| d.round() as u64),
                web_url: text(j, "/web_url"),
            })
        })
        .collect()
}
