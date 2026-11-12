//! The shell form, `jlog <shell> <flags> <line> [args...]`, which `just` calls
//! for each line of a recipe and for each backtick.

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::SystemTime;

use serde::Serialize;

use crate::{logdir, proc, redact, time};

/// One line of the run log.
#[derive(Debug, Serialize)]
pub struct Line {
    /// The process ID of `just` and its start time, joined by `-`.
    pub run: String,
    /// The `just` command as a list, with secrets redacted.
    pub argv: Vec<String>,
    /// The line of the recipe, with secrets redacted. It is null when jlog
    /// cannot tell which argument is the line.
    pub line: Option<String>,
    pub exit: i32,
    pub start: String,
    pub end: String,
    /// `line` for a line of a recipe, `backtick` for a backtick.
    pub kind: &'static str,
}

/// The index of the line in the arguments after the shell, or None when
/// jlog cannot be sure. A null line can only make a gate fail, while a wrong
/// line could make it pass, so jlog knows a short list of forms and gives
/// None for every other one.
///
/// The shell must be `sh`, `bash`, `zsh`, `dash` or `python`. Before the
/// line come only groups of short flags from the safe letters of that shell,
/// such as `-eu` or `+x`, and `-o NAME` or `+o NAME` in the sh family, `-O
/// NAME` in bash, and `-W VALUE` or `-X VALUE` in python. The last group
/// holds `c`, and the line is the next argument. The safe letters leave out
/// every flag that stops the line from running, such as `-n`, `-h` or `-V`.
/// A long option, `--`, a script, or a line of the sh family that starts
/// with `-` or `+` gives None.
pub fn line_index(shell: &std::ffi::OsStr, after_shell: &[OsString]) -> Option<usize> {
    let name = std::path::Path::new(shell).file_name()?.to_str()?;
    let kind = Kind::of(name)?;
    let arg = |k: usize| after_shell.get(k).and_then(|a| a.to_str());
    let mut i = 0;
    loop {
        let a = arg(i)?;
        if kind.takes_value(a) {
            if !kind.safe_value(a, arg(i + 1)?) {
                return None;
            }
            i += 2;
            continue;
        }
        let group = a.strip_prefix('-').or_else(|| match kind {
            Kind::Python => None,
            _ => a.strip_prefix('+'),
        })?;
        if group.is_empty() || !group.bytes().all(|b| kind.safe(b)) {
            return None;
        }
        if group.contains('c') {
            // A + group cannot hold c, and in python c must end its group.
            if a.starts_with('+') || (kind == Kind::Python && !group.ends_with('c')) {
                return None;
            }
            let line = arg(i + 1)?;
            if kind != Kind::Python && (line.starts_with('-') || line.starts_with('+')) {
                return None;
            }
            return Some(i + 1);
        }
        i += 1;
    }
}

/// The shells whose arguments jlog knows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Sh,
    Bash,
    Zsh,
    Python,
}

impl Kind {
    fn of(name: &str) -> Option<Kind> {
        match name {
            "sh" | "dash" => Some(Kind::Sh),
            "bash" => Some(Kind::Bash),
            "zsh" => Some(Kind::Zsh),
            // python3 or python3.N. A bare python can be Python 2.
            _ if name == "python3"
                || name
                    .strip_prefix("python3.")
                    .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())) =>
            {
                Some(Kind::Python)
            }
            _ => None,
        }
    }

    /// The letters that leave the line to run as a command. For `sh` and
    /// `dash`, only the letters that both know.
    fn safe(self, b: u8) -> bool {
        let letters: &[u8] = match self {
            Kind::Sh => b"aCcefuvx",
            // No -t: it stops after one command.
            Kind::Bash => b"aBCcEefHhkmPpTuvx",
            Kind::Zsh => b"cdefuvx",
            Kind::Python => b"BbcdEIOPqRSsu",
        };
        letters.contains(&b)
    }

    /// True for a value of `opt` that lets the line run, in every version of
    /// the shell that jlog expects: bash 3.2 and later, zsh 5, and a dash
    /// with no pipefail. Any other value, such as `noexec` or a name that the
    /// shell does not know, gives no line.
    fn safe_value(self, opt: &str, value: &str) -> bool {
        const SH: &[&str] = &[
            "allexport",
            "errexit",
            "noclobber",
            "noglob",
            "nounset",
            "verbose",
            "xtrace",
        ];
        let list: &[&str] = match (self, opt) {
            (Kind::Sh, _) => SH,
            (Kind::Bash, "-o" | "+o") => &[
                "allexport",
                "errexit",
                "errtrace",
                "functrace",
                "noclobber",
                "noglob",
                "nounset",
                "pipefail",
                "verbose",
                "xtrace",
            ],
            (Kind::Bash, _) => &["dotglob", "extglob", "failglob", "nocaseglob", "nullglob"],
            (Kind::Zsh, _) => &["errexit", "nounset", "pipefail", "verbose", "xtrace"],
            // A bad -W value only warns. Most -X values can stop python.
            (Kind::Python, "-W") => return !value.starts_with('-'),
            (Kind::Python, _) => &["dev", "utf8"],
        };
        list.contains(&value)
    }

    /// An option whose value is the next argument.
    fn takes_value(self, a: &str) -> bool {
        match self {
            Kind::Sh | Kind::Zsh => matches!(a, "-o" | "+o"),
            Kind::Bash => matches!(a, "-o" | "+o" | "-O" | "+O"),
            Kind::Python => matches!(a, "-W" | "-X"),
        }
    }
}

