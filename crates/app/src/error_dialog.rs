//! Recognises the git failures the app can offer a fix for.
//!
//! The classifier is pure: it reads `git`'s stderr and returns a small enum.
//! [`open`] turns one into a modal that explains the failure, shows the raw
//! output in selectable text, and offers the matching action.

use std::rc::Rc;

use gpui::{
    div, px, App, AppContext, ClickEvent, Context, ElementId, Entity, InteractiveElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};

use ui::metrics::{FONT_MONO, MONO};
use ui::theme::ActiveTheme;
use ui::{dialog, overlay, Button, Dialog, DialogFocus, IconName, SelectableText};

/// A git failure with a known remedy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KnownError {
    /// The branch is checked out in another worktree.
    BranchInWorktree { branch: String, path: String },
    /// A switch would overwrite local modifications.
    LocalChangesOverwritten,
    /// The remote has commits the local branch does not.
    PushRejected,
    /// A fast-forward-only pull met a diverged branch.
    PullNotFastForward,
}

impl KnownError {
    pub(crate) fn title(&self) -> SharedString {
        match self {
            Self::BranchInWorktree { .. } => ui::t!("dialogs.error.worktree_title"),
            Self::LocalChangesOverwritten => ui::t!("dialogs.error.local_changes_title"),
            Self::PushRejected => ui::t!("dialogs.error.push_rejected_title"),
            Self::PullNotFastForward => ui::t!("dialogs.error.pull_diverged_title"),
        }
    }

    pub(crate) fn explanation(&self) -> SharedString {
        match self {
            Self::BranchInWorktree { branch, path } => ui::t!(
                "dialogs.error.worktree_body",
                branch = branch.as_str(),
                path = path.as_str()
            ),
            Self::LocalChangesOverwritten => ui::t!("dialogs.error.local_changes_body"),
            Self::PushRejected => ui::t!("dialogs.error.push_rejected_body"),
            Self::PullNotFastForward => ui::t!("dialogs.error.pull_diverged_body"),
        }
    }
}

/// Classifies a git error by its stderr.
pub(crate) fn classify_git_error(error: &git_core::Error) -> Option<KnownError> {
    match error {
        git_core::Error::Git { stderr, .. } => classify_git_message(stderr),
        _ => None,
    }
}

/// Classifies raw git output. Kept separate so it can be unit-tested on the
/// exact strings git prints.
pub(crate) fn classify_git_message(message: &str) -> Option<KnownError> {
    for line in message.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("fatal: '")
            .or_else(|| line.strip_prefix("error: '"))
        else {
            continue;
        };
        for marker in [
            "' is already used by worktree at '",
            "' is already checked out at '",
        ] {
            if let Some((branch, tail)) = rest.split_once(marker) {
                let path = tail.strip_suffix('\'').unwrap_or(tail);
                return Some(KnownError::BranchInWorktree {
                    branch: branch.to_owned(),
                    path: path.to_owned(),
                });
            }
        }
    }

    if message
        .contains("Your local changes to the following files would be overwritten by checkout")
    {
        return Some(KnownError::LocalChangesOverwritten);
    }
    if message.contains("Updates were rejected because the tip of your current branch is behind")
        || message.contains("non-fast-forward")
    {
        return Some(KnownError::PushRejected);
    }
    if message.contains("Not possible to fast-forward, aborting") {
        return Some(KnownError::PullNotFastForward);
    }
    None
}

/// What git itself said, without the app's `git <cmd> failed` wrapper.
pub(crate) fn raw_output(error: &git_core::Error) -> String {
    match error {
        git_core::Error::Git { stderr, .. } => stderr.trim().to_owned(),
        _ => error.to_string(),
    }
}

type ErrorActionHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// One button in the error dialog.
pub(crate) struct ErrorAction {
    label: SharedString,
    on_click: ErrorActionHandler,
}

impl ErrorAction {
    pub(crate) fn new(
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            on_click: Rc::new(on_click),
        }
    }
}

struct StoredAction {
    label: SharedString,
    on_click: ErrorActionHandler,
    focus: gpui::FocusHandle,
}

pub(crate) struct ErrorDialog {
    layer: Option<overlay::LayerId>,
    title: SharedString,
    explanation: SharedString,
    details: SharedString,
    actions: Vec<StoredAction>,
    cancel_focus: gpui::FocusHandle,
}

impl ErrorDialog {
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            dialog::close(layer, window, cx);
        }
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let action = self
            .actions
            .get(index)
            .map(|action| action.on_click.clone());
        self.close(window, cx);
        if let Some(action) = action {
            action(window, cx);
        }
    }
}

