//! Labs for the sessions and the commands `session`, `env`, `tools` and
//! `skip`, with a real `just` and fake folders for each harness.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{JLOG, all_records, project, records, run_log};
use serde_json::{Value, json};

const THREAD: &str = "019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b";
const PARENT: &str = "019a0000-0000-7000-8000-000000000001";
const CLAUDE_SID: &str = "5f0c2a9e-1111-4222-8333-944455556666";

fn plugins() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins")
}

/// A project whose Justfile runs each line through jlog with `sh -cu`.
fn justfile(body: &str) -> tempfile::TempDir {
    let p = project();
    let text = format!("set shell := [\"{JLOG}\", \"sh\", \"-cu\"]\n{body}");
    fs::write(p.path().join("Justfile"), text).unwrap();
    p
}

fn run(c: &mut Command, dir: &Path) -> Output {
    let out = c.current_dir(dir).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    out
}

fn just(dir: &Path, args: &[&str]) -> Command {
    let mut c = common::cmd("just");
    c.args(args).current_dir(dir);
    c
}

/// Each path under `root` with its size and times, to see any write.
fn tree(root: &Path) -> BTreeMap<PathBuf, (u64, i64, i64)> {
    let mut out = BTreeMap::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(p) = todo.pop() {
        let m = fs::symlink_metadata(&p).unwrap();
        out.insert(
            p.clone(),
            (
                m.len(),
                m.mtime_nsec() + m.mtime() * 1_000_000_000,
                m.ctime(),
            ),
        );
        if m.is_dir() {
            todo.extend(fs::read_dir(&p).unwrap().map(|e| e.unwrap().path()));
        }
    }
    out
}