/// Writes a warning. A closed stderr must not stop jlog, so errors are dropped.
fn warn(msg: std::fmt::Arguments) {
    let _ = writeln!(std::io::stderr(), "jlog: {msg}");
}

/// The process ID of the shell, for the signal handler. It is 0 before the
/// spawn and after the wait.
static CHILD: AtomicI32 = AtomicI32::new(0);
/// A signal that came before the shell had a process ID.
static PENDING: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward(sig: libc::c_int) {
    let pid = CHILD.load(Ordering::SeqCst);
    if pid > 0 {
        // SAFETY: kill is safe to call in a signal handler.
        unsafe {
            libc::kill(pid, sig);
        }
    } else {
        PENDING.store(sig, Ordering::SeqCst);
    }
}

pub fn run(args: &[OsString]) -> i32 {
    let start = SystemTime::now();
    let (shell, rest) = args.split_first().expect("run() checks for an argument");

    // Before the spawn, so that no signal is lost. exec sets a caught signal
    // back to its default action, so the shell is not affected.
    catch_signals();
    let mut started = false;
    let code = match Command::new(shell).args(rest).spawn() {
        Ok(mut child) => {
            started = true;
            adopt(child.id() as i32);
            wait_until_exited(child.id() as i32);
            // The shell has exited but is not reaped, so its process ID is
            // not free yet. Clear it before the reap, so a late signal is not
            // sent to another process.
            CHILD.store(0, Ordering::SeqCst);
            match child.wait() {
                Ok(status) => match (status.code(), status.signal()) {
                    (Some(c), _) => Exit::Code(c),
                    (None, Some(s)) => Exit::Signal(s),
                    (None, None) => Exit::Code(1),
                },
                Err(e) => {
                    warn(format_args!("could not wait for the shell: {e}"));
                    Exit::Code(1)
                }
            }
        }
        Err(e) => {
            warn(format_args!(
                "could not run the shell `{}`: {e}",
                shell.to_string_lossy()
            ));
            Exit::Code(127)
        }
    };
    let end = SystemTime::now();

    log(shell, rest, code.number(), start, end, started);
    code.finish()
}

enum Exit {
    Code(i32),
    Signal(i32),
}

impl Exit {
    fn number(&self) -> i32 {
        match *self {
            Exit::Code(c) => c,
            Exit::Signal(s) => 128 + s,
        }
    }

    /// Ends as the shell ended. For a signal, jlog sends the same signal to
    /// itself, so `just` sees the signal and not a plain exit code. A signal
    /// that dumps core, such as SIGSEGV, becomes the exit code 128 plus the
    /// signal instead, so that jlog itself does not crash and leave a report.
    fn finish(self) -> i32 {
        if let Exit::Signal(s) = self
            && reraise(s)
        {
            // SAFETY: signal and raise have no memory preconditions.
            unsafe {
                libc::signal(s, libc::SIG_DFL);
                libc::raise(s);
            }
        }
        self.number()
    }
}

/// True for a signal that jlog sends to itself, false for one that dumps core.
pub fn reraise(sig: i32) -> bool {
    ![
        libc::SIGQUIT,
        libc::SIGILL,
        libc::SIGTRAP,
        libc::SIGABRT,
        libc::SIGFPE,
        libc::SIGBUS,
        libc::SIGSEGV,
        libc::SIGSYS,
        libc::SIGXCPU,
        libc::SIGXFSZ,
        #[cfg(target_os = "macos")]
        libc::SIGEMT,
    ]
    .contains(&sig)
}

extern "C" fn ignore(_sig: libc::c_int) {}

/// Records the process ID of the shell for the handler, and sends on a
/// signal that came before the shell had one.
fn adopt(pid: i32) {
    CHILD.store(pid, Ordering::SeqCst);
    let early = PENDING.swap(0, Ordering::SeqCst);
    if early != 0 {
        // SAFETY: kill has no memory preconditions.
        unsafe { libc::kill(pid, early) };
    }
}

