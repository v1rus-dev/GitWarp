//! The folder → repository flow shared by the "Open from folder" dialog and
//! the welcome screen's Initialize dialog.
//!
//! Both dialogs own a folder field and differ only in what the primary button
//! offers; the debounced probe, the status the banner and footer show, and
//! `git init` with its optional initial commit live here so the two cannot
//! drift apart.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, px, AnyElement, App, Context, IntoElement, ParentElement, SharedString, Styled, Window,
};

use git_core::{probe_folder, Error, ErrorClass, FolderProbe, Git, Repo};
use ui::metrics::{FONT_MONO, FS_BASE};
use ui::theme::ThemeColors;
use ui::BannerKind;

use crate::format::{display_path, format_count};
use crate::history_state::git_unavailable_message;
use crate::workspace::repo_name;

/// The field's debounce: a probe starts this long after the last keystroke.
pub(crate) const PROBE_DEBOUNCE: Duration = Duration::from_millis(250);

/// The primary button of a repository root (and of a missing folder in the
/// Open dialog).
const OPEN: &str = "Open";
const INITIALIZE: &str = "Initialize and open";
const INITIALIZE_SHORT: &str = "Initialize";

/// Which dialog asks. The Open dialog offers to open an existing repository;
/// the welcome dialog offers to create one, so a folder that already belongs
/// to a repository has to be opened instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogKind {
    Open,
    Initialize,
}

/// The dialog's border state, before the labels and texts are looked up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StatusKind {
    Empty,
    Probing,
    Repo,
    Nested,
    Plain,
    Missing,
    Failed,
    /// Git itself is missing or too old; the folder is not to blame.
    GitUnavailable,
}

/// Everything the footer and the banner need for one status. `banner` is the
/// kind and title; the body text is built separately because it contains mono
/// spans and, for a plain folder, the initialize controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StatusSpec {
    pub kind: StatusKind,
    pub banner: Option<(BannerKind, &'static str)>,
    pub primary: &'static str,
    pub enabled: bool,
    pub hint: Option<&'static str>,
}

/// The outcome of a probe, with the message flattened for the banner and the
/// error's class kept for the "git is unavailable" case.
#[derive(Clone, Debug)]
pub(crate) struct ProbeError {
    pub message: SharedString,
    pub git_unavailable: bool,
}

impl ProbeError {
    pub fn from_error(error: &Error) -> Self {
        let git_unavailable = matches!(
            error.class(),
            ErrorClass::GitNotFound | ErrorClass::GitTooOld { .. }
        );
        let message = if git_unavailable {
            git_unavailable_message(error).unwrap_or_else(|| error.to_string())
        } else {
            error.to_string()
        };
        Self {
            message: message.into(),
            git_unavailable,
        }
    }
}

pub(crate) type ProbeResult = Result<FolderProbe, ProbeError>;

/// The status of a folder field with `text`, `probing` and `result`.
///
/// While a probe runs the previous banner stays (`result`), so typing does not
/// flash an empty dialog; only the primary button is disabled.
pub(crate) fn status_spec(
    dialog: DialogKind,
    field_empty: bool,
    probing: bool,
    result: Option<&ProbeResult>,
    branch_blank: bool,
) -> StatusSpec {
    if field_empty {
        return StatusSpec {
            kind: StatusKind::Empty,
            banner: None,
            primary: primary_label(dialog),
            enabled: false,
            hint: Some("Choose a folder to continue"),
        };
    }
    let previous = |result: Option<&ProbeResult>| match result {
        Some(result) => result_spec(dialog, result, branch_blank),
        None => StatusSpec {
            kind: StatusKind::Empty,
            banner: None,
            primary: primary_label(dialog),
            enabled: false,
            hint: None,
        },
    };
    if probing {
        let mut spec = previous(result);
        spec.kind = StatusKind::Probing;
        spec.enabled = false;
        spec.hint = Some("Checking the folder…");
        return spec;
    }
    previous(result)
}

