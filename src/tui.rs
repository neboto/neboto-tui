use crate::error::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::thread;
use std::time::Duration;

pub type TerminalType = Terminal<CrosstermBackend<io::Stdout>>;

pub struct Tui {
    terminal: TerminalType,
}

impl Tui {
    pub fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }

    pub fn terminal(&mut self) -> &mut TerminalType {
        &mut self.terminal
    }

    pub fn restore(&mut self) -> Result<()> {
        // Show cursor first
        self.terminal.show_cursor()?;

        // Disable raw mode
        disable_raw_mode()?;

        // Leave alternate screen and disable mouse capture
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;

        // CRITICAL: Flush the backend to ensure all commands are processed
        io::Write::flush(self.terminal.backend_mut())?;

        // Give the terminal time to fully process the mode changes
        // Without this, vim may start before the terminal is ready
        thread::sleep(Duration::from_millis(100));

        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
