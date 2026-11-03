//! Per-window toast stack.
//!
//! Toasts are drawn by [`crate::overlay::render_overlays`] above every layer,
//! bottom-right and newest last. Errors stay until dismissed; every other kind
//! hides itself after five seconds.

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, BorrowAppContext, Div, FontWeight, Global, ParentElement, SharedString, Styled,
    Window, WindowId,
};

use crate::components::{Button, ButtonSize};
use crate::icon::{Icon, IconName};
use crate::metrics::{FS_BASE, R_MD};
use crate::theme::{ActiveTheme, ThemeColors};

/// How long an auto-hiding toast stays up.
pub const TOAST_LIFETIME: Duration = Duration::from_secs(5);

/// Identifies one toast in one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ToastId(u64);

/// The four toast kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Ok,
    Info,
    Warn,
    Error,
}

impl ToastKind {
    fn auto_hide(self) -> bool {
        !matches!(self, Self::Error)
    }

    fn icon(self) -> IconName {
        match self {
            Self::Ok => IconName::Check,
            Self::Info => IconName::Info,
            Self::Warn => IconName::Warn,
            Self::Error => IconName::Err,
        }
    }

    fn color(self, colors: ThemeColors) -> gpui::Hsla {
        match self {
            Self::Ok => colors.success,
            Self::Info => colors.info,
            Self::Warn => colors.warning,
            Self::Error => colors.error,
        }
    }
}

/// What a toast button runs.
pub type ToastHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A small button inside a toast, e.g. "Reconnect". Clicking it dismisses the
/// toast and runs the handler.
#[derive(Clone)]
pub struct ToastAction {
    pub label: SharedString,
    pub on_click: ToastHandler,
}

impl ToastAction {
    pub fn new(
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            on_click: Rc::new(on_click),
        }
    }
}

impl fmt::Debug for ToastAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToastAction")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ToastAction {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label && Rc::ptr_eq(&self.on_click, &other.on_click)
    }
}

impl Eq for ToastAction {}

/// One line of a toast's list: a result icon, a text, an optional muted
/// detail and an optional button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastLine {
    /// Check icon when `true`, error icon otherwise.
    pub ok: bool,
    pub text: SharedString,
    pub detail: Option<SharedString>,
    pub action: Option<ToastAction>,
}

/// What a toast shows besides its message (the title): a list of lines, a
/// button next to the message, or both.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToastBody {
    pub lines: Vec<ToastLine>,
    pub action: Option<ToastAction>,
}

/// One visible toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub id: ToastId,
    pub kind: ToastKind,
    pub message: SharedString,
    pub body: Option<ToastBody>,
}

struct QueuedToast {
    toast: Toast,
    deadline: Option<Instant>,
}

/// The pure toast state of one window: ordering, expiry and dismissal.
#[derive(Default)]
pub struct ToastQueue {
    entries: Vec<QueuedToast>,
    next_id: u64,
}

impl ToastQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a toast; newest is last.
    pub fn push(
        &mut self,
        kind: ToastKind,
        message: impl Into<SharedString>,
        now: Instant,
    ) -> ToastId {
        self.push_with(kind, message, None, now)
    }

    /// [`ToastQueue::push`] with a list and/or a button under the message.
    pub fn push_with(
        &mut self,
        kind: ToastKind,
        message: impl Into<SharedString>,
        body: Option<ToastBody>,
        now: Instant,
    ) -> ToastId {
        self.next_id += 1;
        let id = ToastId(self.next_id);
        self.entries.push(QueuedToast {
            toast: Toast {
                id,
                kind,
                message: message.into(),
                body,
            },
            deadline: kind.auto_hide().then(|| now + TOAST_LIFETIME),
        });
        id
    }

    /// Removes `id`; returns whether it was still visible.
    pub fn dismiss(&mut self, id: ToastId) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.toast.id != id);
        self.entries.len() != before
    }

    /// Removes every toast whose lifetime ran out at `now`.
    pub fn expire(&mut self, now: Instant) -> Vec<ToastId> {
        let mut expired = Vec::new();
        self.entries.retain(|entry| {
            let live = entry.deadline.is_none_or(|deadline| deadline > now);
            if !live {
                expired.push(entry.toast.id);
            }
            live
        });
        expired
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Toast> {
        self.entries.iter().map(|entry| &entry.toast)
    }
}

#[derive(Default)]
struct ToastRegistry {
    by_window: HashMap<WindowId, ToastQueue>,
}

impl Global for ToastRegistry {}