fn result_spec(dialog: DialogKind, result: &ProbeResult, branch_blank: bool) -> StatusSpec {
    match result {
        Ok(FolderProbe::Repo { .. }) => StatusSpec {
            kind: StatusKind::Repo,
            banner: Some(match dialog {
                DialogKind::Open => (BannerKind::Ok, "Git repository found"),
                DialogKind::Initialize => (BannerKind::Warn, "This folder is already a repository"),
            }),
            primary: OPEN,
            enabled: true,
            hint: None,
        },
        Ok(FolderProbe::Nested { .. }) => StatusSpec {
            kind: StatusKind::Nested,
            banner: Some(match dialog {
                DialogKind::Open => (BannerKind::Warn, "This folder is inside a repository"),
                DialogKind::Initialize => (BannerKind::Warn, "This folder is already a repository"),
            }),
            primary: OPEN,
            enabled: true,
            hint: None,
        },
        Ok(FolderProbe::Plain) => plain_spec(dialog, branch_blank),
        // `git init` creates the directory, so a missing folder is fine when
        // the dialog is there to create a repository.
        Ok(FolderProbe::Missing) if dialog == DialogKind::Initialize => {
            plain_spec(dialog, branch_blank)
        }
        Ok(FolderProbe::Missing) => StatusSpec {
            kind: StatusKind::Missing,
            banner: Some((BannerKind::Error, "Folder does not exist")),
            primary: OPEN,
            enabled: false,
            hint: Some("Choose an existing folder"),
        },
        Err(error) if error.git_unavailable => StatusSpec {
            kind: StatusKind::GitUnavailable,
            banner: Some((BannerKind::Error, "Git is unavailable")),
            primary: OPEN,
            enabled: false,
            hint: Some("Choose another folder"),
        },
        Err(_) => StatusSpec {
            kind: StatusKind::Failed,
            banner: Some((BannerKind::Error, "Could not read this folder")),
            primary: OPEN,
            enabled: false,
            hint: Some("Choose another folder"),
        },
    }
}

fn plain_spec(dialog: DialogKind, branch_blank: bool) -> StatusSpec {
    StatusSpec {
        kind: StatusKind::Plain,
        banner: Some((BannerKind::Info, "Not a Git repository yet")),
        primary: plain_primary(dialog),
        enabled: !branch_blank,
        hint: branch_blank.then_some("Enter a default branch name"),
    }
}

fn primary_label(dialog: DialogKind) -> &'static str {
    match dialog {
        DialogKind::Open => OPEN,
        DialogKind::Initialize => INITIALIZE_SHORT,
    }
}

fn plain_primary(dialog: DialogKind) -> &'static str {
    match dialog {
        DialogKind::Open => INITIALIZE,
        DialogKind::Initialize => INITIALIZE_SHORT,
    }
}

/// The plain part of the banner body: the sentences without mono spans.
pub(crate) fn banner_body(flow: &InitFlow, dialog: DialogKind) -> Option<SharedString> {
    match flow.result()? {
        Ok(FolderProbe::Plain) => Some("You can initialize one here and open it.".into()),
        Ok(FolderProbe::Missing) if dialog == DialogKind::Open => {
            Some("Check the path, or choose a folder with Browse.".into())
        }
        Ok(FolderProbe::Missing) => Some("The folder will be created by git init.".into()),
        Err(error) => Some(error.message.clone()),
        _ => None,
    }
}

