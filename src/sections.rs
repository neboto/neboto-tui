//! Section descriptors: one table per split-pane resource type from which
//! every consumer of section order derives — digit keys, Tab cycling,
//! reset-to-default, the snapshot walk, the tab bar, and the flat view. The
//! table is declared with the [`sections!`] macro next to the resource type
//! and exposed through `Resource::detail_sections()`; order agreement across
//! consumers holds by construction. See
//! docs/adr/0002-section-descriptor-table.md.

use tokio::sync::mpsc;

use crate::app::App;
use crate::event::Event;

/// A section's on-enter hook — fired whenever the section becomes active
/// (digit key, Tab cycle, reset on drill-in, flat-view trigger sweep,
/// bookmark restore). Must be idempotent: point it at a LazyMap-backed
/// `trigger_*` so repeated firing short-circuits.
pub type OnEnter = fn(&mut App, &mpsc::UnboundedSender<Event>);

/// One entry in a pane's section table.
pub struct SectionDef {
    pub label: &'static str,
    pub on_enter: Option<OnEnter>,
}

/// A resource type's ordered section table — the single source of truth for
/// its split pane's section wiring. The default section is always index 0.
pub struct SectionDescriptor {
    pub sections: &'static [SectionDef],
}

impl SectionDescriptor {
    pub fn len(&self) -> usize {
        self.sections.len()
    }

    /// Fire a section's on-enter hook, when it has one.
    pub fn enter(&self, idx: usize, app: &mut App, event_tx: &mpsc::UnboundedSender<Event>) {
        if let Some(hook) = self.sections.get(idx).and_then(|s| s.on_enter) {
            hook(app, event_tx);
        }
    }
}

/// Digit key for a section position: `1`–`9`, then `0` for a tenth section
/// (the sub-tab convention). Positions beyond ten have no key and are
/// reachable only by Tab.
pub fn key_for(idx: usize) -> Option<char> {
    match idx {
        0..=8 => char::from_digit(idx as u32 + 1, 10),
        9 => Some('0'),
        _ => None,
    }
}

/// Inverse of [`key_for`].
pub fn index_for_key(c: char) -> Option<usize> {
    match c {
        '1'..='9' => Some(c as usize - '1' as usize),
        '0' => Some(9),
        _ => None,
    }
}

/// Declare a split pane's section table: emits the renderer-local section
/// enum (with an index conversion that clamps out-of-range to the first
/// section, matching the app's clamping) and the `SectionDescriptor` static,
/// both from one list — variants, labels, and on-enter hooks cannot drift.
#[macro_export]
macro_rules! sections {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident,
        $svis:vis static $desc:ident = [
            $( $variant:ident $label:literal $( => $hook:expr )? ),+ $(,)?
        ]
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis enum $name { $( $variant, )+ }

        impl $name {
            /// Section index → variant; out of range falls back to the
            /// first (default) section.
            $vis fn from_index(idx: usize) -> Self {
                const ALL: &[$name] = &[ $( $name::$variant, )+ ];
                *ALL.get(idx).unwrap_or(&ALL[0])
            }
        }

        $svis static $desc: $crate::sections::SectionDescriptor =
            $crate::sections::SectionDescriptor {
                sections: &[
                    $( $crate::sections::SectionDef {
                        label: $label,
                        on_enter: $crate::sections!(@hook $( $hook )?),
                    }, )+
                ],
            };
    };
    (@hook) => { None };
    (@hook $hook:expr) => { Some($hook) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_keys_round_trip() {
        for idx in 0..10 {
            let key = key_for(idx).unwrap();
            assert_eq!(index_for_key(key), Some(idx));
        }
        assert_eq!(key_for(10), None);
        assert_eq!(index_for_key('x'), None);
    }
}
