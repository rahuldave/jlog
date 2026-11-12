#![allow(dead_code)]

use std::fs;
use std::path::Path;

use serde_json::Value;

pub const JLOG: &str = env!("CARGO_BIN_EXE_jlog");

/// The lines of the run log under `root`, parsed.
pub fn run_log(root: &Path) -> Vec<Value> {
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