/// Opens the dialog. Actions run after it closes; with no actions it shows the
/// explanation and git output with just Cancel.
pub(crate) fn open(
    title: impl Into<SharedString>,
    explanation: impl Into<SharedString>,
    details: impl Into<SharedString>,
    actions: Vec<ErrorAction>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ErrorDialog> {
    let dialog = cx.new(|cx| ErrorDialog {
        layer: None,
        title: title.into(),
        explanation: explanation.into(),
        details: details.into(),
        actions: actions
            .into_iter()
            .map(|action| StoredAction {
                label: action.label,
                on_click: action.on_click,
                focus: cx.focus_handle().tab_stop(true),
            })
            .collect(),
        cancel_focus: cx.focus_handle().tab_stop(true),
    });
    // Keep a strong handle in the dialog host: callers do not have to store the
    // returned entity for the dialog to survive until it closes.
    let state = dialog.clone();
    let (layer, _host) = dialog::open_with_view(window, cx, move |_, cx| build(&state, cx));
    dialog.update(cx, |dialog, _| dialog.layer = Some(layer));
    dialog
}

fn build(state: &Entity<ErrorDialog>, cx: &App) -> Dialog {
    let colors = cx.theme().colors;
    let dialog = state.read(cx);

    let cancel = {
        let state = state.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            state.update(cx, |dialog, cx| dialog.cancel(window, cx));
        }
    };

    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .text_color(colors.text)
        .child(dialog.explanation.clone())
        .child(
            div()
                .id(ElementId::Name("error-dialog-details-box".into()))
                .max_h(px(180.))
                .overflow_y_scroll()
                .p(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(colors.border_strong)
                .bg(colors.bg)
                .font_family(FONT_MONO)
                .text_size(MONO)
                .line_height(px(18.))
                .text_color(colors.text)
                .child(SelectableText::new(
                    ElementId::Name("error-dialog-details".into()),
                    dialog.details.clone(),
                )),
        );

    let mut footer = div().flex().items_center().gap(px(8.)).child(
        Button::new("error-dialog-cancel")
            .label(ui::t!("dialogs.common.cancel"))
            .focus_handle(dialog.cancel_focus.clone())
            .on_click(cancel),
    );
    let mut focus_order = vec![DialogFocus::control(dialog.cancel_focus.clone())];
    for (index, action) in dialog.actions.iter().enumerate() {
        let click = {
            let state = state.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                state.update(cx, |dialog, cx| dialog.choose(index, window, cx));
            }
        };
        footer = footer.child(
            Button::new(ElementId::Name(
                format!("error-dialog-action-{index}").into(),
            ))
            .label(action.label.clone())
            .focus_handle(action.focus.clone())
            .on_click(click),
        );
        focus_order.push(DialogFocus::control(action.focus.clone()));
    }

    Dialog::new(dialog.title.clone())
        .icon(IconName::Warn)
        .width(px(560.))
        .compact_body()
        .body(body)
        .footer(footer)
        .focus_order(focus_order)
        .autofocus(dialog.cancel_focus.clone())
        .default_button(dialog.cancel_focus.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_used_by_a_worktree_names_both() {
        let stderr =
            "fatal: 'fix/mms2-8048' is already used by worktree at '/home/mira/MMS2-8048'\n";
        assert_eq!(
            classify_git_message(stderr),
            Some(KnownError::BranchInWorktree {
                branch: "fix/mms2-8048".to_owned(),
                path: "/home/mira/MMS2-8048".to_owned(),
            })
        );
    }

    #[test]
    fn the_older_checked_out_wording_is_recognised() {
        let stderr = "fatal: 'main' is already checked out at '/tmp/repo'\n";
        assert_eq!(
            classify_git_message(stderr),
            Some(KnownError::BranchInWorktree {
                branch: "main".to_owned(),
                path: "/tmp/repo".to_owned(),
            })
        );
    }

    #[test]
    fn checkout_overwriting_local_changes_is_recognised() {
        let stderr = "error: Your local changes to the following files would be overwritten by checkout:\n\tsrc/main.rs\nPlease commit your changes or stash them before you switch branches.\nAborting\n";
        assert_eq!(
            classify_git_message(stderr),
            Some(KnownError::LocalChangesOverwritten)
        );
    }

    #[test]
    fn a_rejected_push_is_recognised() {
        let stderr = "To example.com:team/repo.git\n ! [rejected]        main -> main (non-fast-forward)\nerror: failed to push some refs to 'example.com:team/repo.git'\n";
        assert_eq!(classify_git_message(stderr), Some(KnownError::PushRejected));
        let behind = "! [rejected] main -> main (fetch first)\nerror: Updates were rejected because the tip of your current branch is behind\n";
        assert_eq!(classify_git_message(behind), Some(KnownError::PushRejected));
    }

    #[test]
    fn a_ff_only_pull_failure_is_recognised() {
        assert_eq!(
            classify_git_message(
                "hint: You have divergent branches and need to specify how to reconcile them.\nfatal: Not possible to fast-forward, aborting.\n"
            ),
            Some(KnownError::PullNotFastForward)
        );
    }

    #[test]
    fn unknown_stderr_is_not_classified() {
        assert_eq!(classify_git_message("fatal: bad object HEAD"), None);
        assert_eq!(classify_git_message(""), None);
    }

    #[test]
    fn git_errors_are_classified_through_the_wrapper() {
        let error = git_core::Error::Git {
            command: "switch",
            code: Some(128),
            stderr: "fatal: 'topic' is already used by worktree at '/tmp/topic'\n".to_owned(),
        };
        assert!(matches!(
            classify_git_error(&error),
            Some(KnownError::BranchInWorktree { .. })
        ));
        assert_eq!(
            raw_output(&error),
            "fatal: 'topic' is already used by worktree at '/tmp/topic'"
        );
        assert_eq!(classify_git_error(&git_core::Error::Parse("refs")), None);
    }
}
