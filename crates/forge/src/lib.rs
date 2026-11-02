//! Hosting services (GitHub, GitLab, Gitea / Forgejo, Bitbucket, Azure DevOps).
//! Most of the crate is pure types and functions; I/O lives in [`api`] (blocking
//! HTTP: instance probe, token check) and [`secrets`] (token storage).
//!
//! - [`ServiceKind`] and [`Instance`] describe a service and one concrete
//!   installation of it (`https://github.com`, `https://gitlab.company.com`).
//! - [`RemoteRef::parse`] turns any git remote URL (https, ssh, scp-like) into
//!   host, namespace and repository name.
//! - [`Instance::matches`] and [`resolve`] / [`resolve_with`] map a remote to
//!   the account that should be used for it.
//! - [`web_url`] builds the browser URL of a repository, commit, branch, tag,
//!   file or pull / merge request.
//! - [`api::probe`] detects the service behind a URL, [`api::current_user`]
//!   validates a token; [`secrets::SecretStore`] keeps tokens in the OS keychain.

pub mod api;
mod encode;
mod instance;
mod kind;
mod remote;
mod resolve;
pub mod secrets;
mod url;
mod web;

#[cfg(test)]
mod tests;

pub use instance::{known_kind_for_host, Instance, InstanceError, PUBLIC_INSTANCES};
pub use kind::ServiceKind;
pub use remote::{RemoteRef, RepoCoords, Transport};
pub use resolve::{resolve, resolve_with, AccountRef, Resolution};
pub use web::{web_url, WebTarget};
