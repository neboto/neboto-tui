//! `|`: open a second neboto beside this one, at the same place — in a new
//! pane of whichever multiplexer / terminal neboto is running in.
//!
//! The new process is told where to go with `--at <place>`: the current
//! `NavLocation` (service, sub-tab, row, section, lens, region, account) as
//! base64url JSON, restored at startup the way a bookmark is. The session
//! flags (`-r`, `-p`, `--endpoint-url`, `--demo`, `-w`) travel beside it,
//! because a new pane's shell starts from the terminal's environment, not
//! neboto's. Credentials follow the `session_launch` rules: never written
//! into the command (see `crate::session_launch`).

use crate::app::NavLocation;
use base64::Engine;

/// Somewhere that can open a pane next to this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Tmux,
    Zellij,
    WezTerm,
    Kitty,
    ITerm,
}

impl Host {
    pub fn name(self) -> &'static str {
        match self {
            Host::Tmux => "tmux",
            Host::Zellij => "zellij",
            Host::WezTerm => "WezTerm",
            Host::Kitty => "kitty",
            Host::ITerm => "iTerm",
        }
    }
}

/// The host neboto runs in, innermost first: a multiplexer inside a terminal
/// splits the multiplexer's pane (WezTerm's variables are inherited into
/// the tmux it runs, so the order matters).
pub fn detect(env: impl Fn(&str) -> Option<String>) -> Option<Host> {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    if set("TMUX") {
        Some(Host::Tmux)
    } else if set("ZELLIJ") {
        Some(Host::Zellij)
    } else if set("WEZTERM_PANE") {
        Some(Host::WezTerm)
    } else if set("KITTY_WINDOW_ID") {
        Some(Host::Kitty)
    } else if env("TERM_PROGRAM").as_deref() == Some("iTerm.app") {
        Some(Host::ITerm)
    } else {
        None
    }
}