/// The banner's rich body (mono spans) for a repository or a nested folder.
/// `Banner` takes plain text, so this element sits after it instead of
/// re-implementing the banner.
pub(crate) fn details(
    flow: &InitFlow,
    home: Option<&Path>,
    colors: ThemeColors,
) -> Option<AnyElement> {
    match flow.result()? {
        Ok(FolderProbe::Repo {
            root,
            branch,
            commits,
        }) => {
            let branch = branch.clone().unwrap_or_else(|| "detached HEAD".to_owned());
            let commits = usize::try_from(*commits).unwrap_or(usize::MAX);
            Some(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.))
                    .text_size(FS_BASE)
                    .text_color(colors.muted)
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .child(repo_name(root.as_path())),
                    )
                    .child("· branch")
                    .child(div().font_family(FONT_MONO).child(branch))
                    .child(format!("· {} commits", format_count(commits)))
                    .into_any_element(),
            )
        }
        Ok(FolderProbe::Nested { root }) => {
            let root = display_path(root.as_path(), home);
            Some(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(2.))
                    .text_size(FS_BASE)
                    .text_color(colors.muted)
                    .child("The repository root is ")
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .max_w(px(420.))
                            .truncate()
                            .child(root),
                    )
                    .child(". GitWarp will open the root, not this subfolder.")
                    .into_any_element(),
            )
        }
        _ => None,
    }
}

/// Called with the root to open and whether `git init` just created it.
pub(crate) type OpenCallback = Rc<dyn Fn(std::path::PathBuf, bool, &mut Window, &mut App)>;

/// The probe and init state of one dialog.
pub(crate) struct InitFlow {
    git: Result<Arc<Git>, Arc<Error>>,
    /// Set once the user typed in the field or picked a folder there; an empty
    /// field has no status.
    generation: u64,
    probing: bool,
    result: Option<ProbeResult>,
    initial_commit: bool,
    busy: bool,
    /// Set once an open is scheduled; a second activation before it runs must
    /// not open the same folder twice.
    opening: bool,
    error: Option<SharedString>,
    on_open: Option<OpenCallback>,
}

impl InitFlow {
    pub fn new(git: Result<Arc<Git>, Arc<Error>>) -> Self {
        Self {
            git,
            generation: 0,
            probing: false,
            result: None,
            // Both dialogs start with the initial commit on.
            initial_commit: true,
            busy: false,
            opening: false,
            error: None,
            on_open: None,
        }
    }

    /// The repository the dialog opens: the caller closes itself there.
    pub fn set_on_open(&mut self, on_open: OpenCallback) {
        self.on_open = Some(on_open);
    }

    pub fn result(&self) -> Option<&ProbeResult> {
        self.result.as_ref()
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn initial_commit(&self) -> bool {
        self.initial_commit
    }

    pub fn spec(&self, dialog: DialogKind, field_empty: bool, branch_blank: bool) -> StatusSpec {
        status_spec(
            dialog,
            field_empty,
            self.probing,
            self.result.as_ref(),
            branch_blank,
        )
    }

    pub fn set_initial_commit(&mut self, on: bool, cx: &mut Context<Self>) {
        self.initial_commit = on;
        cx.notify();
    }

    /// An empty field has no status: drop any result and any probe in flight.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.probing = false;
        self.result = None;
        cx.notify();
    }

    /// Starts a probe for `path`, after the debounce or at once.
    pub fn probe(&mut self, path: std::path::PathBuf, debounced: bool, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.probing = true;
        // A new folder is a new attempt: an error about the previous one is
        // stale now.
        self.error = None;
        cx.notify();
        let git = self.git.clone();
        cx.spawn(async move |this, cx| {
            if debounced {
                cx.background_executor().timer(PROBE_DEBOUNCE).await;
            }
            // A keystroke after this probe was scheduled made it obsolete.
            let current = this.update(cx, |flow, _| flow.generation).ok();
            if current != Some(generation) {
                return;
            }
            let result =
                cx.background_executor()
                    .spawn(async move {
                        match git {
                            Ok(git) => probe_folder(git, &path)
                                .map_err(|error| ProbeError::from_error(&error)),
                            Err(error) => Err(ProbeError::from_error(&error)),
                        }
                    })
                    .await;
            this.update(cx, |flow, cx| flow.finish_probe(generation, result, cx))
                .ok();
        })
        .detach();
    }

    fn finish_probe(&mut self, generation: u64, result: ProbeResult, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        self.probing = false;
        self.result = Some(result);
        cx.notify();
    }