fn lines(path: &Path, rows: &[Value]) {
    let text: String = rows.iter().map(|r| format!("{r}\n")).collect();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn at_a_terminal_all_calls_share_one_session_and_log_it_once() {
    let p = justfile("a:\n  echo one\n  echo two\n");
    run(&mut just(p.path(), &["a"]), p.path());
    run(&mut just(p.path(), &["a"]), p.path());
    let log = run_log(p.path());
    assert_eq!(log.len(), 4);
    for e in &log {
        assert_eq!(e["harness"], "terminal");
        assert_eq!(e["session"], log[0]["session"]);
    }
    assert_eq!(records(p.path(), "session").len(), 1);
    assert_eq!(records(p.path(), "env").len(), 1);
    assert_eq!(records(p.path(), "tools").len(), 1);
    let s = &records(p.path(), "session")[0];
    assert_eq!(
        s["run"], log[0]["run"],
        "the records of a session carry the run of their line"
    );
    let marker = format!("terminal-{}", log[0]["session"].as_str().unwrap());
    assert!(p.path().join(".agents/log/sessions").join(marker).is_file());

    // A call in a new terminal session is a new session.
    let mut c = just(p.path(), &["a"]);
    // SAFETY: setsid is safe to call in the child before exec.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    run(&mut c, p.path());
    let log = run_log(p.path());
    assert_ne!(log[4]["session"], log[0]["session"]);
    assert_eq!(records(p.path(), "session").len(), 2);
}

#[test]
fn in_codex_each_line_has_the_thread_and_the_session_names_the_rollout() {
    let p = justfile("a:\n  echo one\n  echo two\n");
    let home = tempfile::tempdir().unwrap();
    let codex = home.path().join("codex");
    let rollout = codex.join(format!(
        "sessions/2026/11/12/rollout-2026-11-12T10-00-00-{THREAD}.jsonl"
    ));
    let spawn = json!({"subagent": {"thread_spawn": {"parent_thread_id": PARENT, "depth": 1}}});
    lines(
        &rollout,
        &[
            json!({"type": "session_meta", "payload": {"id": THREAD, "source": spawn}}),
            json!({"type": "turn_context", "payload": {"model": "gpt-fake-5"}}),
        ],
    );
    fs::write(codex.join("state_5.sqlite"), b"SQLite format 3\0").unwrap();
    let before = tree(&codex);
    let venv = p.path().join(".venv");
    let mut c = just(p.path(), &["a"]);
    c.env("CODEX_THREAD_ID", THREAD)
        .env("CODEX_HOME", &codex)
        .env("JLOG_PLUGINS", plugins())
        // A repository setting must not move the plugin into the repository.
        .env("UV_PROJECT_ENVIRONMENT", &venv);
    run(&mut c, p.path());

    for e in run_log(p.path()) {
        assert_eq!(
            (e["harness"].clone(), e["session"].clone()),
            (json!("codex"), json!(THREAD))
        );
        assert_eq!(e["child"], Value::Null);
    }
    let s = &records(p.path(), "session")[0];
    assert_eq!(s["transcript"], rollout.to_str().unwrap(), "{s}");
    assert_eq!(s["model"], "gpt-fake-5");
    assert_eq!(s["parent"], PARENT);
    assert_eq!(s["error"], Value::Null);
    assert_eq!(
        tree(&codex),
        before,
        "the plugin wrote in the folder of Codex"
    );
    assert!(
        !venv.exists(),
        "the plugin made an environment in the repository"
    );
}

#[test]
fn in_claude_code_each_line_has_the_session_and_a_fresh_agent_is_marked() {
    let p = justfile("a:\n  echo one\n");
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("claude");
    let transcript = config.join(format!("projects/-proj/{CLAUDE_SID}.jsonl"));
    lines(
        &transcript,
        &[json!({"type": "assistant", "message": {"model": "claude-fake-1"}})],
    );
    let before = tree(&config);
    let mut c = just(p.path(), &["a"]);
    c.env("CLAUDE_CODE_SESSION_ID", CLAUDE_SID)
        .env("CLAUDE_CONFIG_DIR", &config)
        .env("JLOG_PLUGINS", plugins());
    run(&mut c, p.path());
    let mut c = just(p.path(), &["a"]);
    c.env("CLAUDE_CODE_SESSION_ID", CLAUDE_SID)
        .env("CLAUDE_CODE_CHILD_SESSION", "1")
        .env("CLAUDE_CONFIG_DIR", &config)
        .env("JLOG_PLUGINS", plugins());
    run(&mut c, p.path());

    let log = run_log(p.path());
    assert_eq!(
        (log[0]["harness"].clone(), log[0]["session"].clone()),
        (json!("claude"), json!(CLAUDE_SID))
    );
    assert_eq!(log[0]["child"], false);
    assert_eq!(log[1]["child"], true);
    let s = records(p.path(), "session");
    assert_eq!(
        s.len(),
        1,
        "the fresh agent shares the session of its parent"
    );
    assert_eq!(s[0]["transcript"], transcript.to_str().unwrap());
    assert_eq!(s[0]["model"], "claude-fake-1");
    assert_eq!(
        tree(&config),
        before,
        "the plugin wrote in the folder of Claude Code"
    );
}

#[test]
fn a_missing_plugin_is_logged_and_the_line_still_passes() {
    let p = justfile("a:\n  echo one\n");
    let mut c = just(p.path(), &["a"]);
    c.env("CODEX_THREAD_ID", THREAD);
    run(&mut c, p.path());
    let s = &records(p.path(), "session")[0];
    assert!(
        s["error"].as_str().unwrap().starts_with("no plugin at"),
        "{s}"
    );
    assert_eq!(s["transcript"], Value::Null);
}

#[test]
fn a_missing_program_in_the_tools_table_does_not_fail_a_line() {
    let p = justfile("a:\n  echo one\n");
    let bin = tempfile::tempdir().unwrap();
    // A PATH with sh and nothing of the tools table, so each tool is missing.
    std::os::unix::fs::symlink("/bin/sh", bin.path().join("sh")).unwrap();
    let just_path = which("just");
    let out = Command::new(just_path)
        .arg("a")
        .current_dir(p.path())
        .env_clear()
        .env("PATH", bin.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let t = &records(p.path(), "tools")[0]["tools"];
    let names: Vec<&str> = t
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "git", "just", "uv", "jlog", "jagt", "gest", "gh", "jq", "rg", "ast-grep", "node"
        ]
    );
    for x in t.as_array().unwrap() {
        assert_eq!(
            (x["version"].clone(), x["error"].clone()),
            (Value::Null, json!("not found")),
            "{x}"
        );
    }
}

fn which(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").unwrap();
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{name} is not on PATH"))
}

#[test]
fn jlog_tools_prints_and_logs_the_versions() {
    let p = project();
    let out = run(
        common::cmd(JLOG).arg("tools").current_dir(p.path()),
        p.path(),
    );
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed["kind"], "tools");
    assert_eq!(all_records(p.path()), std::slice::from_ref(&printed));
    let git = &printed["tools"][0];
    assert!(
        git["version"].as_str().unwrap().starts_with("git version"),
        "{git}"
    );
}

