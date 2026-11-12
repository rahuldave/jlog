#![allow(dead_code)]

use std::fs;
use std::path::Path;

use serde_json::Value;

pub const JLOG: &str = env!("CARGO_BIN_EXE_jlog");

/// The records of lines and backticks in the run log under `root`, parsed.
pub fn run_log(root: &Path) -> Vec<Value> {
    all_records(root)
        .into_iter()
        .filter(|r| r["kind"] == "line" || r["kind"] == "backtick")
        .collect()
}

/// Every record of the run log under `root`, parsed.
pub fn all_records(root: &Path) -> Vec<Value> {
    let text = fs::read_to_string(root.join(".agents/log/just.jsonl")).unwrap_or_default();
    text.lines()
        .map(|l| serde_json::from_str(l).expect("each line is JSON"))
        .collect()
}

/// A temporary folder that holds `.agents/`.
pub fn project() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir(t.path().join(".agents")).unwrap();
    t
}

/// The variables by which jlog finds a harness. The tests remove them, so a
/// test run inside Claude Code or Codex never reads the real transcripts.
pub const HARNESS_VARS: [&str; 4] = [
    "CODEX_THREAD_ID",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "JLOG_SESSION",
];

/// A command for `program` with no harness variables, no plugins, and a
/// home folder inside the target folder, so the key of `jlog env` is not
/// written into the real home. A test can set any of them again.
pub fn cmd(program: &str) -> std::process::Command {
    let mut c = std::process::Command::new(program);
    for v in HARNESS_VARS {
        c.env_remove(v);
    }
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("home");
    fs::create_dir_all(&home).unwrap();
    c.env("HOME", home)
        .env("JLOG_PLUGINS", "/nonexistent/jlog-plugins");
    c
}

/// The records of the run log of one kind.
pub fn records(root: &Path, kind: &str) -> Vec<Value> {
    all_records(root)
        .into_iter()
        .filter(|r| r["kind"] == kind)
        .collect()
}
