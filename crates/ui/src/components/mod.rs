//! The component kit.

use std::rc::Rc;

use gpui::{ClickEvent, Window};

pub(crate) type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static>;
pub(crate) type ToggleHandler = Box<dyn Fn(bool, &mut Window, &mut gpui::App) + 'static>;
pub(crate) type SelectHandler = Rc<dyn Fn(usize, &mut Window, &mut gpui::App) + 'static>;

mod avatar;
mod banner;
mod button;
mod chip;
mod counter;
mod empty_state;
mod progress;
mod ref_badge;
mod segmented;
mod skeleton;
mod spinner;
mod status_glyph;
mod tabs;
mod toggles;

pub use avatar::Avatar;
pub use banner::{Banner, BannerKind};
pub use button::{Button, ButtonSize, ButtonVariant};
pub use chip::{Chip, ChipKind};
pub use counter::{CountPill, Counter, Kbd};
pub use empty_state::EmptyState;
pub use progress::Progress;
pub use ref_badge::{RefBadge, RefKind};
pub use segmented::{SegmentItem, Segmented};
pub use skeleton::Skeleton;
pub use spinner::Spinner;
pub use status_glyph::StatusGlyph;
pub use tabs::{Tab, Tabs};
pub use toggles::{Checkbox, Radio, Switch};
