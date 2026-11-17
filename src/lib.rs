//! `jlog` is the shell of each `just` recipe. It runs one line with the real
//! shell, then adds one JSON line to the run log, `.agents/log/just.jsonl`.

use std::ffi::OsString;
use std::io::Write;

pub mod capped;
pub mod commands;
pub mod env;
pub mod logdir;
pub mod pass;
pub mod proc;
pub mod redact;
pub mod resolve;
pub mod session;
pub mod shell;
pub mod time;
pub mod tree;

pub const USAGE: &str = "\
usage: jlog <shell> <flags> <line> [args...]
       jlog run <command> [args...]
       jlog path
       jlog session | env | tools
       jlog skip <step> <reason>
       jlog step <step>
       jlog pass

Runs <line> with <shell> and adds one JSON line to .agents/log/just.jsonl.
A Justfile sets it as its shell:

    set shell := [\"jlog\", \"sh\", \"-cu\"]

The commands log the session, the environment, the versions of the tools,
a step that has nothing to do, the end of a step that passed, or the pass
of `just check`, and print what they log.

Before each of these, jlog resolves the PATH and the environment from the
`.jlog` folders above the current folder and the user defaults.
`jlog run` then runs <command> in place of jlog, with no log line, for
example to start an agent with the project's tools. `jlog path` prints the
scopes and the PATH order, and runs nothing.
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
        Some("path") if args.len() == 1 => inspect(),
        Some("run") => {
            if let Err(e) = resolve::apply_here() {
                return fail(&e);
            }
            exec(&args[1..])
        }
        _ => {
            // Before any thread starts, so the change of the environment
            // races with nothing.
            if let Err(e) = resolve::apply_here() {
                return fail(&e);
            }
            commands::run(args).unwrap_or_else(|| shell::run(args))
        }
    }
}

/// The exit code of a configuration that jlog cannot read. It is reported
/// before anything runs.
pub const CONFIG_ERROR: i32 = 2;

fn fail(e: &resolve::Error) -> i32 {
    let _ = writeln!(std::io::stderr(), "jlog: {e}");
    CONFIG_ERROR
}

/// `jlog run`: runs the command in place of jlog, so its exit, its signals
/// and its terminal are its own. It returns only when the command cannot
/// start.
fn exec(args: &[OsString]) -> i32 {
    use std::os::unix::process::CommandExt;
    let Some((program, rest)) = args.split_first() else {
        let _ = writeln!(std::io::stderr(), "jlog: run needs a command\n{USAGE}");
        return 2;
    };
    let e = std::process::Command::new(program).args(rest).exec();
    let _ = writeln!(
        std::io::stderr(),
        "jlog: could not run `{}`: {e}",
        program.to_string_lossy()
    );
    if e.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    }
}

/// `jlog path`: the scopes, the user defaults, the PATH order and the names
/// of the managed variables. It prints no value of a text variable.
fn inspect() -> i32 {
    use resolve::{EnvValue, Source};
    let cwd = std::env::current_dir().unwrap_or_default();
    let var = |k: &str| std::env::var_os(k);
    let plan = match resolve::plan(&cwd, &var) {
        Ok(p) => p,
        Err(e) => return fail(&e),
    };
    let mut out = String::new();
    let mut say = |line: String| {
        out.push_str(&line);
        out.push('\n');
    };
    let src = |s: &Source| match s {
        Source::Scope(d) => format!("scope {}", d.display()),
        Source::User => "user".to_string(),
    };
    say("scopes, nearest first:".into());
    if plan.scopes.is_empty() {
        say("  none".into());
    }
    for s in &plan.scopes {
        let bin = s.dir.join(resolve::DIR).join("bin").is_dir();
        let mut parts = Vec::new();
        if bin {
            parts.push(".jlog/bin");
        }
        if s.config.is_some() {
            parts.push(".jlog/environment.json");
        }
        if s.config.as_ref().is_some_and(|c| c.root) {
            parts.push("root");
        }
        say(format!("  {}  ({})", s.dir.display(), parts.join(", ")));
    }
    let show = |p: &Option<std::path::PathBuf>, there: bool| match p {
        Some(p) if there => p.display().to_string(),
        Some(p) => format!("{} (absent)", p.display()),
        None => "none (no HOME and no absolute XDG folder)".to_string(),
    };
    say(format!(
        "user bin:  {}",
        show(
            &plan.user_bin,
            plan.user_bin.as_ref().is_some_and(|p| p.is_dir())
        )
    ));
    say(format!(
        "user file: {}",
        show(&plan.user_file, plan.user.is_some())
    ));
    say("PATH, in order:".into());
    for (p, s) in plan.path() {
        say(format!("  {}  [{}]", p.display(), src(&s)));
    }
    let changes = resolve::changes(&plan, &var);
    let path = changes
        .set
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone())
        .or_else(|| var("PATH"))
        .unwrap_or_default();
    let managed = plan.path().len();
    let rest = if path.is_empty() {
        0
    } else {
        path.to_string_lossy()
            .split(':')
            .count()
            .saturating_sub(managed)
    };
    say(format!("  then the inherited PATH, {rest} entries"));
    say("variables, nearest declaration wins:".into());
    let env = plan.env();
    if env.is_empty() {
        say("  none".into());
    }
    for (name, (value, s)) in env {
        match value {
            EnvValue::Path(p) => say(format!("  {name} = {}  [{}]", p.display(), src(&s))),
            EnvValue::Text(_) => say(format!("  {name} = <text, not shown>  [{}]", src(&s))),
        }
    }
    let _ = std::io::stdout().write_all(out.as_bytes());
    0
}
