//! Where interactive `aws` sessions (SSM Session Manager, ECS Exec) open —
//! config `session_launch` / `--session-launch` (#146) — and the temp-file
//! credential handover the `window` mode uses.
//!
//! The rule from #120 / #143 holds in every mode: **no credential is ever
//! written into a launched command**. A new tmux / terminal window runs a
//! command string, and that string reaches shell history (the macOS
//! `do script` path), `ps` and the screen. The setting only decides *how* to
//! keep to the rule:
//!
//! - `auto` (default): tmux window → new terminal window → current terminal.
//!   A session on static env keys (no named profile) skips the window tiers,
//!   since only the inline child can inherit the keys unwritten.
//! - `window`: always a new tmux / terminal window. Static keys travel in a
//!   0600 temp file the window's shell sources and deletes, so the command
//!   holds only the file's path.
//! - `inline`: always the current terminal (the TUI suspends).

use std::io::Write;
use std::path::Path;

/// How long a handover file may outlive its launch before neboto deletes it
/// itself. The new shell normally removes it within a second or two; this
/// bounds the window when the terminal never got as far as running the
/// command (a window that failed after `spawn` returned).
pub const HANDOVER_TTL: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionLaunch {
    #[default]
    Auto,
    Window,
    Inline,
}

impl SessionLaunch {
    /// Parse the config / CLI value. Unknown values warn and fall back to
    /// `auto` — never fatal (the `export_formats` precedent).
    pub fn from_config(value: Option<&str>) -> (Self, Option<String>) {
        let Some(v) = value else {
            return (Self::Auto, None);
        };
        match v.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => (Self::Auto, None),
            "window" => (Self::Window, None),
            "inline" => (Self::Inline, None),
            other => (
                Self::Auto,
                Some(format!(
                    "session_launch: unknown value {other:?} (use auto, window, inline) — using auto"
                )),
            ),
        }
    }
}

/// What the launcher should try, decided from the mode and whether the
/// session's credentials are static env keys that a window can't inherit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchPlan {
    /// Try the window tiers; `handover` = pass the static keys through a
    /// temp file rather than not at all.
    Window { handover: bool },
    /// Run in the current terminal.
    Inline,
}

pub fn plan(mode: SessionLaunch, static_keys: bool) -> LaunchPlan {
    match mode {
        SessionLaunch::Inline => LaunchPlan::Inline,
        SessionLaunch::Auto if static_keys => LaunchPlan::Inline,
        SessionLaunch::Auto => LaunchPlan::Window { handover: false },
        SessionLaunch::Window => LaunchPlan::Window { handover: static_keys },
    }
}

/// The handover file's body: one `export K='V'` line per non-empty
/// credential variable (`is_secret`), sorted for determinism.
pub fn handover_contents(
    vars: impl Iterator<Item = (String, String)>,
    is_secret: impl Fn(&str) -> bool,
) -> String {
    let mut pairs: Vec<(String, String)> = vars
        .filter(|(k, v)| k.starts_with("AWS_") && is_secret(k) && !v.is_empty())
        .collect();
    pairs.sort();
    let mut out = String::new();
    for (k, v) in pairs {
        out.push_str(&format!("export {}='{}'\n", k, v.replace('\'', "'\\''")));
    }
    out
}

/// Write `contents` to a fresh temp file readable only by the user (0600 —
/// `tempfile` creates it that way, and the mode is re-asserted before any
/// secret is written). The returned `TempPath` deletes the file when dropped,
/// which is the backstop for a shell that never ran.
pub fn write_handover_file(contents: &str) -> std::io::Result<tempfile::TempPath> {
    let mut file = tempfile::Builder::new()
        .prefix("neboto-session-")
        .suffix(".sh")
        .tempfile()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(contents.as_bytes())?;
    file.flush()?;
    Ok(file.into_temp_path())
}

/// Prefix a window command so it sources the handover file and deletes it
/// before anything else runs: `. <file> && rm -f <file> && { <cmd>; }`. The
/// path is the only trace of the credentials in the command. The braces keep
/// a multi-statement `cmd` (the `export …;` prefix) behind the `&&`, so
/// nothing runs if the file couldn't be read.
pub fn with_handover(path: &Path, cmd: &str) -> String {
    let p = crate::aws::resource::shell_quote(&path.to_string_lossy());
    format!(". {p} && rm -f {p} && {{ {cmd}; }}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Iterator<Item = (String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn secret(k: &str) -> bool {
        k == "AWS_ACCESS_KEY_ID" || k.contains("SECRET") || k.ends_with("_TOKEN")
    }

    #[test]
    fn parses_modes_and_warns_on_unknown() {
        assert_eq!(SessionLaunch::from_config(None), (SessionLaunch::Auto, None));
        assert_eq!(SessionLaunch::from_config(Some("Window")).0, SessionLaunch::Window);
        assert_eq!(SessionLaunch::from_config(Some(" inline ")).0, SessionLaunch::Inline);
        let (m, w) = SessionLaunch::from_config(Some("tab"));
        assert_eq!(m, SessionLaunch::Auto);
        assert!(w.unwrap().contains("session_launch"));
    }

    #[test]
    fn plan_follows_the_mode() {
        use LaunchPlan::*;
        assert_eq!(plan(SessionLaunch::Auto, false), Window { handover: false });
        assert_eq!(plan(SessionLaunch::Auto, true), Inline);
        assert_eq!(plan(SessionLaunch::Window, false), Window { handover: false });
        assert_eq!(plan(SessionLaunch::Window, true), Window { handover: true });
        assert_eq!(plan(SessionLaunch::Inline, false), Inline);
        assert_eq!(plan(SessionLaunch::Inline, true), Inline);
    }

    #[test]
    fn handover_carries_only_nonempty_credentials() {
        let body = handover_contents(
            vars(&[
                ("AWS_SECRET_ACCESS_KEY", "s3cr3t"),
                ("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE"),
                ("AWS_SESSION_TOKEN", ""),
                ("AWS_REGION", "eu-west-1"),
                ("HOME", "/home/me"),
            ]),
            secret,
        );
        assert_eq!(
            body,
            "export AWS_ACCESS_KEY_ID='AKIAEXAMPLE'\nexport AWS_SECRET_ACCESS_KEY='s3cr3t'\n"
        );
    }

    #[test]
    fn handover_file_is_private_and_the_path_is_all_the_command_holds() {
        let body = "export AWS_SECRET_ACCESS_KEY='s3cr3t'\n";
        let path = write_handover_file(body).expect("temp file");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "mode {mode:o}");
        }
        let cmd = with_handover(&path, "aws ssm start-session --target i-1");
        assert!(!cmd.contains("s3cr3t"));
        let p = path.to_string_lossy().to_string();
        assert!(cmd.starts_with(&format!(". {}", crate::aws::resource::shell_quote(&p))));
        assert!(cmd.ends_with("&& { aws ssm start-session --target i-1; }"));
        // Dropping the TempPath is the backstop delete.
        drop(path);
        assert!(!Path::new(&p).exists());
    }

    #[cfg(unix)]
    #[test]
    fn sourcing_the_handover_sets_the_keys_and_removes_the_file() {
        let path = write_handover_file("export AWS_SECRET_ACCESS_KEY='it'\\''s'\n").unwrap();
        let p = path.to_string_lossy().to_string();
        let cmd = with_handover(&path, "printf %s \"$AWS_SECRET_ACCESS_KEY\"");
        let out = std::process::Command::new("sh").args(["-c", &cmd]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "it's");
        assert!(!Path::new(&p).exists(), "the shell deletes the file once read");
    }
}
