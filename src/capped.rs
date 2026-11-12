//! Runs a program with a time limit, for `jlog tools` and the plugins. The
//! program gets no stdin and no stderr, so it cannot wait for input, and it
//! cannot write into a backtick that `just` reads.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The most bytes of stdout that jlog keeps.
const MAX_OUT: u64 = 64 * 1024;

/// Runs `cmd` and gives its stdout, or a short reason for the failure. When
/// the time runs out, jlog kills the whole process group of the program.
pub fn run(cmd: &mut Command, limit: Duration) -> Result<Vec<u8>, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "not found".to_string(),
        _ => format!("could not start: {e}"),
    })?;
    let pid = child.id() as i32;
    let mut out = child.stdout.take().expect("stdout is piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut out).take(MAX_OUT).read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + limit;
    let stopped = loop {
        if exited(pid) {
            break None;
        }
        if crate::shell::stopping() {
            break Some("stopped by a signal to jlog".to_string());
        }
        if Instant::now() >= deadline {
            break Some(format!("no answer in {} s", limit.as_secs_f32()));
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    // The child is not reaped yet, so its process ID, which is the ID of its
    // group, cannot belong to anyone else. Kill what is left of the group,
    // such as a child that holds the pipe, then reap.
    // SAFETY: killpg has no memory preconditions, and the group is ours.
    unsafe { libc::killpg(pid, libc::SIGKILL) };
    let status = child.wait();
    if let Some(why) = stopped {
        return Err(why);
    }
    let status = status.map_err(|e| format!("could not wait: {e}"))?;
    let buf = rx
        .recv_timeout(Duration::from_millis(500))
        .unwrap_or_default();
    match status.code() {
        Some(0) => Ok(buf),
        Some(c) => Err(format!("exit {c}")),
        None => Err("stopped by a signal".to_string()),
    }
}

/// True when the child has exited, which leaves it to be reaped.
fn exited(pid: i32) -> bool {
    // SAFETY: info is plain data that waitid fills in.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    // With WNOHANG, a child that still runs leaves si_pid at 0. An error
    // means there is no such child to wait for, so the loop must end.
    // SAFETY: si_pid reads the field that waitid set.
    rc != 0 || unsafe { info.si_pid() } != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gives_the_output_of_a_quick_program() {
        let out = run(
            Command::new("sh").args(["-c", "echo hi"]),
            Duration::from_secs(3),
        );
        assert_eq!(out.unwrap(), b"hi\n");
    }

    #[test]
    fn a_missing_program_is_not_found() {
        let out = run(
            &mut Command::new("jlog-no-such-program"),
            Duration::from_secs(3),
        );
        assert_eq!(out.unwrap_err(), "not found");
    }

    #[test]
    fn a_failed_program_gives_its_exit_code() {
        let out = run(
            Command::new("sh").args(["-c", "exit 3"]),
            Duration::from_secs(3),
        );
        assert_eq!(out.unwrap_err(), "exit 3");
    }

    #[test]
    fn a_slow_program_and_its_children_are_killed() {
        let t = Instant::now();
        let out = run(
            Command::new("sh").args(["-c", "sleep 30 & sleep 30"]),
            Duration::from_millis(300),
        );
        assert!(out.unwrap_err().starts_with("no answer"));
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_child_that_keeps_the_pipe_does_not_hold_jlog() {
        let t = Instant::now();
        let out = run(
            Command::new("sh").args(["-c", "echo v1; sleep 30 &"]),
            Duration::from_secs(3),
        );
        assert_eq!(out.unwrap(), b"v1\n");
        assert!(t.elapsed() < Duration::from_secs(2));
    }
}
