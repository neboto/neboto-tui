//! Copying from a TUI that may be running over SSH (from nebaz's
//! `src/clipboard.rs`).
//!
//! Two sinks, both tried on every copy:
//!
//! - **OSC 52**: the escape sequence terminals accept to set the clipboard
//!   of the machine the terminal runs on. Over SSH this is the only way
//!   the text reaches the user's own clipboard; inside tmux it is wrapped
//!   in the DCS passthrough. A terminal that does not support it ignores
//!   the bytes, so it is always sent.
//! - **The system clipboard** through `arboard`, kept alive in one
//!   long-lived handle. On Linux the writer *is* the clipboard owner, so a
//!   handle dropped right after writing loses the text and `arboard`
//!   prints a warning to stderr, straight over the TUI. Holding the handle
//!   for the app's lifetime avoids both.

use base64::Engine;
use std::io::{IsTerminal, Write};

/// Which sink took the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sink {
    /// The system clipboard (and OSC 52 was sent too).
    System,
    /// Only OSC 52 went out: no system clipboard here (headless, SSH).
    Terminal,
}

impl Sink {
    /// What the copy toast adds: nothing for the system clipboard; for OSC 52
    /// alone, that it went through the terminal — which may ignore it.
    pub fn note(self) -> &'static str {
        match self {
            Sink::System => "",
            Sink::Terminal => " (via the terminal, OSC 52)",
        }
    }
}

pub struct Clipboard {
    system: Option<arboard::Clipboard>,
    system_error: Option<String>,
    /// The last text copied. Under `cfg(test)` it is the only sink, so a
    /// test run never touches the developer's clipboard; the wiring harness
    /// reads it back.
    #[cfg_attr(not(test), allow(dead_code))]
    last: Option<String>,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    pub fn new() -> Self {
        Self {
            system: None,
            system_error: None,
            last: None,
        }
    }

    /// Copy `text` to every sink available. `Err` only when neither is:
    /// OSC 52 needs stdout to be a terminal.
    pub fn copy(&mut self, text: &str) -> Result<Sink, String> {
        self.last = Some(text.to_string());
        if cfg!(test) {
            return Ok(Sink::System);
        }
        let terminal = write_osc52(text).is_ok();
        if self.system.is_none() && self.system_error.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => self.system = Some(c),
                Err(e) => self.system_error = Some(e.to_string()),
            }
        }
        let system = match self.system.as_mut() {
            Some(c) => match c.set_text(text.to_string()) {
                Ok(()) => true,
                Err(e) => {
                    self.system_error = Some(e.to_string());
                    false
                }
            },
            None => false,
        };
        match (system, terminal) {
            (true, _) => Ok(Sink::System),
            (false, true) => Ok(Sink::Terminal),
            (false, false) => Err(self
                .system_error
                .clone()
                .unwrap_or_else(|| "no clipboard available".into())),
        }
    }
}

/// The OSC 52 sequence for `text`, wrapped for tmux when `inside_tmux`.
pub fn osc52(text: &str, inside_tmux: bool) -> Vec<u8> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let seq = format!("\x1b]52;c;{}\x07", b64);
    if inside_tmux {
        // DCS passthrough: ESC inside the payload is doubled.
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b")).into_bytes()
    } else {
        seq.into_bytes()
    }
}

/// Send the OSC 52 sequence to the terminal on stdout. `Err` when stdout
/// is not a terminal (tests, pipes).
fn write_osc52(text: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout();
    if !out.is_terminal() {
        return Err(std::io::Error::other("stdout is not a terminal"));
    }
    let inside_tmux = std::env::var_os("TMUX").is_some();
    out.write_all(&osc52(text, inside_tmux))?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Clipboard {
        /// What the last copy put on the clipboard (the wiring harness).
        pub(crate) fn last(&self) -> Option<&str> {
            self.last.as_deref()
        }
    }

    #[test]
    fn osc52_encodes_base64_and_wraps_for_tmux() {
        assert_eq!(osc52("hi", false), b"\x1b]52;c;aGk=\x07".to_vec());
        assert_eq!(
            osc52("hi", true),
            b"\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\".to_vec()
        );
    }

    #[test]
    fn a_test_build_copies_to_no_real_sink() {
        let mut c = Clipboard::new();
        assert_eq!(c.copy("id"), Ok(Sink::System));
        assert_eq!(c.last(), Some("id"));
        assert!(c.system.is_none(), "arboard is never opened under test");
    }
}
