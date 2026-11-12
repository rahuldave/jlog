//! Finds the session of a line: the harness, its session ID, and whether the
//! line comes from a fresh agent. jlog reads only the environment here.

use std::ffi::OsString;

use serde::Serialize;

/// The session of a line, as each record of the run log shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    /// `codex`, `claude` or `terminal`.
    pub harness: &'static str,
    /// The ID of the session. For a terminal, the process ID of the session
    /// leader and its start time, joined by `-`.
    pub session: String,
    /// True for a line of a fresh agent. Null when the environment cannot
    /// tell, as in Codex, where the plugin finds the parent thread.
    pub child: Option<bool>,
}

/// The longest session ID that jlog keeps.
const MAX_ID: usize = 200;

impl Session {
    /// The session of this process.
    pub fn detect() -> Session {
        Session::detect_with(|k| std::env::var_os(k), terminal)
    }

    /// The session from the variables that `var` gives. `terminal` gives the
    /// ID of the terminal session, when no harness is found.
    pub fn detect_with(
        var: impl Fn(&str) -> Option<OsString>,
        terminal: impl FnOnce() -> String,
    ) -> Session {
        let id = |k: &str| {
            var(k)
                .map(|v| v.to_string_lossy().trim().to_string())
                .filter(|v| !v.is_empty())
                .map(|v| v.chars().take(MAX_ID).collect::<String>())
        };
        let claude_child = || {
            Some(
                id("CLAUDE_CODE_CHILD_SESSION")
                    .is_some_and(|v| v != "0" && !v.eq_ignore_ascii_case("false")),
            )
        };
        if let Some(thread) = id("CODEX_THREAD_ID") {
            return Session {
                harness: "codex",
                session: thread,
                child: None,
            };
        }
        // The SessionStart hook of Claude Code writes JLOG_SESSION when
        // CLAUDE_CODE_SESSION_ID is missing.
        if let Some(s) = id("CLAUDE_CODE_SESSION_ID").or_else(|| id("JLOG_SESSION")) {
            return Session {
                harness: "claude",
                session: s,
                child: claude_child(),
            };
        }
        Session {
            harness: "terminal",
            session: terminal(),
            child: Some(false),
        }
    }

    /// The name of the marker file in `.agents/log/sessions/`. An ID with
    /// other characters than letters, digits, `.`, `_` and `-` gets its hash
    /// in place of itself, so the name is always a plain file name.
    pub fn marker(&self) -> String {
        let plain = self
            .session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if plain && !self.session.starts_with('.') {
            format!("{}-{}", self.harness, self.session)
        } else {
            use sha2::Digest;
            let hash = sha2::Sha256::digest(self.session.as_bytes());
            format!("{}-x{}", self.harness, hex(&hash[..16]))
        }
    }
}

/// The terminal session of this process: the process ID of the session
/// leader from `getsid(0)`, and its start time, so a reused process ID makes
/// a new session.
pub fn terminal() -> String {
    // SAFETY: getsid and getppid have no preconditions.
    let (sid, ppid) = unsafe { (libc::getsid(0), libc::getppid()) };
    terminal_from(sid, ppid, crate::proc::start_time)
}

/// The terminal session from `sid` and the parent `ppid`. In a container the
/// leader can live outside the namespace of process IDs, so `getsid` gives
/// 0. Then the session falls back to the parent, which is `just`, so each
/// call is a session of its own, and never one session for ever.
pub fn terminal_from(sid: i32, ppid: i32, start_time: impl Fn(i32) -> Option<u64>) -> String {
    match (sid > 0).then(|| start_time(sid)).flatten() {
        Some(start) => format!("{sid}-{start}"),
        None => format!("p{ppid}-{}", start_time(ppid).unwrap_or(0)),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(vars: &[(&str, &str)]) -> Session {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Session::detect_with(
            |k| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.into()),
            || "77-1700000000".to_string(),
        )
    }

    #[test]
    fn codex_wins_over_claude_and_has_no_child_flag() {
        let s = detect(&[
            ("CODEX_THREAD_ID", "t-1"),
            ("CLAUDE_CODE_SESSION_ID", "c-1"),
        ]);
        assert_eq!(
            s,
            Session {
                harness: "codex",
                session: "t-1".into(),
                child: None
            }
        );
    }

    #[test]
    fn claude_marks_a_fresh_agent() {
        let s = detect(&[("CLAUDE_CODE_SESSION_ID", "c-1")]);
        assert_eq!(s.child, Some(false));
        let s = detect(&[
            ("CLAUDE_CODE_SESSION_ID", "c-1"),
            ("CLAUDE_CODE_CHILD_SESSION", "1"),
        ]);
        assert_eq!((s.harness, s.child), ("claude", Some(true)));
        let s = detect(&[
            ("CLAUDE_CODE_SESSION_ID", "c-1"),
            ("CLAUDE_CODE_CHILD_SESSION", "0"),
        ]);
        assert_eq!(s.child, Some(false));
    }

    #[test]
    fn jlog_session_is_the_claude_fallback() {
        let s = detect(&[("JLOG_SESSION", "j-1")]);
        assert_eq!((s.harness, s.session.as_str()), ("claude", "j-1"));
    }

    #[test]
    fn empty_variables_do_not_count() {
        let s = detect(&[("CODEX_THREAD_ID", " "), ("CLAUDE_CODE_SESSION_ID", "")]);
        assert_eq!(
            s,
            Session {
                harness: "terminal",
                session: "77-1700000000".into(),
                child: Some(false)
            }
        );
    }

    #[test]
    fn a_marker_is_always_a_plain_file_name() {
        let s = detect(&[("CODEX_THREAD_ID", "019a-ff.x_1")]);
        assert_eq!(s.marker(), "codex-019a-ff.x_1");
        for bad in ["../../etc", ".hidden", "a/b", "a\u{0}b", "é"] {
            let m = detect(&[("CODEX_THREAD_ID", bad)]).marker();
            assert!(
                m.starts_with("codex-x") && m.len() == 39,
                "{bad:?} gave {m}"
            );
        }
        let long = "a".repeat(500);
        assert_eq!(detect(&[("CODEX_THREAD_ID", &long)]).session.len(), MAX_ID);
    }

    #[test]
    fn a_session_with_no_leader_falls_back_to_the_parent() {
        let start = |pid: i32| (pid == 9).then_some(1700);
        assert_eq!(terminal_from(9, 4, start), "9-1700");
        assert_eq!(terminal_from(0, 9, start), "p9-1700");
        assert_eq!(
            terminal_from(5, 9, start),
            "p9-1700",
            "a leader with no start time"
        );
    }

    #[test]
    fn the_terminal_session_is_the_same_in_this_process() {
        assert_eq!(terminal(), terminal());
        assert!(!terminal().ends_with("-0"));
    }
}
