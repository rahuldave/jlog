//! Runs the jlog binary directly, as `just` would, and checks the run log.

mod common;

use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;

use common::{JLOG, project, run_log};

fn jlog(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(JLOG)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

#[test]
fn runs_the_line_with_the_positional_arguments() {
    let p = project();
    let out = jlog(
        p.path(),
        &["sh", "-cu", "echo \"$0|$1|$2\"", "recipe", "a b", "c"],
    );
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "recipe|a b|c\n");
}

#[test]
fn logs_one_line_with_the_fields() {
    let p = project();
    jlog(p.path(), &["sh", "-cu", "true"]);
    let log = run_log(p.path());
    assert_eq!(log.len(), 1);
    let e = &log[0];
    let keys: Vec<&str> = e.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["argv", "end", "exit", "kind", "line", "run", "start"]
    );
    assert_eq!(e["line"], "true");
    assert_eq!(e["exit"], 0);
    assert!(e["run"].as_str().unwrap().contains('-'));
    assert!(e["start"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn passes_the_exit_code_through_and_logs_it() {
    let p = project();
    let out = jlog(p.path(), &["sh", "-c", "exit 7"]);
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(run_log(p.path())[0]["exit"], 7);
}

#[test]
fn a_killed_shell_kills_jlog_with_the_same_signal() {
    let p = project();
    let out = jlog(p.path(), &["sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.signal(), Some(libc::SIGTERM));
    assert_eq!(run_log(p.path())[0]["exit"], 128 + libc::SIGTERM);
}

#[test]
fn sigterm_to_jlog_reaches_the_shell_and_is_logged() {
    let p = project();
    let mut child = Command::new(JLOG)
        .args(["sh", "-c", "sleep 2; echo late > after.txt"])
        .current_dir(p.path())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    // SAFETY: kill has no memory preconditions.
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM));
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert!(
        !p.path().join("after.txt").exists(),
        "the shell kept running"
    );
    assert_eq!(run_log(p.path())[0]["exit"], 128 + libc::SIGTERM);
}

#[test]
fn sighup_to_jlog_reaches_the_shell() {
    let p = project();
    let mut child = Command::new(JLOG)
        .args(["sh", "-c", "sleep 5"])
        .current_dir(p.path())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let t0 = std::time::Instant::now();
    // SAFETY: kill has no memory preconditions.
    unsafe { libc::kill(child.id() as i32, libc::SIGHUP) };
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGHUP));
    assert!(
        t0.elapsed().as_secs() < 3,
        "jlog waited for the whole sleep"
    );
    assert_eq!(run_log(p.path())[0]["exit"], 128 + libc::SIGHUP);
}

#[test]
fn a_closed_stderr_does_not_stop_jlog() {
    use std::os::fd::{FromRawFd, OwnedFd};
    let p = project();
    fs::write(p.path().join(".agents/log"), "").unwrap();
    let mut fds = [0; 2];
    // SAFETY: fds has room for the two descriptors of the pipe.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: each descriptor is open, and each is owned once.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    drop(read);
    // Exact codes: a panic would give 101 in a debug build, or SIGABRT in a
    // release build.
    let cases: [(&[&str], i32); 3] = [
        (&["sh", "-c", "exit 0"], 0),
        (&["no-such-shell-jlog", "-c", "true"], 127),
        (&[], 2),
    ];
    for (args, code) in cases {
        let status = Command::new(JLOG)
            .args(args)
            .current_dir(p.path())
            .stderr(write.try_clone().unwrap())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(code), "{args:?}");
    }
    for flag in ["--version", "--help"] {
        let status = Command::new(JLOG)
            .arg(flag)
            .stdout(write.try_clone().unwrap())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(0), "{flag} with a closed stdout");
    }
}

#[test]
fn an_ignored_sighup_stays_ignored_in_the_shell() {
    let p = project();
    // nohup makes SIGHUP ignored for jlog, and so for the shell.
    let out = Command::new("nohup")
        .args([JLOG, "sh", "-c", "kill -HUP $$; echo survived"])
        .current_dir(p.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "survived\n");
}

#[test]
fn sigint_to_jlog_alone_leaves_the_line_running() {
    let p = project();
    let mut child = Command::new(JLOG)
        .args(["sh", "-c", "sleep 1; exit 3"])
        .current_dir(p.path())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    // SAFETY: kill has no memory preconditions.
    unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(3), "the line ran to its end");
    assert_eq!(run_log(p.path())[0]["exit"], 3);
}

