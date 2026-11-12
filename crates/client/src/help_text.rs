//! How much the UI explains: the "Help text" setting (the settings screen's Game tab,
//! `ClientSettings::help_text`, `--help-text full|short|off` for a session).
//!
//! Cards and panels say a good deal about what things are and how they work, which is what a new
//! player needs and what someone who has played a lot reads past. Three levels:
//!
//! - **Full** (as shipped): everything.
//! - **Short**: the explaining sentences go ([`explains`] is false): a card keeps its name, its
//!   numbers and what it needs, and drops the line on what it is for or how it works.
//! - **Off**: as Short, and the reminders of what keys and clicks do go too ([`keys`] is false):
//!   the footers' key hints (`theme::flat::hint` draws nothing), the first night's hint line.
//!
//! What a thing *needs* and what *went wrong* are never help text: a recipe's missing inputs, a
//! refusal's reason, a wound's treatment stay at every level.
//!
//! **Using it:** anything that draws explaining text asks [`explains`] (or wraps the text in
//! [`explain`]) and anything that draws a key reminder asks [`keys`]; a panel that is only
//! rebuilt when what it shows changes puts [`level`] in what it compares. The level is kept in
//! one process-wide cell rather than a resource so that the small spawn helpers every panel
//! shares can ask without each caller handing it down; [`set`] is called at the start (from the
//! settings or the flag) and by the settings screen when it changes.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum HelpText {
    #[default]
    Full,
    Short,
    Off,
}

impl HelpText {
    pub const ALL: [HelpText; 3] = [HelpText::Full, HelpText::Short, HelpText::Off];

    pub fn name(self) -> &'static str {
        match self {
            HelpText::Full => "Full",
            HelpText::Short => "Short",
            HelpText::Off => "Off",
        }
    }

    pub fn parse(text: &str) -> Option<HelpText> {
        HelpText::ALL.into_iter().find(|h| h.name().eq_ignore_ascii_case(text.trim()))
    }

    /// Sentences that explain what a thing is for or how it works are shown.
    pub fn explains(self) -> bool {
        self == HelpText::Full
    }

    /// Reminders of what keys and clicks do are shown.
    pub fn keys(self) -> bool {
        self != HelpText::Off
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(0);

pub fn set(level: HelpText) {
    LEVEL.store(level as u8, Ordering::Relaxed);
}

pub fn level() -> HelpText {
    HelpText::ALL[(LEVEL.load(Ordering::Relaxed) as usize).min(2)]
}

/// See [`HelpText::explains`], for the level in force.
pub fn explains() -> bool {
    level().explains()
}

/// See [`HelpText::keys`], for the level in force.
pub fn keys() -> bool {
    level().keys()
}

/// `text` if explaining text is shown.
pub fn explain<T>(text: T) -> Option<T> {
    explains().then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_level_shows_less() {
        assert!(HelpText::Full.explains() && HelpText::Full.keys());
        assert!(!HelpText::Short.explains() && HelpText::Short.keys());
        assert!(!HelpText::Off.explains() && !HelpText::Off.keys());
        assert_eq!(HelpText::parse(" short "), Some(HelpText::Short));
        assert_eq!(HelpText::parse("verbose"), None);
        assert_eq!(HelpText::default(), HelpText::Full);
    }
}
