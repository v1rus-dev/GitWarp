//! Per-window toast stack.
//!
//! Toasts are drawn by [`crate::overlay::render_overlays`] above every layer,
//! bottom-right and newest last. Errors stay until dismissed; every other kind
//! hides itself after five seconds.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{
    div, px, App, BorrowAppContext, Div, Global, ParentElement, SharedString, Styled, Window,
    WindowId,
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

/// One visible toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub id: ToastId,
    pub kind: ToastKind,
    pub message: SharedString,
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
        self.next_id += 1;
        let id = ToastId(self.next_id);
        self.entries.push(QueuedToast {
            toast: Toast {
                id,
                kind,
                message: message.into(),
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

/// Prunes the toasts of closed windows.
pub(crate) fn init(cx: &mut App) {
    cx.on_window_closed(|cx| {
        let live: HashSet<WindowId> = cx
            .windows()
            .iter()
            .map(|handle| handle.window_id())
            .collect();
        if cx.try_global::<ToastRegistry>().is_some() {
            cx.update_global::<ToastRegistry, _>(|registry, _| {
                registry.by_window.retain(|id, _| live.contains(id));
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
    let now = cx.background_executor().now();
    let id = with_queue(window, cx, |queue| queue.push(kind, message, now));
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
    div()
        .relative()
        .flex()
        .items_center()
        .gap(px(8.))
        .min_w(px(260.))
        .max_w(px(380.))
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
        )
        .child(Icon::new(toast.kind.icon()).size(px(14.)).color(color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(FS_BASE)
                .text_color(colors.text)
                .child(toast.message.clone()),
        )
        .child(
            Button::ghost(gpui::ElementId::Name(
                format!("toast-dismiss-{}", id.0).into(),
            ))
            .icon_only(IconName::X)
            .size(ButtonSize::Xs)
            .tooltip("Dismiss")
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
}
