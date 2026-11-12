//! The commands of jlog other than the shell form: `session`, `env`, `tools`
//! and `skip`, and the work that the first line of a session does.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::Value;

use crate::logdir::{self, LogDir};
use crate::session::Session;
use crate::{capped, env, proc, redact, time};

/// The programs of the tools table in the spec.
pub const TOOLS: [&str; 11] = [
    "git", "just", "uv", "jlog", "jagt", "gest", "gh", "jq", "rg", "ast-grep", "node",
];
pub const TOOLS_LIMIT: Duration = Duration::from_secs(3);
pub const PLUGIN_LIMIT: Duration = Duration::from_secs(5);
/// The longest text that jlog keeps from a plugin or a version.
const MAX_TEXT: usize = 4096;

/// One record of the run log, other than a line: the kind, the run, the
/// session and the time, then the fields of the kind.
#[derive(Serialize)]
struct Record<'a, T: Serialize> {
    kind: &'static str,
    run: Option<&'a str>,
    #[serde(flatten)]
    session: &'a Session,
    time: String,
    #[serde(flatten)]
    body: T,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Tool {
    pub name: &'static str,
    /// The first line of the output of `--version`.
    pub version: Option<String>,
    pub error: Option<String>,
}

#[derive(Serialize)]
struct Tools {
    tools: Vec<Tool>,
}

/// What the plugin of a harness found: the path of the transcript, the model
/// and the parent thread. jlog keeps only these fields of its output.
#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct Plugin {
    pub transcript: Option<String>,
    pub model: Option<String>,
    pub parent: Option<String>,
    pub error: Option<String>,
}

#[derive(Serialize)]
struct Skip {
    argv: Vec<String>,
    step: String,
    reason: String,
}

/// Runs a command, and gives the exit code. None when `args` is not a command.
pub fn run(args: &[OsString]) -> Option<i32> {
    let name = args.first()?.to_str()?;
    let rest = &args[1..];
    let code = match name {
        // With arguments, these names are the shell form, for example
        // `jlog env sh -c LINE`, so a shell named `env` keeps working.
        "session" | "env" | "tools" if !rest.is_empty() => return None,
        "session" => command(|s| {
            let p = plugin(s, plugins_dir().as_deref(), PLUGIN_LIMIT);
            ("session", serde_json::to_value(p))
        }),
        "env" => {
            let dir = here().and_then(Result::ok);
            let snap = env::snapshot_here(dir.as_ref());
            command(|_| ("env", serde_json::to_value(snap)))
        }
        "tools" => command(|_| {
            (
                "tools",
                serde_json::to_value(Tools {
                    tools: tools(TOOLS_LIMIT),
                }),
            )
        }),
        "skip" => skip(rest),
        _ => return None,
    };
    Some(code)
}

fn usage(msg: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "jlog: {msg}\n{}", crate::USAGE);
    2
}

fn warn(msg: std::fmt::Arguments) {
    let _ = writeln!(std::io::stderr(), "jlog: {msg}");
}

/// The run of the nearest `just` above this process, if any.
fn run_here() -> (Option<String>, Option<proc::Proc>) {
    // SAFETY: getppid has no preconditions.
    let just = proc::nearest_just(unsafe { libc::getppid() });
    (
        just.as_ref().map(|j| format!("{}-{}", j.pid, j.start)),
        just,
    )
}

/// The log folder above the current folder: None when there is none.
fn here() -> Option<std::io::Result<LogDir>> {
    let cwd = std::env::current_dir().ok()?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let root = logdir::find_root(&cwd, home.as_deref())?;
    Some(LogDir::open(&root, home.as_deref()))
}

