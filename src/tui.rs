use crate::error::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
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
        TUI_ACTIVE.store(true, Ordering::SeqCst);
        Ok(Self { terminal })
    }

    pub fn terminal(&mut self) -> &mut TerminalType {
        &mut self.terminal
    }

    pub fn restore(&mut self) -> Result<()> {
        TUI_ACTIVE.store(false, Ordering::SeqCst);

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

// ── Terminal handoff ────────────────────────────────────────────────────────
//
// A `credential_process` can need the terminal: granted with the `pass`
// backend decrypts through gpg, and gpg-agent puts `pinentry-curses` on our
// tty to ask for the passphrase. The SDK only runs the process when a request
// first needs credentials, which is long after the TUI owns the screen, so
// pinentry and ratatui used to draw over each other and race for keystrokes
// (#151). `with_terminal` lends the terminal out for the duration of a future:
// drawing and the input reader pause, and — only if the future outlives a
// short grace period — the tty goes to cooked mode with a note, and the TUI
// comes back with a full redraw.

/// Set while a `Tui` owns the terminal (raw mode + alternate screen).
static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Set while the terminal is lent out: no drawing, no input polling.
static HANDED_OFF: AtomicBool = AtomicBool::new(false);
/// The screen under ratatui's buffer is stale: clear before the next draw.
static NEEDS_CLEAR: AtomicBool = AtomicBool::new(false);
/// Held across a draw and across each mode switch, so the handoff never
/// leaves the alternate screen halfway through a frame.
static TERM_LOCK: Mutex<()> = Mutex::new(());
/// One handoff at a time (two prompting providers would interleave).
static HANDOFF_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn term_lock() -> MutexGuard<'static, ()> {
    TERM_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the input reader should leave the tty alone right now.
pub fn input_paused() -> bool {
    HANDED_OFF.load(Ordering::SeqCst)
}

/// Draw one frame unless the terminal is lent out. Clears first when the
/// screen came back from a handoff.
pub fn draw<F>(tui: &mut Tui, render: F) -> Result<()>
where
    F: FnOnce(&mut ratatui::Frame),
{
    let _guard = term_lock();
    if HANDED_OFF.load(Ordering::SeqCst) {
        return Ok(());
    }
    if NEEDS_CLEAR.swap(false, Ordering::SeqCst) {
        tui.terminal.clear()?;
    }
    tui.terminal.draw(render)?;
    Ok(())
}

fn bottom_row() -> u16 {
    crossterm::terminal::size().map(|(_, h)| h.saturating_sub(1)).unwrap_or(0)
}

/// How long a lent-out future may run before the handoff becomes visible. A
/// `credential_process` serving cached credentials (granted with a warm
/// cache, the common case on a profile switch) answers well inside this, so
/// it never flashes the screen; one that is still running is prompting or
/// about to.
const HANDOFF_GRACE: Duration = Duration::from_millis(300);

/// Run `fut` with the terminal lent out. With no TUI up (startup, headless,
/// an editor or session already holding the terminal) it simply runs `fut`.
///
/// Two stages, so a fast process never shows anything (#151 follow-up: every
/// profile switch flashed the shell and the note):
/// 1. Invisible: drawing and the input reader pause and mouse reporting goes
///    off (its escape sequences would land in a prompt's input). The screen
///    is untouched.
/// 2. After [`HANDOFF_GRACE`], if `fut` is still running: cooked mode, the
///    cursor shown, and `note` printed on the top row. This stays on the
///    alternate screen on purpose — a `pinentry-curses` that started inside
///    the grace period has already drawn there, and leaving would hide it.
///    A plain `/dev/tty` prompt writes on the bottom row, where stage 1
///    parked the cursor.
pub async fn with_terminal<F, T>(note: &str, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let _serial = HANDOFF_SERIAL.lock().await;
    if !TUI_ACTIVE.load(Ordering::SeqCst) {
        return fut.await;
    }
    {
        let _guard = term_lock();
        HANDED_OFF.store(true, Ordering::SeqCst);
        // Park the (hidden) cursor bottom-left, so a prompt that writes to
        // the tty lands on its own row rather than mid-frame.
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            crossterm::cursor::MoveTo(0, bottom_row())
        );
    }

    tokio::pin!(fut);
    let (result, visible) = tokio::select! {
        r = &mut fut => (r, false),
        _ = tokio::time::sleep(HANDOFF_GRACE) => {
            {
                let _guard = term_lock();
                // The note goes on the top row and the cursor goes back to
                // where the prompt may already have written its question.
                let _ = execute!(
                    io::stdout(),
                    crossterm::cursor::SavePosition,
                    crossterm::cursor::MoveTo(0, 0),
                    crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine),
                    crossterm::style::Print(format!("neboto: {note}")),
                    crossterm::cursor::RestorePosition,
                    crossterm::cursor::Show
                );
                let _ = disable_raw_mode();
            }
            (fut.await, true)
        }
    };

    {
        let _guard = term_lock();
        // An editor/session teardown in the meantime owns the terminal now;
        // its own `Tui::new` brings the screen back.
        if TUI_ACTIVE.load(Ordering::SeqCst) {
            if visible {
                let _ = enable_raw_mode();
                // Re-enter: a curses prompt's exit (`rmcup`) drops us back to
                // the normal screen. The note and any prompt text are on the
                // screen either way, so redraw from scratch.
                let _ = execute!(
                    io::stdout(),
                    EnterAlternateScreen,
                    EnableMouseCapture,
                    crossterm::cursor::Hide
                );
                NEEDS_CLEAR.store(true, Ordering::SeqCst);
            } else {
                let _ = execute!(io::stdout(), EnableMouseCapture);
            }
        }
        HANDED_OFF.store(false, Ordering::SeqCst);
    }
    result
}
