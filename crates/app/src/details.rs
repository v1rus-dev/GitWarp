//! The data behind the details dock: the selected commit's metadata, the
//! files it changed and one file's diff. Reading happens off the UI thread;
//! the state here only holds the answer (or why there is none).

use std::path::Path;
use std::sync::Arc;

use git_core::{ChangedFile, CommitDetails, FileDiff, Git, Oid, Repo};

use crate::diff_model::{build_rows, build_split_rows, max_columns, DiffRow, SplitRow};

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

/// One file's diff, already flattened into the unified view's rows.
#[derive(Clone, Debug)]
pub struct DiffData {
    pub diff: FileDiff,
    pub rows: Vec<DiffRow>,
    pub split_rows: Vec<SplitRow>,
    /// Widest row in display columns, for the horizontal scroll width.
    pub columns: usize,
}

/// What the diff pane knows about the selected file.
#[derive(Clone, Debug, Default)]
pub enum DiffState {
    /// No file selected yet.
    #[default]
    Empty,
    Loading,
    Loaded(Arc<DiffData>),
    Failed(String),
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

/// Reads one file's diff and flattens it into rows off the UI thread.
pub fn read_diff(
    git: Arc<Git>,
    root: &Path,
    oid: Oid,
    file: &ChangedFile,
) -> Result<DiffData, String> {
    let repo = Repo::open(git, root).map_err(|error| error.to_string())?;
    let diff = repo
        .file_diff(oid, &file.path, file.old_path.as_deref())
        .map_err(|error| error.to_string())?;
    let rows = build_rows(&diff.hunks);
    let split_rows = build_split_rows(&rows);
    let columns = max_columns(&rows);
    Ok(DiffData {
        diff,
        rows,
        split_rows,
        columns,
    })
}