/// Prints one record and adds it to the run log. A missing log folder is no
/// failure, but a failed write is.
fn command(body: impl FnOnce(&Session) -> (&'static str, serde_json::Result<Value>)) -> i32 {
    let session = Session::detect();
    let (run, _) = run_here();
    let (kind, body) = body(&session);
    let record = Record {
        kind,
        run: run.as_deref(),
        session: &session,
        time: time::utc(SystemTime::now()),
        body: body.expect("a record always serializes"),
    };
    write(&record)
}

fn write<T: Serialize>(record: &Record<T>) -> i32 {
    let json = serde_json::to_string(record).expect("a record always serializes");
    let _ = writeln!(std::io::stdout(), "{json}");
    match here() {
        None => 0,
        Some(Ok(dir)) => match dir.append(&json) {
            Ok(()) => 0,
            Err(e) => {
                warn(format_args!("could not write the run log: {e}"));
                1
            }
        },
        Some(Err(e)) => {
            warn(format_args!("could not open the log folder: {e}"));
            1
        }
    }
}

/// `jlog skip <step> <reason>`: a no-op step, with its reason.
fn skip(rest: &[OsString]) -> i32 {
    let [step, reason] = rest else {
        return usage("usage: jlog skip <step> <reason>");
    };
    let Some(step) = step.to_str().filter(|s| is_step(s)) else {
        return usage("a step is a name of letters, digits, `-` and `_`");
    };
    let reason = reason.to_string_lossy();
    if reason.trim().is_empty() {
        return usage("a skip needs a reason");
    }
    let secrets = redact::secrets(std::env::vars_os());
    let session = Session::detect();
    let (run, just) = run_here();
    let argv = just
        .and_then(|j| proc::argv(j.pid))
        .unwrap_or_default()
        .iter()
        .map(|a| redact::redact(a, &secrets))
        .collect();
    let record = Record {
        kind: "skip",
        run: run.as_deref(),
        session: &session,
        time: time::utc(SystemTime::now()),
        body: Skip {
            argv,
            step: redact::redact(step, &secrets),
            reason: redact::redact(&reason, &secrets),
        },
    };
    write(&record)
}

fn is_step(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The version of each program of the tools table, all at once. A missing or
/// slow program gives an error, never a failure. No download starts.
pub fn tools(limit: Duration) -> Vec<Tool> {
    let handles: Vec<_> = TOOLS
        .iter()
        .map(|&name| {
            std::thread::spawn(move || {
                let mut cmd = Command::new(name);
                cmd.arg("--version").env("UV_PYTHON_DOWNLOADS", "never");
                match capped::run(&mut cmd, limit) {
                    Ok(out) => match first_line(&out) {
                        Some(v) => Tool {
                            name,
                            version: Some(v),
                            error: None,
                        },
                        None => Tool {
                            name,
                            version: None,
                            error: Some("no output".into()),
                        },
                    },
                    Err(e) => Tool {
                        name,
                        version: None,
                        error: Some(e),
                    },
                }
            })
        })
        .collect();
    handles
        .into_iter()
        .zip(TOOLS)
        .map(|(h, name)| {
            h.join().unwrap_or(Tool {
                name,
                version: None,
                error: Some("failed".into()),
            })
        })
        .collect()
}

/// The first line that holds a digit, or else the first line that is not
/// empty. A program such as `gest` prints a banner before its version.
fn first_line(out: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(out);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.clone().next()?;
    let line = lines
        .find(|l| l.bytes().any(|b| b.is_ascii_digit()))
        .unwrap_or(first);
    Some(line.chars().take(200).collect())
}

/// The folder of the plugins: `JLOG_PLUGINS`, or `~/.local/share/jlog/plugins`.
pub fn plugins_dir() -> Option<PathBuf> {
    // A relative folder would name a folder inside the repository.
    let var = |k| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .filter(|v| v.is_absolute())
    };
    var("JLOG_PLUGINS").or_else(|| var("HOME").map(|h| h.join(".local/share/jlog/plugins")))
}

/// Runs the plugin of the harness of `session`, as the spec says:
/// `uv run --project <dir> --frozen python <dir>/session.py`. A terminal has
/// no plugin.
pub fn plugin(session: &Session, plugins: Option<&Path>, limit: Duration) -> Plugin {
    if session.harness == "terminal" {
        return Plugin::default();
    }
    let fail = |e: String| Plugin {
        error: Some(e),
        ..Plugin::default()
    };
    let Some(plugins) = plugins else {
        return fail("no plugins folder: HOME and JLOG_PLUGINS are unset".into());
    };
    let dir = plugins.join(session.harness);
    let script = dir.join("session.py");
    if !script.is_file() {
        return fail(format!("no plugin at {}", script.display()));
    }
    let mut cmd = Command::new("uv");
    cmd.arg("run")
        .arg("--project")
        .arg(&dir)
        // No cache: in a sandbox the home can be read-only, and uv would
        // fail to open its cache there. `just install` makes the
        // environment of each plugin, so a run needs no write.
        .args(["--frozen", "--no-cache", "--quiet", "python"])
        .arg(&script);
    // The variables of a repository must not move the environment of the
    // plugin into the repository, for example UV_PROJECT_ENVIRONMENT=.venv.
    for (k, _) in std::env::vars_os() {
        let k8 = k.to_string_lossy();
        if k8.starts_with("UV_") || k8.starts_with("PYTHON") || k8 == "VIRTUAL_ENV" {
            cmd.env_remove(&k);
        }
    }
    cmd.env("UV_PYTHON_DOWNLOADS", "never");
    match capped::run(&mut cmd, limit) {
        Ok(out) => parse_plugin(&out),
        Err(e) => fail(format!("the plugin failed: {e}")),
    }
}

/// Keeps the fields `transcript`, `model` and `parent` of the JSON object
/// that a plugin prints, each a string or null.
pub fn parse_plugin(out: &[u8]) -> Plugin {
    let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(out) else {
        return Plugin {
            error: Some("the plugin did not print one JSON object".into()),
            ..Plugin::default()
        };
    };
    let text = |k: &str| match map.get(k) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.chars().take(MAX_TEXT).collect()),
        _ => None,
    };
    Plugin {
        transcript: text("transcript"),
        model: text("model"),
        parent: text("parent"),
        error: text("error"),
    }
}

