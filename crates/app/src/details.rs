//! The data behind the details dock: the selected commit's metadata, the
//! files it changed and one file's diff. Reading happens off the UI thread;
//! the state here only holds the answer (or why there is none).

use std::path::Path;
use std::sync::Arc;

use git_core::{ChangedFile, CommitDetails, Git, Oid, Repo};

/// What the details dock knows about the selected commit.
#[derive(Clone, Debug, Default)]
pub enum DetailsState {
    /// Nothing is selected yet (the history is empty).
    #[default]
    Empty,
    Loading,
    Loaded(Box<CommitDetailsData>),
    Failed(String),
}

impl DetailsState {
    pub fn details(&self) -> Option<&CommitDetails> {
        match self {
            Self::Loaded(data) => Some(&data.details),
            _ => None,
        }
    }

    pub fn files(&self) -> Option<&[ChangedFile]> {
        match self {
            Self::Loaded(data) => Some(&data.files),
            _ => None,
        }
    }
}

/// One commit's metadata and changed files.
#[derive(Clone, Debug)]
pub struct CommitDetailsData {
    pub details: CommitDetails,
    pub files: Vec<ChangedFile>,
}

/// Reads the details and changed files of `oid`. Both come from one `git`
/// process tree, so the pane shows them together.
pub fn read_details(git: Arc<Git>, root: &Path, oid: Oid) -> Result<CommitDetailsData, String> {
    let repo = Repo::open(git, root).map_err(|error| error.to_string())?;
    let details = repo
        .commit_details(oid)
        .map_err(|error| error.to_string())?;
    let files = repo
        .commit_changes(oid)
        .map_err(|error| error.to_string())?;
    Ok(CommitDetailsData { details, files })
}