/// Prunes the toasts of a window that closed.
pub(crate) fn init(cx: &mut App) {
    cx.on_window_closed(|cx, window_id| {
        if cx.try_global::<ToastRegistry>().is_some() {
            cx.update_global::<ToastRegistry, _>(|registry, _| {
                registry.by_window.remove(&window_id);
            });
        }
    })
    .detach();
}

fn with_queue<R>(window: &Window, cx: &mut App, f: impl FnOnce(&mut ToastQueue) -> R) -> R {
    let window_id = window.window_handle().window_id();
    if cx.try_global::<ToastRegistry>().is_none() {
        cx.set_global(ToastRegistry::default());
    }
    cx.update_global::<ToastRegistry, _>(|registry, _| {
        f(registry.by_window.entry(window_id).or_default())
    })
}

/// Shows a toast and schedules its hide, except for errors.
pub fn show(
    kind: ToastKind,
    message: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> ToastId {
    show_with(kind, message, None, window, cx)
}

/// [`show`] with a list of lines and/or a button under the message.
pub fn show_with(
    kind: ToastKind,
    message: impl Into<SharedString>,
    body: Option<ToastBody>,
    window: &mut Window,
    cx: &mut App,
) -> ToastId {
    let now = cx.background_executor().now();
    let id = with_queue(window, cx, |queue| {
        queue.push_with(kind, message, body, now)
    });
    window.refresh();

    if kind.auto_hide() {
        let window_handle = window.window_handle();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(TOAST_LIFETIME).await;
            window_handle
                .update(cx, |_, window, cx| dismiss(id, window, cx))
                .ok();
        })
        .detach();
    }
    id
}

/// Removes one toast; returns whether it was visible.
pub fn dismiss(id: ToastId, window: &mut Window, cx: &mut App) -> bool {
    let removed = with_queue(window, cx, |queue| queue.dismiss(id));
    if removed {
        window.refresh();
    }
    removed
}

/// Number of visible toasts in `window`.
pub fn count(window: &Window, cx: &App) -> usize {
    let window_id = window.window_handle().window_id();
    cx.try_global::<ToastRegistry>()
        .and_then(|registry| registry.by_window.get(&window_id))
        .map_or(0, ToastQueue::len)
}

/// Copies of the visible toasts, oldest first.
pub fn toasts(window: &Window, cx: &App) -> Vec<Toast> {
    let window_id = window.window_handle().window_id();
    cx.try_global::<ToastRegistry>()
        .and_then(|registry| registry.by_window.get(&window_id))
        .map(|queue| queue.iter().cloned().collect())
        .unwrap_or_default()
}

/// The bottom-right toast stack, or `None` when the window has no toasts.
pub(crate) fn render_stack(window: &Window, cx: &App) -> Option<Div> {
    let window_id = window.window_handle().window_id();
    let queue = cx
        .try_global::<ToastRegistry>()?
        .by_window
        .get(&window_id)?;
    if queue.is_empty() {
        return None;
    }

    let colors = cx.theme().colors;
    let shadows = cx.theme().pop_shadow();
    let mut stack = div()
        .absolute()
        .right(px(12.))
        .bottom(px(12.))
        .flex()
        .flex_col()
        .items_end()
        .gap(px(8.));
    for toast in queue.iter() {
        stack = stack.child(render_toast(toast, colors, shadows.clone()));
    }
    Some(stack)
}

