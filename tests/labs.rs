//! Labs: a real `just` and a temporary git repository, with jlog as the shell.

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use common::{JLOG, project, run_log};

fn shell_line(shell: &str, flags: &str) -> String {
    format!("set shell := [\"{JLOG}\", \"{shell}\", {flags}]\n")
}

/// A temporary git repository with `.agents/` and the given Justfile body,
/// after a `set shell` line that runs jlog with `sh -cu`.
fn repo(justfile: &str) -> tempfile::TempDir {
    let t = project();
    git(t.path(), &["init", "-q"]);
    let text = format!("{}{justfile}", shell_line("sh", "\"-cu\""));
    fs::write(t.path().join("Justfile"), text).unwrap();
    t
}

fn git(dir: &Path, args: &[&str]) -> Output {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "lab")
        .env("GIT_AUTHOR_EMAIL", "lab@example.com")
        .env("GIT_COMMITTER_NAME", "lab")
        .env("GIT_COMMITTER_EMAIL", "lab@example.com")
        .output()
        .expect("git is on PATH");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
    out
}

fn just(dir: &Path, args: &[&str]) -> Output {
    common::cmd("just")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("just is on PATH")
}

#[test]
fn each_line_of_a_recipe_is_logged_with_the_argv_and_one_run() {
    let r = repo("set positional-arguments\na x:\n  echo one \"$1\"\n  echo two\n");
    let out = just(r.path(), &["a", "it is"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "one it is\ntwo\n");
    let log = run_log(r.path());
    assert_eq!(log.len(), 2);
    assert_eq!(log[0]["line"], "echo one \"$1\"");
    assert_eq!(log[1]["line"], "echo two");
    for e in &log {
        assert_eq!(e["argv"], serde_json::json!(["just", "a", "it is"]));
        assert_eq!(e["kind"], "line");
        assert_eq!(e["run"], log[0]["run"]);
    }
    just(r.path(), &["a", "again"]);
    let log = run_log(r.path());
    assert_ne!(log[2]["run"], log[0]["run"], "a new call is a new run");
    let pid = log[0]["run"].as_str().unwrap().split('-').next().unwrap();
    assert!(pid.parse::<u32>().is_ok());
}

#[test]
fn sigterm_to_just_stops_the_line_as_with_plain_sh() {
    let r = repo("a:\n  sleep 2; echo late > after.txt\n");
    let mut child = common::cmd("just")
        .arg("a")
        .current_dir(r.path())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    // SAFETY: kill has no memory preconditions.
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    child.wait().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert!(
        !r.path().join("after.txt").exists(),
        "the line ran on after just stopped"
    );
    assert_eq!(run_log(r.path())[0]["exit"], 128 + libc::SIGTERM);
}

#[test]
fn a_failed_line_stops_the_recipe_and_is_logged() {
    let r = repo("a:\n  false\n  echo never\n");
    let out = just(r.path(), &["a"]);
    assert_eq!(out.status.code(), Some(1));
    let log = run_log(r.path());
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["exit"], 1);
}