    /// `git init` the folder; the callback opens the new repository. On a
    /// failure the folder is probed again: the repository may exist even
    /// though the initial commit did not happen, and then the dialog offers
    /// "Open" instead of a dead end.
    pub fn start_init(
        &mut self,
        path: std::path::PathBuf,
        branch: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        let git = match &self.git {
            Ok(git) => git.clone(),
            Err(error) => {
                let message = error.to_string();
                self.probe(path, false, cx);
                self.fail(message, cx);
                return;
            }
        };
        self.busy = true;
        self.error = None;
        cx.notify();

        let initial_commit = self.initial_commit;
        let init_path = path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Repo::init_with(git, &init_path, branch.as_deref(), initial_commit)
                        .map(|repo| repo.root().to_path_buf())
                        .map_err(|error| error.to_string())
                })
                .await;
            this.update_in(cx, |flow, window, cx| {
                flow.finish_init(result, path, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_init(
        &mut self,
        result: Result<std::path::PathBuf, String>,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = false;
        // The fields were disabled while `git init` ran, so `path` is still
        // what the folder field shows; the re-probe below cannot resurrect an
        // older folder.
        cx.notify();
        match result {
            Ok(root) => {
                if let Some(on_open) = self.on_open.clone() {
                    on_open(root, true, window, cx);
                }
            }
            Err(message) => {
                // The repository may have been created before the commit
                // failed: re-probe so the primary becomes "Open".
                self.probe(path, false, cx);
                self.fail(message, cx);
            }
        }
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        let message = message.trim();
        self.busy = false;
        self.error = Some(
            if message.is_empty() {
                "git failed without a message"
            } else {
                message
            }
            .to_owned()
            .into(),
        );
        cx.notify();
    }

    /// Opens a folder that already is a repository root.
    ///
    /// Both dialogs call this from inside their own update, which still holds
    /// the dialog's entity lease; the callback re-enters the dialog to close
    /// it, so it has to run after that update returned — otherwise gpui
    /// panics with a double lease.
    pub fn open_root(
        &mut self,
        root: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opening {
            return;
        }
        self.opening = true;
        cx.defer_in(window, move |flow, window, cx| {
            if let Some(on_open) = flow.on_open.clone() {
                on_open(root, false, window, cx);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo() -> ProbeResult {
        Ok(FolderProbe::Repo {
            root: PathBuf::from("/home/user/gitwarp"),
            branch: Some("main".to_owned()),
            commits: 21,
        })
    }

    #[test]
    fn an_empty_field_asks_for_a_folder_and_disables_open() {
        let spec = status_spec(DialogKind::Open, true, false, None, false);
        assert_eq!(spec.kind, StatusKind::Empty);
        assert_eq!(spec.banner, None);
        assert_eq!(spec.primary, "Open");
        assert!(!spec.enabled);
        assert_eq!(spec.hint, Some("Choose a folder to continue"));
    }

    #[test]
    fn a_repository_root_keeps_the_previous_banner_while_probing() {
        let repo = repo();
        let ready = status_spec(DialogKind::Open, false, false, Some(&repo), false);
        assert_eq!(
            (ready.kind, ready.banner),
            (
                StatusKind::Repo,
                Some((BannerKind::Ok, "Git repository found"))
            )
        );
        assert_eq!(
            (ready.primary, ready.enabled, ready.hint),
            ("Open", true, None)
        );

        // Typing again: the same banner stays, the button goes dead and the
        // footer says what is happening.
        let probing = status_spec(DialogKind::Open, false, true, Some(&repo), false);
        assert_eq!(probing.kind, StatusKind::Probing);
        assert_eq!(probing.banner, ready.banner);
        assert_eq!(probing.primary, "Open");
        assert!(!probing.enabled);
        assert_eq!(probing.hint, Some("Checking the folder…"));
    }

    #[test]
    fn a_nested_folder_opens_the_root() {
        let nested = Ok(FolderProbe::Nested {
            root: PathBuf::from("/home/user/gitwarp"),
        });
        let spec = status_spec(DialogKind::Open, false, false, Some(&nested), false);
        assert_eq!(spec.kind, StatusKind::Nested);
        assert_eq!(
            spec.banner,
            Some((BannerKind::Warn, "This folder is inside a repository"))
        );
        assert_eq!(
            (spec.primary, spec.enabled, spec.hint),
            ("Open", true, None)
        );
    }

    #[test]
    fn a_plain_folder_initializes_and_needs_a_branch() {
        let plain = Ok(FolderProbe::Plain);
        let ready = status_spec(DialogKind::Open, false, false, Some(&plain), false);
        assert_eq!(ready.kind, StatusKind::Plain);
        assert_eq!(
            ready.banner,
            Some((BannerKind::Info, "Not a Git repository yet"))
        );
        assert_eq!(
            (ready.primary, ready.enabled, ready.hint),
            ("Initialize and open", true, None)
        );

        let blank = status_spec(DialogKind::Open, false, false, Some(&plain), true);
        assert_eq!(blank.primary, "Initialize and open");
        assert!(!blank.enabled);
        assert_eq!(blank.hint, Some("Enter a default branch name"));
    }

    #[test]
    fn a_missing_folder_reports_the_error_and_hints_at_browse() {
        let missing = Ok(FolderProbe::Missing);
        let spec = status_spec(DialogKind::Open, false, false, Some(&missing), false);
        assert_eq!(spec.kind, StatusKind::Missing);
        assert_eq!(
            spec.banner,
            Some((BannerKind::Error, "Folder does not exist"))
        );
        assert_eq!((spec.primary, spec.enabled), ("Open", false));
        assert_eq!(spec.hint, Some("Choose an existing folder"));
    }

    #[test]
    fn a_failed_probe_shows_gits_message_and_a_way_out() {
        let failed: ProbeResult = Err(ProbeError {
            message: "git rev-parse failed (exit 128)".into(),
            git_unavailable: false,
        });
        let spec = status_spec(DialogKind::Open, false, false, Some(&failed), false);
        assert_eq!(spec.kind, StatusKind::Failed);
        assert_eq!(
            spec.banner,
            Some((BannerKind::Error, "Could not read this folder"))
        );
        assert_eq!((spec.primary, spec.enabled), ("Open", false));
        assert_eq!(spec.hint, Some("Choose another folder"));
    }

    #[test]
    fn a_missing_git_does_not_blame_the_folder() {
        let failed: ProbeResult = Err(ProbeError {
            message: "git was not found".into(),
            git_unavailable: true,
        });
        let spec = status_spec(DialogKind::Open, false, false, Some(&failed), false);
        assert_eq!(spec.kind, StatusKind::GitUnavailable);
        assert_eq!(spec.banner, Some((BannerKind::Error, "Git is unavailable")));
        assert!(!spec.enabled);
    }

    #[test]
    fn the_initialize_dialog_offers_to_open_an_existing_repository() {
        let repo = repo();
        let spec = status_spec(DialogKind::Initialize, false, false, Some(&repo), false);
        assert_eq!(spec.kind, StatusKind::Repo);
        assert_eq!(
            spec.banner,
            Some((BannerKind::Warn, "This folder is already a repository"))
        );
        assert_eq!(
            (spec.primary, spec.enabled),
            ("Open", true),
            "an existing repository must be opened, not initialized"
        );

        let nested = Ok(FolderProbe::Nested {
            root: PathBuf::from("/home/user/gitwarp"),
        });
        let spec = status_spec(DialogKind::Initialize, false, false, Some(&nested), false);
        assert_eq!(
            spec.banner,
            Some((BannerKind::Warn, "This folder is already a repository"))
        );
        assert_eq!(spec.primary, "Open");
    }

    #[test]
    fn the_initialize_dialog_accepts_a_missing_folder() {
        let missing = Ok(FolderProbe::Missing);
        let spec = status_spec(DialogKind::Initialize, false, false, Some(&missing), false);
        assert_eq!(spec.kind, StatusKind::Plain);
        assert_eq!(spec.primary, "Initialize");
        assert!(spec.enabled);
        assert_eq!(spec.hint, None);
    }
}