#[test]
fn jlog_env_shows_a_secret_as_redacted_and_tells_a_change() {
    let p = project();
    let env = |value: &str| {
        let mut c = common::cmd(JLOG);
        c.arg("env").env("FAKE_API_KEY", value);
        let out = run(&mut c, p.path());
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(!String::from_utf8_lossy(&out.stdout).contains(value));
        v["vars"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == "FAKE_API_KEY")
            .unwrap()
            .clone()
    };
    assert_eq!(
        env("sk-fake-0001-value"),
        json!({"name": "FAKE_API_KEY", "value": "redacted", "changed": null})
    );
    assert_eq!(env("sk-fake-0001-value")["changed"], false);
    assert_eq!(env("sk-fake-0002-value")["changed"], true);
    let text = fs::read_to_string(p.path().join(".agents/log/just.jsonl")).unwrap();
    assert!(!text.contains("sk-fake-000"));
    let state = fs::read_to_string(p.path().join(".agents/log/env-hashes.json")).unwrap();
    assert!(!state.contains("sk-fake-000"));
    assert_eq!(records(p.path(), "env").len(), 3);
}

#[test]
fn jlog_env_keeps_the_values_of_the_safe_list() {
    let p = project();
    let mut c = common::cmd(JLOG);
    c.arg("env").env("CI", "true");
    let out = run(&mut c, p.path());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ci = v["vars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "CI")
        .unwrap();
    assert_eq!(ci["value"], "true");
}

#[test]
fn jlog_session_at_a_terminal_names_the_terminal_session() {
    let p = project();
    let out = run(common::cmd(JLOG).arg("session"), p.path());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (v["kind"].clone(), v["harness"].clone()),
        (json!("session"), json!("terminal"))
    );
    // The run is the nearest `just` above, which is `just test` when the
    // tests run through just, and null when they do not.
    assert!(v["run"].is_null() || v["run"].as_str().unwrap().contains('-'));
    assert_eq!(all_records(p.path()), [v]);
}

#[test]
fn a_skip_in_a_recipe_logs_the_step_the_reason_and_the_run() {
    let p = justfile(&format!(
        "set dotenv-load\ntypecheck:\n  {JLOG} skip typecheck \"no type checker, key $FAKE_TOKEN\"\n"
    ));
    fs::write(p.path().join(".env"), "FAKE_TOKEN=tok-fake-abcdef\n").unwrap();
    run(&mut just(p.path(), &["typecheck"]), p.path());
    let skips = records(p.path(), "skip");
    assert_eq!(skips.len(), 1);
    let s = &skips[0];
    assert_eq!(s["step"], "typecheck");
    assert_eq!(s["reason"], "no type checker, key <redacted>");
    assert_eq!(s["argv"], json!(["just", "typecheck"]));
    assert_eq!(s["run"], run_log(p.path())[0]["run"]);
    let text = fs::read_to_string(p.path().join(".agents/log/just.jsonl")).unwrap();
    assert!(!text.contains("tok-fake-abcdef"));
}

