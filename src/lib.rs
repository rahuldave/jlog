//! `jlog` is the shell of each `just` recipe. It runs one line with the real
//! shell, then adds one JSON line to the run log, `.agents/log/just.jsonl`.

use std::ffi::OsString;
use std::io::Write;

pub mod logdir;
pub mod proc;
pub mod redact;
pub mod shell;
pub mod time;

pub const USAGE: &str = "\
usage: jlog <shell> <flags> <line> [args...]

Runs <line> with <shell> and adds one JSON line to .agents/log/just.jsonl.
A Justfile sets it as its shell:

    set shell := [\"jlog\", \"sh\", \"-cu\"]
";

/// Runs jlog with the arguments after the program name, and returns the exit code.
pub fn run(args: &[OsString]) -> i32 {
    let Some(first) = args.first() else {
        let _ = std::io::stderr().write_all(USAGE.as_bytes());
        return 2;
    };
    match first.to_str() {
        Some("-h" | "--help") => {
            let _ = std::io::stdout().write_all(USAGE.as_bytes());
            0
        }
        Some("-V" | "--version") => {
            let _ = writeln!(std::io::stdout(), "jlog {}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => shell::run(args),
    }
}
