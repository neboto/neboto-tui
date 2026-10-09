use crate::error::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
// (#151). `with_terminal` lends the terminal out for the duration of a future.
//
// The rule that makes this work: **every tty change happens before the
// process starts, and none while it runs.** pinentry-curses saves the tty
// modes it finds and restores them on exit, so switching raw → cooked
// underneath a running pinentry (as #161 did after a grace timer) leaves it
// echoing the passphrase into a line-buffered tty and then "restoring" raw
// mode; printing a note while it draws interleaves the two escape streams.

/// Set while a `Tui` owns the terminal (raw mode + alternate screen).
static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Set while the terminal is lent out: no drawing, no input polling.
static HANDED_OFF: AtomicBool = AtomicBool::new(false);
/// Set by the input reader while it is parked (not inside a `poll`/`read`).
/// The handoff waits for it, so no in-flight poll can take the prompt's keys.
static READER_PARKED: AtomicBool = AtomicBool::new(false);
/// The screen under ratatui's buffer is stale: clear before the next draw.
static NEEDS_CLEAR: AtomicBool = AtomicBool::new(false);
/// Held across a draw and across each mode switch, so the handoff never
/// switches modes halfway through a frame.
static TERM_LOCK: Mutex<()> = Mutex::new(());
/// One handoff at a time (two prompting providers would interleave).
static HANDOFF_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// What the status bar says while a handoff is pending or running — the
/// TUI draws it like any other status line, so nothing is printed raw.
static HANDOFF_NOTE: Mutex<Option<String>> = Mutex::new(None);
/// Frames drawn so far: the handoff waits for one drawn after it set
/// `HANDOFF_NOTE`, so the note is on screen before drawing pauses.
static FRAMES_DRAWN: AtomicU64 = AtomicU64::new(0);

/// How long the handoff waits for the input reader to park. The reader polls
/// in 100ms slices, so this only runs out if no reader is running.
const READER_PARK_TIMEOUT: Duration = Duration::from_millis(500);

/// How long the handoff waits for the main loop to draw the note. The loop
/// draws at least every tick (250ms); this runs out only when the handoff
/// was started from the main loop itself (an org-role assume validating
/// base credentials), which can't draw until the handoff returns.
const NOTE_FRAME_TIMEOUT: Duration = Duration::from_millis(400);

fn term_lock() -> MutexGuard<'static, ()> {
    TERM_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Called by the input reader before each poll: whether it should leave the
/// tty alone right now. Records whether it is parked for [`with_terminal`] —
/// cleared *before* the pause is checked, so a handoff that sees the reader
/// parked can never race a poll that is about to start.
pub fn input_paused() -> bool {
    READER_PARKED.store(false, Ordering::SeqCst);
    let paused = HANDED_OFF.load(Ordering::SeqCst);
    if paused {
        READER_PARKED.store(true, Ordering::SeqCst);
    }
    paused
}

/// The credential note for the status bar, while a handoff is pending or
/// running.
pub fn handoff_note() -> Option<String> {
    HANDOFF_NOTE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_handoff_note(note: Option<String>) {
    *HANDOFF_NOTE.lock().unwrap_or_else(|e| e.into_inner()) = note;
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
    FRAMES_DRAWN.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// Run `fut` with the terminal lent out. With no TUI up (startup, headless,
/// an editor or session already holding the terminal) it simply runs `fut`.
///
/// 1. Before `fut` starts: `note` goes in the status bar and the handoff
///    waits for the main loop to draw it, so it is a normal TUI status line,
///    never raw text printed over the screen. Then drawing pauses, and the
///    input reader is waited on until it is parked. Mouse reporting goes off
///    (its escape sequences would land in a prompt's input), the tty goes to
///    cooked mode, and the cursor is parked at the start of the row above
///    the status bar, where a plain `/dev/tty` prompt writes. It stays on the
///    alternate screen, so a process that answers from its cache shows only
///    the status line — leaving the alternate screen is what flashed (#151).
/// 2. While `fut` runs, nothing touches the tty.
/// 3. After: raw mode, the alternate screen again (pinentry-curses' exit
///    drops to the normal screen), mouse back on, leftover typed input
///    discarded (a stray Enter after the passphrase), and a full redraw.
pub async fn with_terminal<F, T>(note: &str, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let _serial = HANDOFF_SERIAL.lock().await;
    if !TUI_ACTIVE.load(Ordering::SeqCst) {
        return fut.await;
    }
    let drawn = FRAMES_DRAWN.load(Ordering::SeqCst);
    set_handoff_note(Some(note.to_string()));
    let deadline = tokio::time::Instant::now() + NOTE_FRAME_TIMEOUT;
    while FRAMES_DRAWN.load(Ordering::SeqCst) == drawn && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    HANDED_OFF.store(true, Ordering::SeqCst);
    let deadline = tokio::time::Instant::now() + READER_PARK_TIMEOUT;
    while !READER_PARKED.load(Ordering::SeqCst) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    {
        let _guard = term_lock();
        if TUI_ACTIVE.load(Ordering::SeqCst) {
            let rows = crossterm::terminal::size().map(|(_, h)| h).unwrap_or(1);
            let _ = execute!(
                io::stdout(),
                DisableMouseCapture,
                crossterm::cursor::MoveTo(0, rows.saturating_sub(2)),
                crossterm::cursor::Show
            );
            let _ = disable_raw_mode();
        }
    }

    let result = fut.await;

    {
        let _guard = term_lock();
        // An editor/session teardown in the meantime owns the terminal now;
        // its own `Tui::new` brings the screen back.
        if TUI_ACTIVE.load(Ordering::SeqCst) {
            let _ = enable_raw_mode();
            let _ = execute!(
                io::stdout(),
                EnterAlternateScreen,
                EnableMouseCapture,
                crossterm::cursor::Hide
            );
            // The reader is still parked, so nothing else is reading.
            while crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
                let _ = crossterm::event::read();
            }
            NEEDS_CLEAR.store(true, Ordering::SeqCst);
        }
        set_handoff_note(None);
        HANDED_OFF.store(false, Ordering::SeqCst);
    }
    result
}