#[test]
fn a_backtick_is_tagged() {
    let r = repo("x := `echo bt`\na:\n  echo {{x}}\n");
    let out = just(r.path(), &["a"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "bt\n");
    let kinds: Vec<_> = run_log(r.path())
        .iter()
        .map(|e| e["kind"].clone())
        .collect();
    assert_eq!(kinds, ["backtick", "line"]);
}

#[test]
fn a_shell_call_in_a_recipe_line_is_tagged_as_a_backtick() {
    let r = repo("a:\n  echo {{shell('echo in')}} `true`\n");
    just(r.path(), &["a"]);
    let kinds: Vec<_> = run_log(r.path())
        .iter()
        .map(|e| e["kind"].clone())
        .collect();
    assert_eq!(kinds, ["backtick", "line"]);
}

#[cfg(target_os = "macos")]
#[test]
fn a_backtick_is_tagged_when_just_runs_in_a_terminal() {
    let r = repo("x := `echo bt`\na:\n  echo {{x}}\n");
    // script(1) gives just a pseudo-terminal as its stdout.
    let out = common::cmd("script")
        .args(["-q", "/dev/null", "just", "a"])
        .current_dir(r.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let kinds: Vec<_> = run_log(r.path())
        .iter()
        .map(|e| e["kind"].clone())
        .collect();
    assert_eq!(kinds, ["backtick", "line"]);
}

#[test]
fn a_secret_from_a_dotenv_file_is_not_in_line_or_argv() {
    let r = repo("set dotenv-load\na v:\n  echo {{env('LAB_API_KEY')}} {{v}}\n");
    fs::write(r.path().join(".env"), "LAB_API_KEY=lab-secret-0123456789\n").unwrap();
    let out = just(r.path(), &["a", "lab-secret-0123456789"]);
    assert!(out.status.success(), "{out:?}");
    let text = fs::read_to_string(r.path().join(".agents/log/just.jsonl")).unwrap();
    assert!(!text.contains("lab-secret-0123456789"), "{text}");
    let e = &run_log(r.path())[0];
    assert_eq!(e["line"], "echo <redacted> <redacted>");
    assert_eq!(e["argv"], serde_json::json!(["just", "a", "<redacted>"]));
}

#[test]
fn the_log_folder_survives_add_stash_and_clean() {
    let r = repo("a:\n  true\n");
    fs::write(r.path().join("f.txt"), "x\n").unwrap();
    git(r.path(), &["add", "-A"]);
    git(r.path(), &["commit", "-qm", "base"]);
    just(r.path(), &["a"]);
    let log = r.path().join(".agents/log/just.jsonl");
    assert!(log.exists());

    git(r.path(), &["add", "-A"]);
    let staged = git(r.path(), &["diff", "--cached", "--name-only"]);
    assert_eq!(String::from_utf8_lossy(&staged.stdout), "");

    fs::write(r.path().join("new.txt"), "y\n").unwrap();
    git(r.path(), &["stash", "-u"]);
    assert!(log.exists(), "git stash -u kept the log");
    git(r.path(), &["stash", "pop"]);

    git(r.path(), &["clean", "-fd"]);
    assert!(log.exists(), "git clean -fd kept the log");
    assert!(!r.path().join("new.txt").exists());
}

#[test]
fn the_lines_of_a_module_are_logged() {
    let r = repo("mod m\na:\n  echo root\n");
    fs::write(
        r.path().join("m.just"),
        format!("{}b:\n  echo module\n", shell_line("sh", "\"-cu\"")),
    )
    .unwrap();
    let out = just(r.path(), &["m", "b"]);
    assert!(out.status.success(), "{out:?}");
    let log = run_log(r.path());
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["line"], "echo module");
    assert_eq!(log[0]["argv"], serde_json::json!(["just", "m", "b"]));
}

#[test]
fn flags_after_c_run_and_give_a_null_line() {
    let t = project();
    git(t.path(), &["init", "-q"]);
    let text = format!("{}a:\n  echo ran\n", shell_line("sh", "\"-c\", \"-eu\""));
    fs::write(t.path().join("Justfile"), text).unwrap();
    let out = just(t.path(), &["a"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ran\n");
    // jlog does not model options after -c, so it logs no line, and never a wrong one.
    assert!(run_log(t.path())[0]["line"].is_null());
}

#[test]
fn a_wrapped_bash_shell_keeps_working() {
    let t = project();
    git(t.path(), &["init", "-q"]);
    let text = format!(
        "{}a:\n  set -o pipefail; false | true\n",
        shell_line("bash", "\"-c\"")
    );
    fs::write(t.path().join("Justfile"), text).unwrap();
    let out = just(t.path(), &["a"]);
    assert_eq!(out.status.code(), Some(1), "pipefail of bash works");
    assert_eq!(run_log(t.path())[0]["exit"], 1);
}