/// A SIGTERM at once after the start. The unit test
/// `a_signal_before_the_spawn_reaches_the_shell` covers the signal that comes
/// before the shell has a process ID; this one checks the whole program.
#[test]
fn an_early_sigterm_leaves_no_shell_running() {
    let p = project();
    let mut children = Vec::new();
    for i in 0..20 {
        let line = format!("sleep 1; echo late > after{i}.txt");
        let child = Command::new(JLOG)
            .args(["sh", "-c", &line])
            .current_dir(p.path())
            .spawn()
            .unwrap();
        // SAFETY: kill has no memory preconditions.
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        children.push(child);
    }
    for mut c in children {
        c.wait().unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let late: Vec<_> = fs::read_dir(p.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("after"))
        .collect();
    assert!(late.is_empty(), "{} shells ran on", late.len());
}

#[test]
fn outside_just_the_argv_is_empty() {
    let p = project();
    jlog(p.path(), &["sh", "-c", "true"]);
    assert_eq!(run_log(p.path())[0]["argv"], serde_json::json!([]));
}

#[test]
fn a_missing_shell_exits_127_and_is_logged() {
    let p = project();
    let out = jlog(p.path(), &["no-such-shell-jlog", "-c", "true"]);
    assert_eq!(out.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not run the shell"));
    assert_eq!(run_log(p.path())[0]["exit"], 127);
    assert!(run_log(p.path())[0]["line"].is_null(), "no line ran");
}

#[test]
fn the_first_write_makes_the_gitignore() {
    let p = project();
    jlog(p.path(), &["sh", "-c", "true"]);
    let ignore = fs::read_to_string(p.path().join(".agents/log/.gitignore")).unwrap();
    assert_eq!(ignore, "*\n");
}

#[test]
fn finds_the_log_folder_from_a_subfolder() {
    let p = project();
    let sub = p.path().join("a/b");
    fs::create_dir_all(&sub).unwrap();
    jlog(&sub, &["sh", "-c", "true"]);
    assert_eq!(run_log(p.path()).len(), 1);
    assert!(!sub.join(".agents").exists());
}

#[test]
fn without_an_agents_folder_it_runs_and_logs_nothing() {
    // An .agents folder above the repository must not catch the log.
    let outer = project();
    let repo = outer.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let out = jlog(&repo, &["sh", "-c", "echo ok"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ok\n");
    assert_eq!(fs::read_dir(&repo).unwrap().count(), 1);
    assert!(!outer.path().join(".agents/log").exists());
}

#[test]
fn does_not_log_into_the_agents_folder_of_home() {
    let home = project();
    let repo = home.path().join("repo");
    fs::create_dir(&repo).unwrap();
    Command::new(JLOG)
        .args(["sh", "-c", "true"])
        .current_dir(&repo)
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(!home.path().join(".agents/log").exists());
}

#[test]
fn redacts_secret_values_from_the_line() {
    let p = project();
    Command::new(JLOG)
        .args(["sh", "-c", "echo hunter2-hunter2 short1 /safe/path"])
        .current_dir(p.path())
        .env("JLOG_TEST_TOKEN", "hunter2-hunter2")
        .env("JLOG_TEST_SHORT", "short1")
        .env(
            "PATH",
            format!("/safe/path:{}", std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    let line = run_log(p.path())[0]["line"].as_str().unwrap().to_string();
    assert_eq!(line, "echo <redacted> short1 /safe/path");
}

#[test]
fn the_line_is_null_when_no_flag_names_it() {
    let p = project();
    fs::write(p.path().join("s.sh"), "true\n").unwrap();
    let out = jlog(p.path(), &["sh", "s.sh"]);
    assert!(out.status.success());
    assert!(run_log(p.path())[0]["line"].is_null());
}

#[test]
fn a_failed_log_write_warns_and_keeps_the_exit_code() {
    let p = project();
    // A file where the log folder must go makes the write fail.
    fs::write(p.path().join(".agents/log"), "").unwrap();
    let out = jlog(p.path(), &["sh", "-c", "exit 4"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not write the run log"));
}

#[test]
fn no_arguments_print_the_usage() {
    let out = Command::new(JLOG).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: jlog"));
    let v = Command::new(JLOG).arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&v.stdout).starts_with("jlog "));
}
