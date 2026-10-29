//! The component kit.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{ClickEvent, Font, FontFeatures, Window};

use crate::metrics::FONT_UI;

pub(crate) type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static>;
pub(crate) type ToggleHandler = Box<dyn Fn(bool, &mut Window, &mut gpui::App) + 'static>;
pub(crate) type SelectHandler = Rc<dyn Fn(usize, &mut Window, &mut gpui::App) + 'static>;

mod avatar;
mod banner;
mod button;
mod chip;
mod counter;
pub mod dialog;
mod empty_state;
mod file_row;
mod list_row;
pub mod menu;
mod progress;
mod ref_badge;
mod scrollbar;
mod search_field;
mod segmented;
pub mod select;
mod skeleton;
mod spinner;
mod splitter;
mod status_glyph;
mod tabs;
mod text_input;
pub mod toast;
mod toggles;
mod tooltip;
mod tree;

pub use avatar::Avatar;
pub use banner::{Banner, BannerKind};
pub use button::{Button, ButtonSize, ButtonVariant};
pub use chip::{Chip, ChipKind};
pub use counter::{CountPill, Counter, Kbd};
pub use dialog::{Dialog, DialogFocus, DialogFocusKind, DialogHost};
pub use empty_state::EmptyState;
pub use file_row::{fit_path_start, FileRow, FittedPath};
pub use list_row::ListRow;
pub use menu::{MenuEntry, MenuItem, MenuNav};
pub use progress::Progress;
pub use ref_badge::{RefBadge, RefKind};
pub use scrollbar::{thumb_metrics, ScrollArea, ScrollHandleSource, Scrollbar, ThumbMetrics};
pub use search_field::SearchField;
pub use segmented::{SegmentItem, Segmented};
pub use select::Select;
pub use skeleton::Skeleton;
pub use spinner::Spinner;
pub use splitter::{clamp_size, SplitAxis, Splitter};
pub use status_glyph::StatusGlyph;
pub use tabs::{Tab, Tabs};
pub use text_input::{Submit as TextInputSubmit, TextInput, TextInputEvent};
pub use toast::{Toast, ToastId, ToastKind, ToastQueue};
pub use toggles::{Checkbox, Radio, Switch};
pub use tooltip::Tooltip;
pub use tree::{Tree, TreeNodeData, TreeNodeTrailing};

pub(crate) use text_input::bind_keys as bind_text_input_keys;

/// UI text with tabular figures, for counts and ahead/behind markers.
pub(crate) fn tabular_font() -> Font {
    let mut font = gpui::font(FONT_UI);
    font.features = FontFeatures(Arc::new(vec![("tnum".to_owned(), 1)]));
    font
}