#[test]
fn a_bad_skip_is_a_usage_error() {
    let p = project();
    for args in [
        &["skip"][..],
        &["skip", "typecheck"],
        &["skip", "type check", "r"],
        &["skip", "typecheck", "  "],
        &["skip", "a", "b", "c"],
    ] {
        let out = common::cmd(JLOG)
            .args(args)
            .current_dir(p.path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
    assert!(all_records(p.path()).is_empty());
}

#[test]
fn a_command_outside_a_project_prints_and_passes() {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir(t.path().join(".git")).unwrap();
    let out = as_recipe_line(t.path(), &["skip", "lint", "none"], &[]);
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["step"], "lint");
    assert!(!t.path().join(".agents").exists());
}

#[test]
fn a_command_that_cannot_write_the_log_fails() {
    let p = project();
    fs::create_dir_all(p.path().join(".agents/log/just.jsonl")).unwrap();
    let out = as_recipe_line(p.path(), &["skip", "lint", "none"], &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
}

#[test]
fn with_arguments_a_command_name_is_a_shell() {
    let p = project();
    let out = common::cmd(JLOG)
        .args(["env", "sh", "-c", "echo via-env"])
        .current_dir(p.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "via-env\n");
    assert_eq!(run_log(p.path()).len(), 1);
}

#[test]
fn a_skip_redacts_a_secret_in_the_step() {
    let p = project();
    let out = as_recipe_line(
        p.path(),
        &["skip", "tokFAKE0123456789", "r"],
        &[("FAKE_TOKEN", "tokFAKE0123456789")],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("tokFAKE"));
    assert_eq!(records(p.path(), "skip")[0]["step"], "<redacted>");
}

#[test]
fn a_broken_sessions_folder_warns_and_the_line_passes() {
    let p = project();
    fs::create_dir_all(p.path().join(".agents/log")).unwrap();
    fs::write(p.path().join(".agents/log/sessions"), "").unwrap();
    let out = common::cmd(JLOG)
        .args(["sh", "-c", "true"])
        .current_dir(p.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not mark the session"));
    assert_eq!(run_log(p.path()).len(), 1);
}

#[test]
fn the_plugin_works_with_a_home_that_cannot_be_written() {
    use std::os::unix::fs::PermissionsExt;
    let p = project();
    let home = tempfile::tempdir().unwrap();
    let codex = home.path().join("codex");
    let rollout = codex.join(format!("sessions/2026/11/12/rollout-x-{THREAD}.jsonl"));
    lines(
        &rollout,
        &[json!({"type": "turn_context", "payload": {"model": "gpt-ro"}})],
    );
    let ro = home.path().join("ro");
    fs::create_dir(&ro).unwrap();
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o500)).unwrap();
    let out = common::cmd(JLOG)
        .args(["session"])
        .env("HOME", &ro)
        .env("CODEX_THREAD_ID", THREAD)
        .env("CODEX_HOME", &codex)
        .env("JLOG_PLUGINS", plugins())
        .current_dir(p.path())
        .output()
        .unwrap();
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o700)).unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["model"], "gpt-ro", "{v}");
    assert_eq!(
        fs::read_dir(&ro).unwrap().count(),
        0,
        "nothing was written in the home"
    );
}

#[test]
fn a_signal_during_the_session_work_stops_it_and_ends_jlog() {
    use std::os::unix::process::ExitStatusExt;
    let p = project();
    let fake = tempfile::tempdir().unwrap();
    let dir = fake.path().join("codex");
    fs::create_dir(&dir).unwrap();
    for f in ["pyproject.toml", "uv.lock"] {
        fs::copy(plugins().join("codex").join(f), dir.join(f)).unwrap();
    }
    fs::write(dir.join("session.py"), "import time\ntime.sleep(30)\n").unwrap();
    let mut child = common::cmd(JLOG)
        .args(["sh", "-c", "true"])
        .env("CODEX_THREAD_ID", THREAD)
        .env("JLOG_PLUGINS", fake.path())
        .current_dir(p.path())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    // SAFETY: kill has no memory preconditions, and the child is not reaped.
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let sent = std::time::Instant::now();
    let status = child.wait().unwrap();
    assert!(
        sent.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        sent.elapsed()
    );
    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(run_log(p.path())[0]["exit"], 0, "the line was logged first");
    let s = records(p.path(), "session");
    assert_eq!(
        s[0]["error"],
        "the plugin failed: stopped by a signal to jlog"
    );
}

#[test]
fn install_plugins_makes_each_environment_where_jlog_looks() {
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("plugins");
    let elsewhere = t.path().join("elsewhere");
    let out = common::cmd("just")
        .args(["install-plugins"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("JLOG_PLUGINS", &dest)
        // jlog removes this variable at run time, so the sync must too.
        .env("UV_PROJECT_ENVIRONMENT", &elsewhere)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    for h in ["claude", "codex"] {
        for f in ["pyproject.toml", "uv.lock", "session.py", ".venv"] {
            assert!(dest.join(h).join(f).exists(), "{h}/{f} is missing");
        }
    }
    assert!(!elsewhere.exists());
}

/// Runs `jlog <args>` as the one line of a recipe, with a real `just`. The
/// tests may run under `just check`, and there `jlog skip` refuses to run
/// unless it is the line itself.
fn as_recipe_line(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let jf = tempfile::NamedTempFile::new().unwrap();
    let mut line = format!("'{JLOG}'");
    for a in args {
        line.push_str(&format!(" '{a}'"));
    }
    fs::write(
        jf.path(),
        format!("set shell := [\"{JLOG}\", \"sh\", \"-cu\"]\nx:\n  {line}\n"),
    )
    .unwrap();
    let mut c = common::cmd("just");
    c.arg("--justfile")
        .arg(jf.path())
        .arg("--working-directory")
        .arg(dir)
        .arg("x");
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}