/// Waits until the shell exits, and leaves it to be reaped.
fn wait_until_exited(pid: i32) {
    loop {
        // SAFETY: info is plain data that waitid fills in.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if rc == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return;
        }
    }
}

/// jlog sends SIGTERM and SIGHUP on to the shell. `just` sends SIGTERM to
/// its child only, which is jlog, and a program that runs jlog can send
/// either signal to jlog alone. Then jlog waits, logs the exit, and ends as
/// the shell ended.
///
/// For ^C and ^\, jlog acts like system(3): the terminal sends them to the
/// shell too, so jlog only waits. It catches them with a handler that does
/// nothing, and does not ignore them, because the shell would keep an
/// ignored signal.
fn catch_signals() {
    let set = |sig: libc::c_int, handler: extern "C" fn(libc::c_int)| {
        // SAFETY: each handler only calls kill or stores an atomic, and the
        // sigaction structs are zeroed.
        unsafe {
            // A signal that the caller ignores, as under nohup, stays ignored,
            // so the shell keeps ignoring it, as it would without jlog.
            let mut old: libc::sigaction = std::mem::zeroed();
            libc::sigaction(sig, std::ptr::null(), &mut old);
            if old.sa_sigaction == libc::SIG_IGN {
                return;
            }
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = handler as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    };
    set(libc::SIGTERM, forward);
    set(libc::SIGHUP, forward);
    set(libc::SIGINT, ignore);
    set(libc::SIGQUIT, ignore);
}

fn log(
    shell: &std::ffi::OsStr,
    after_shell: &[OsString],
    exit: i32,
    start: SystemTime,
    end: SystemTime,
    started: bool,
) {
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let Some(root) = logdir::find_root(&cwd, home.as_deref()) else {
        return;
    };

    // SAFETY: getppid has no preconditions.
    let parent = unsafe { libc::getppid() };
    // `just` runs its shell itself, so in the shell form only the parent
    // counts, not a `just` higher up. Outside `just`, the run is the parent,
    // and the argv stays empty: the command line of another program can
    // hold a secret that is not in the environment.
    let parent_info = proc::info(parent);
    let since = parent_info.as_ref().map_or(0, |p| p.start);
    let argv = match &parent_info {
        Some(p) if p.name == "just" => proc::argv(parent).unwrap_or_default(),
        _ => Vec::new(),
    };
    let run = format!("{parent}-{since}");
    let kind = if proc::stdout_is_own_pipe(parent) {
        "backtick"
    } else {
        "line"
    };

    let secrets = redact::secrets(std::env::vars_os());
    // A shell that did not start ran no line, so the line is null.
    let line = line_index(shell, after_shell)
        .filter(|_| started)
        .map(|i| redact::redact(&after_shell[i].to_string_lossy(), &secrets));
    let entry = Line {
        run,
        argv: argv.iter().map(|a| redact::redact(a, &secrets)).collect(),
        line,
        exit,
        start: time::utc(start),
        end: time::utc(end),
        kind,
    };
    let json = serde_json::to_string(&entry).expect("a Line always serializes");
    if let Err(e) = logdir::append(&root, home.as_deref(), &json) {
        warn(format_args!(
            "could not write the run log in {}: {e}",
            root.join(logdir::LOG).display()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(v: &[&str]) -> Option<usize> {
        idx_for("sh", v)
    }

    fn idx_for(shell: &str, v: &[&str]) -> Option<usize> {
        let args: Vec<OsString> = v.iter().map(OsString::from).collect();
        line_index(std::ffi::OsStr::new(shell), &args)
    }

    #[test]
    fn finds_the_line_in_the_known_forms() {
        assert_eq!(idx(&["-cu", "echo hi"]), Some(1));
        assert_eq!(idx(&["-c", "echo hi", "name", "a"]), Some(1));
        assert_eq!(
            idx_for("bash", &["-eu", "-o", "pipefail", "-c", "x"]),
            Some(4)
        );
        assert_eq!(idx_for("bash", &["-O", "extglob", "-euc", "x"]), Some(3));
        assert_eq!(idx_for("bash", &["-Eeuc", "x"]), Some(1));
        assert_eq!(idx_for("/bin/bash", &["+x", "-c", "x"]), Some(2));
        assert_eq!(idx_for("dash", &["-ec", "x"]), Some(1));
        assert_eq!(idx_for("zsh", &["-dfc", "x"]), Some(1));
        assert_eq!(idx_for("zsh", &["-o", "pipefail", "-c", "x"]), Some(3));
        assert_eq!(idx_for("python3", &["-c", "print(1)"]), Some(1));
        assert_eq!(idx_for("python3.12", &["-Ic", "x"]), Some(1));
        assert_eq!(
            idx_for("python3", &["-W", "ignore", "-u", "-c", "x"]),
            Some(4)
        );
        assert_eq!(idx_for("python3", &["-X", "dev", "-c", "x"]), Some(3));
        let bash = ["-o", "errexit", "+o", "xtrace", "-c", "x"];
        assert_eq!(idx_for("bash", &bash), Some(5));
        assert_eq!(
            idx_for("python3", &["-c", "-x"]),
            Some(1),
            "python takes any code"
        );
    }

    #[test]
    fn every_doubtful_form_gives_no_line() {
        let none: &[(&str, &[&str])] = &[
            // Shells that jlog does not know.
            ("pwsh", &["-Command", "x"]),
            ("cmd", &["/C", "dir"]),
            ("node", &["-e", "x"]),
            ("fish", &["-c", "x"]),
            // Options after -c, and values of o or O.
            ("sh", &["-c", "-eu", "x"]),
            ("bash", &["-c", "-eo", "pipefail", "x"]),
            ("bash", &["-c", "-", "x"]),
            ("sh", &["-c", "--", "x"]),
            ("bash", &["-eo", "pipefail", "-c", "x"]),
            ("bash", &["-oerrexit", "-c", "x"]),
            // A line that the sh family would read as an option.
            ("bash", &["-c", "-x date", "echo name"]),
            ("bash", &["-c", "+x date", "echo name"]),
            // Something other than an option before -c.
            ("bash", &["s.sh", "-c", "x"]),
            ("bash", &["--", "-c", "x"]),
            ("bash", &["--rcfile", "-c", "x"]),
            ("bash", &["--norc", "-c", "x"]),
            ("bash", &["--version", "-c", "x"]),
            ("python3", &["-m", "json.tool", "-c", "x"]),
            ("python3", &["--", "-c", "x"]),
            ("python3", &["+x", "-c", "x"]),
            ("python3", &["-W", "-c", "x"]),
            // Values that stop the line from running, or that the shell does not know.
            ("bash", &["-o", "noexec", "-c", "x"]),
            ("sh", &["-o", "noexec", "-c", "x"]),
            ("dash", &["-o", "noexec", "-c", "x"]),
            ("zsh", &["-o", "noexec", "-c", "x"]),
            ("zsh", &["-o", "no_exec", "-c", "x"]),
            ("zsh", &["+o", "exec", "-c", "x"]),
            ("bash", &["+o", "exec", "-c", "x"]),
            ("bash", &["-o", "bogus", "-c", "x"]),
            ("bash", &["-O", "bogus", "-c", "x"]),
            ("bash", &["-O", "globstar", "-c", "x"]),
            ("dash", &["-o", "pipefail", "-c", "x"]),
            ("python3", &["-X", "int_max_str_digits=abc", "-c", "x"]),
            ("python3", &["-X", "frozen_modules=bad", "-c", "x"]),
            // Names that are not python3.
            ("python", &["-c", "x"]),
            ("python3.", &["-c", "x"]),
            ("python2.7", &["-c", "x"]),
            // Flags that stop the line from running, or change their meaning.
            ("bash", &["-nc", "x"]),
            ("bash", &["-Dc", "x"]),
            ("zsh", &["-nc", "x"]),
            ("zsh", &["-b", "-c", "x"]),
            ("zsh", &["-O", "extglob", "-c", "x"]),
            ("sh", &["-O", "extglob", "-c", "x"]),
            ("python3", &["-Vc", "x"]),
            ("python3", &["-hc", "x"]),
            ("python3", &["-cu", "x"]),
            ("python3", &["-euc", "x"]),
            ("bash", &["-tc", "x"]),
            // No line after the flag.
            ("sh", &["-c"]),
            ("sh", &["-eu"]),
        ];
        for (shell, args) in none {
            assert_eq!(idx_for(shell, args), None, "{shell} {args:?}");
        }
    }

    #[test]
    fn a_signal_before_the_spawn_reaches_the_shell() {
        use std::os::unix::process::ExitStatusExt;
        // The handler runs before the shell has a process ID.
        CHILD.store(0, Ordering::SeqCst);
        forward(libc::SIGTERM);
        assert_eq!(PENDING.load(Ordering::SeqCst), libc::SIGTERM);
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let t0 = std::time::Instant::now();
        adopt(child.id() as i32);
        wait_until_exited(child.id() as i32);
        CHILD.store(0, Ordering::SeqCst);
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGTERM));
        assert!(t0.elapsed().as_secs() < 3);
        assert_eq!(PENDING.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn signals_that_dump_core_are_not_raised_again() {
        assert!(reraise(libc::SIGTERM));
        assert!(reraise(libc::SIGINT));
        assert!(reraise(libc::SIGHUP));
        assert!(!reraise(libc::SIGSEGV));
        assert!(!reraise(libc::SIGABRT));
        assert!(!reraise(libc::SIGXCPU));
        assert!(!reraise(libc::SIGXFSZ));
    }
}