/// The work of the first line of a session: it logs the session, the
/// environment and the tools. The plugin and the tools run at the same time.
pub fn first_of_session(dir: &LogDir, run: &str, session: &Session) -> std::io::Result<()> {
    let plugins = plugins_dir();
    let s = session.clone();
    let plugin = std::thread::spawn(move || plugin(&s, plugins.as_deref(), PLUGIN_LIMIT));
    let tools_handle = std::thread::spawn(|| tools(TOOLS_LIMIT));
    let snap = env::snapshot_here(Some(dir));
    let plugin = plugin.join().unwrap_or_default();
    let tools = tools_handle.join().unwrap_or_default();
    let now = time::utc(SystemTime::now());
    let rec = |kind, body: Value| Record {
        kind,
        run: Some(run),
        session,
        time: now.clone(),
        body,
    };
    for r in [
        rec("session", serde_json::to_value(plugin).expect("serializes")),
        rec("env", serde_json::to_value(snap).expect("serializes")),
        rec(
            "tools",
            serde_json::to_value(Tools { tools }).expect("serializes"),
        ),
    ] {
        dir.append(&serde_json::to_string(&r).expect("a record always serializes"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugin_gives_only_its_known_fields() {
        let p = parse_plugin(br#"{"harness":"codex","transcript":"/t.jsonl","model":"m","parent":null,"secret":"x"}"#);
        assert_eq!(
            p,
            Plugin {
                transcript: Some("/t.jsonl".into()),
                model: Some("m".into()),
                ..Plugin::default()
            }
        );
        let p = parse_plugin(br#"{"model": 5}"#);
        assert_eq!(p.model, None);
        assert!(parse_plugin(b"not json").error.is_some());
        assert!(parse_plugin(b"[1]").error.is_some());
    }

    #[test]
    fn steps_are_plain_names() {
        assert!(is_step("fmt-check") && is_step("type_check2"));
        for bad in ["", "a b", "x;y", "../x", &"a".repeat(65)] {
            assert!(!is_step(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_version_is_the_first_line_that_is_not_empty() {
        assert_eq!(
            first_line(b"\n  git version 2.50\nmore"),
            Some("git version 2.50".into())
        );
        assert_eq!(first_line(b" \n"), None);
        assert_eq!(
            first_line("\u{2588}\u{2588}\ngest 0.4.1\n".as_bytes()),
            Some("gest 0.4.1".into())
        );
        assert_eq!(first_line(b"no digits\n"), Some("no digits".into()));
    }

    #[test]
    fn tools_never_fail_and_keep_the_order_of_the_table() {
        let t = tools(Duration::from_secs(3));
        let names: Vec<_> = t.iter().map(|t| t.name).collect();
        assert_eq!(names, TOOLS);
        assert!(t.iter().all(|t| t.version.is_some() != t.error.is_some()));
        assert!(
            t[0].version
                .as_deref()
                .is_some_and(|v| v.starts_with("git version"))
        );
    }

    #[test]
    fn a_terminal_has_no_plugin_and_a_missing_plugin_is_an_error() {
        let term = Session {
            harness: "terminal",
            session: "1-2".into(),
            child: Some(false),
        };
        assert_eq!(plugin(&term, None, PLUGIN_LIMIT), Plugin::default());
        let t = tempfile::tempdir().unwrap();
        let codex = Session {
            harness: "codex",
            session: "t".into(),
            child: None,
        };
        let p = plugin(&codex, Some(t.path()), PLUGIN_LIMIT);
        assert!(p.error.unwrap().starts_with("no plugin at"));
    }
}