#[cfg(test)]
thread_local! {
    static TEST_HOST: std::cell::Cell<Option<Host>> = const { std::cell::Cell::new(None) };
    static LAUNCHED: std::cell::RefCell<Vec<(String, Vec<String>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The host to split, from the process environment. Under `cfg(test)` it is
/// whatever the test set (`set_test_host`), so a `cargo test` run inside
/// tmux behaves like one outside it.
pub fn current_host() -> Option<Host> {
    #[cfg(test)]
    {
        TEST_HOST.with(|h| h.get())
    }
    #[cfg(not(test))]
    {
        detect(|k| std::env::var(k).ok())
    }
}

#[cfg(test)]
pub fn set_test_host(host: Option<Host>) {
    TEST_HOST.with(|h| h.set(host));
}

/// The launches made on this thread (tests never spawn anything).
#[cfg(test)]
pub fn take_launched() -> Vec<(String, Vec<String>)> {
    LAUNCHED.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

pub fn encode_place(loc: &NavLocation) -> String {
    let json = serde_json::to_vec(loc).unwrap_or_default();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
}

pub fn decode_place(s: &str) -> Result<NavLocation, String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.trim())
        .map_err(|e| format!("--at: not a neboto place ({e})"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("--at: not a neboto place ({e})"))
}

/// What the new neboto is started with, besides the place.
#[derive(Debug, Default, Clone)]
pub struct Session<'a> {
    pub service: Option<&'a str>,
    pub region: &'a str,
    pub profile: Option<&'a str>,
    pub endpoint: Option<&'a str>,
    pub demo: bool,
    pub watch: bool,
}

/// The `neboto …` command line (shell-quoted, ready for `sh -c`).
pub fn neboto_command(exe: &str, session: &Session, place: Option<&str>) -> String {
    let q = crate::aws::resource::shell_quote;
    let mut parts = vec![q(exe)];
    if let Some(s) = session.service {
        parts.extend(["-s".to_string(), q(s)]);
    }
    parts.extend(["-r".to_string(), q(session.region)]);
    if let Some(p) = session.profile {
        parts.extend(["-p".to_string(), q(p)]);
    }
    if let Some(e) = session.endpoint {
        parts.extend(["--endpoint-url".to_string(), q(e)]);
    }
    if session.demo {
        parts.push("--demo".to_string());
    }
    if session.watch {
        parts.push("-w".to_string());
    }
    // One update check per run is enough; the first pane already did it.
    parts.push("--no-update-check".to_string());
    if let Some(p) = place {
        parts.extend(["--at".to_string(), p.to_string()]);
    }
    parts.join(" ")
}

/// The program + args that open a pane to the right of this one running
/// `shell_cmd` through `sh -c`. `env` is read for the pane / window ids, so
/// the split lands beside neboto rather than beside whichever pane is
/// active.
pub fn split_invocation(
    host: Host,
    shell_cmd: &str,
    env: impl Fn(&str) -> Option<String>,
) -> (String, Vec<String>) {
    let s = |v: &str| v.to_string();
    match host {
        Host::Tmux => {
            let mut args = vec![s("split-window"), s("-h")];
            if let Some(pane) = env("TMUX_PANE") {
                args.extend([s("-t"), pane]);
            }
            args.push(shell_cmd.to_string());
            (s("tmux"), args)
        }
        Host::Zellij => (
            s("zellij"),
            vec![
                s("run"),
                s("--direction"),
                s("right"),
                s("--close-on-exit"),
                s("--"),
                s("sh"),
                s("-c"),
                shell_cmd.to_string(),
            ],
        ),
        Host::WezTerm => {
            let mut args = vec![s("cli"), s("split-pane"), s("--right")];
            if let Some(pane) = env("WEZTERM_PANE") {
                args.extend([s("--pane-id"), pane]);
            }
            args.extend([s("--"), s("sh"), s("-c"), shell_cmd.to_string()]);
            (s("wezterm"), args)
        }
        // Needs `allow_remote_control` in kitty.conf; a vertical split
        // needs the `splits` layout, other layouts just add a window.
        Host::Kitty => (
            s("kitten"),
            vec![
                s("@"),
                s("launch"),
                s("--location=vsplit"),
                s("--cwd=current"),
                s("sh"),
                s("-c"),
                shell_cmd.to_string(),
            ],
        ),
        Host::ITerm => {
            let inner = format!("sh -c {}", crate::aws::resource::shell_quote(shell_cmd));
            let escaped = inner.replace('\\', "\\\\").replace('"', "\\\"");
            (
                s("osascript"),
                vec![
                    s("-e"),
                    format!(
                        "tell application \"iTerm\" to tell current session of current window \
                         to split vertically with default profile command \"{escaped}\""
                    ),
                ],
            )
        }
    }
}

/// Run the split. It returns as soon as the host has the new pane, so the
/// wait is short; the host's own error text is what's reported on failure.
pub fn run(prog: &str, args: &[String]) -> Result<(), String> {
    #[cfg(test)]
    {
        LAUNCHED.with(|l| l.borrow_mut().push((prog.to_string(), args.to_vec())));
        Ok(())
    }
    #[cfg(not(test))]
    {
        let out = std::process::Command::new(prog)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("{prog}: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            let err = err.trim();
            Err(if err.is_empty() { format!("{prog} exited with {}", out.status) } else { format!("{prog}: {err}") })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_innermost_host_is_split() {
        assert_eq!(detect(env(&[])), None);
        assert_eq!(detect(env(&[("TERM_PROGRAM", "iTerm.app")])), Some(Host::ITerm));
        assert_eq!(detect(env(&[("KITTY_WINDOW_ID", "3")])), Some(Host::Kitty));
        // tmux run inside WezTerm inherits WEZTERM_PANE: split the tmux pane.
        assert_eq!(
            detect(env(&[("WEZTERM_PANE", "0"), ("TMUX", "/tmp/tmux-501/default,1,0")])),
            Some(Host::Tmux)
        );
        assert_eq!(detect(env(&[("ZELLIJ", "0"), ("TERM_PROGRAM", "iTerm.app")])), Some(Host::Zellij));
        assert_eq!(detect(env(&[("TMUX", "")])), None);
    }

    #[test]
    fn tmux_splits_neboto_s_own_pane() {
        let (prog, args) = split_invocation(Host::Tmux, "neboto -r eu-west-1", env(&[("TMUX_PANE", "%4")]));
        assert_eq!(prog, "tmux");
        assert_eq!(args, ["split-window", "-h", "-t", "%4", "neboto -r eu-west-1"]);
    }

    #[test]
    fn the_command_carries_the_session_and_the_place() {
        let session = Session {
            service: Some("@ecs"),
            region: "ap-southeast-2",
            profile: Some("prod admin"),
            endpoint: Some("http://localhost:4566"),
            demo: false,
            watch: true,
        };
        assert_eq!(
            neboto_command("/opt/bin/neboto", &session, Some("eyJ9")),
            "/opt/bin/neboto -s @ecs -r ap-southeast-2 -p 'prod admin' \
             --endpoint-url http://localhost:4566 -w --no-update-check --at eyJ9"
        );
    }

    #[test]
    fn iterm_gets_a_quoted_applescript_command() {
        let (_, args) = split_invocation(Host::ITerm, "neboto -p 'a b'", env(&[]));
        assert_eq!(
            args[1],
            "tell application \"iTerm\" to tell current session of current window to split \
             vertically with default profile command \"sh -c 'neboto -p '\\\\''a b'\\\\'''\""
        );
    }

    #[test]
    fn a_place_survives_the_trip() {
        let loc = NavLocation {
            service: crate::aws::service::ServiceType::ECS,
            view: crate::app::JumpView::None,
            query: "orders".into(),
            selected_id: Some("arn:aws:ecs:ap-southeast-2:123456789012:service/prod/orders".into()),
            label: "ECS · orders".into(),
            s3_object_path: None,
            details_focused: true,
            detail_section: Some("Tasks".into()),
            region: Some("ap-southeast-2".into()),
            credentials: None,
            lens: None,
        };
        let encoded = encode_place(&loc);
        assert!(encoded.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        let back = decode_place(&encoded).unwrap();
        assert_eq!(back.selected_id, loc.selected_id);
        assert_eq!(back.detail_section.as_deref(), Some("Tasks"));
        assert!(back.details_focused);
        assert!(decode_place("not a place!").is_err());
    }
}