fn render_toast(toast: &Toast, colors: ThemeColors, shadows: Vec<gpui::BoxShadow>) -> Div {
    let color = toast.kind.color(colors);
    let id = toast.id;
    let body = toast.body.as_ref();
    let has_lines = body.is_some_and(|body| !body.lines.is_empty());
    let action_button = |action: &ToastAction, key: String, size: ButtonSize| {
        let on_click = action.on_click.clone();
        Button::new(gpui::ElementId::Name(key.into()))
            .size(size)
            .label(action.label.clone())
            .on_click(move |_, window, cx| {
                dismiss(id, window, cx);
                on_click(window, cx);
            })
    };

    let mut message = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .flex_1()
        .min_w_0()
        .child(
            div()
                .text_size(FS_BASE)
                .text_color(colors.text)
                .when(has_lines, |title| title.font_weight(FontWeight::SEMIBOLD))
                .child(toast.message.clone()),
        );
    if let Some(body) = body {
        for (index, line) in body.lines.iter().enumerate() {
            let line_color = if line.ok {
                colors.success
            } else {
                colors.error
            };
            let icon = if line.ok {
                IconName::Check
            } else {
                IconName::Err
            };
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(FS_BASE)
                .child(Icon::new(icon).size(px(12.)).color(line_color))
                .child(div().text_color(colors.text).child(line.text.clone()));
            if let Some(detail) = &line.detail {
                row = row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(colors.muted)
                        .child(detail.clone()),
                );
            }
            if let Some(action) = &line.action {
                row = row.child(action_button(
                    action,
                    format!("toast-{}-line-{index}", id.0),
                    ButtonSize::Xs,
                ));
            }
            message = message.child(row);
        }
    }

    let mut toast_el = div()
        .relative()
        .flex()
        .items_center()
        .gap(px(8.))
        .min_w(px(260.))
        .max_w(if body.is_some() { px(420.) } else { px(380.) })
        .py(px(8.))
        .pr(px(8.))
        .pl(px(12.))
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border_strong)
        .rounded(R_MD)
        .overflow_hidden()
        .shadow(shadows)
        .child(
            div()
                .absolute()
                .left(px(0.))
                .top(px(0.))
                .bottom(px(0.))
                .w(px(3.))
                .bg(color),
        );
    if has_lines {
        // A list: the icon stays beside the title, not centred on the list.
        toast_el = toast_el.items_start();
    }
    toast_el = toast_el
        .child(Icon::new(toast.kind.icon()).size(px(14.)).color(color))
        .child(message);
    if let Some(action) = body.and_then(|body| body.action.as_ref()) {
        toast_el = toast_el.child(action_button(
            action,
            format!("toast-action-{}", id.0),
            ButtonSize::Xs,
        ));
    }
    toast_el.child(
        Button::ghost(gpui::ElementId::Name(
            format!("toast-dismiss-{}", id.0).into(),
        ))
        .icon_only(IconName::X)
        .size(ButtonSize::Xs)
        .tooltip(crate::i18n::t("kit.toast.dismiss"))
        .on_click(move |_, window, cx| {
            dismiss(id, window, cx);
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_hiding_kinds_expire_and_errors_do_not() {
        let mut queue = ToastQueue::new();
        let now = Instant::now();
        let ok = queue.push(ToastKind::Ok, "saved", now);
        let info = queue.push(ToastKind::Info, "fetched", now);
        let warn = queue.push(ToastKind::Warn, "skipped", now);
        let error = queue.push(ToastKind::Error, "failed", now);
        assert_eq!(queue.len(), 4);

        let expired = queue.expire(now + TOAST_LIFETIME);
        assert_eq!(expired, vec![ok, info, warn]);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.iter().next().map(|toast| toast.id), Some(error));
    }

    #[test]
    fn newest_is_last_and_dismiss_removes_one() {
        let mut queue = ToastQueue::new();
        let now = Instant::now();
        let first = queue.push(ToastKind::Info, "one", now);
        let second = queue.push(ToastKind::Info, "two", now);
        let messages: Vec<_> = queue
            .iter()
            .map(|toast| toast.message.to_string())
            .collect();
        assert_eq!(messages, vec!["one", "two"]);

        assert!(queue.dismiss(first));
        assert!(!queue.dismiss(first));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.iter().next().map(|toast| toast.id), Some(second));
    }

    #[test]
    fn nothing_expires_before_the_deadline() {
        let mut queue = ToastQueue::new();
        let now = Instant::now();
        queue.push(ToastKind::Ok, "saved", now);
        assert!(queue.expire(now + Duration::from_millis(4999)).is_empty());
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn a_toast_can_carry_lines_and_a_button() {
        let mut queue = ToastQueue::new();
        let now = Instant::now();
        let body = ToastBody {
            lines: vec![ToastLine {
                ok: false,
                text: "codeberg.org · mira-k".into(),
                detail: Some("Token invalid or revoked".into()),
                action: Some(ToastAction::new("Reconnect", |_, _| {})),
            }],
            action: None,
        };
        let id = queue.push_with(
            ToastKind::Error,
            "Pushed main to 1 of 2 targets",
            Some(body),
            now,
        );
        let toast = queue.iter().next().unwrap();
        assert_eq!(toast.id, id);
        let lines = &toast.body.as_ref().unwrap().lines;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].action.as_ref().unwrap().label, "Reconnect");
        // Errors with a list stay until dismissed, like every error.
        assert!(queue.expire(now + TOAST_LIFETIME * 10).is_empty());
        // A plain push has no body.
        queue.push(ToastKind::Ok, "saved", now);
        assert!(queue.iter().nth(1).unwrap().body.is_none());
    }
}
