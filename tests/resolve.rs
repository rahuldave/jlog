//! The resolver: the PATH and the environment that jlog builds from the
//! `.jlog` folders and the user defaults, for each form of a call.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{JLOG, run_log};

/// A workspace: a temporary folder with a home and XDG folders of its own,
/// so no test reads or writes the real ones.
struct Ws {
    t: tempfile::TempDir,
}

impl Ws {
    fn new() -> Ws {
        let t = tempfile::tempdir().unwrap();
        let ws = Ws { t };
        for d in ["home", "data", "config"] {
            fs::create_dir_all(ws.p(d)).unwrap();
        }
        ws
    }

    /// The real path, as getcwd gives it, so paths compare equal on macOS.
    fn root(&self) -> PathBuf {
        fs::canonicalize(self.t.path()).unwrap()
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root().join(rel)
    }

    /// A command for jlog, run in `dir`, with a plain PATH and the
    /// workspace's home and XDG folders.
    fn jlog(&self, dir: &str, args: &[&str]) -> Command {
        let mut c = common::cmd(JLOG);
        let dir = self.p(dir);
        fs::create_dir_all(&dir).unwrap();
        c.args(args)
            .current_dir(dir)
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.p("home"))
            .env("XDG_DATA_HOME", self.p("data"))
            .env("XDG_CONFIG_HOME", self.p("config"))
            .env_remove("JLOG_RESOLVED")
            .env_remove("JLOG_RUN");
        c
    }
}

/// A script `name` in `dir` that prints `says`.
fn tool(dir: &Path, name: &str, says: &str) {
    fs::create_dir_all(dir).unwrap();
    let f = dir.join(name);
    fs::write(&f, format!("#!/bin/sh\necho {says}\n")).unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).unwrap();
}

fn config(dir: &Path, json: &str) {
    fs::create_dir_all(dir.join(".jlog")).unwrap();
    fs::write(dir.join(".jlog/environment.json"), json).unwrap();
}

fn out(mut c: impl std::borrow::BorrowMut<Command>) -> Output {
    let c = c.borrow_mut();
    c.output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).trim_end().to_string()
}

fn ok(c: impl std::borrow::BorrowMut<Command>) -> String {
    let o = out(c);
    assert!(o.status.success(), "{o:?}");
    stdout(&o)
}

/// The PATH that a command run by jlog sees, as a list.
fn path_in(ws: &Ws, dir: &str) -> Vec<String> {
    ok(ws.jlog(dir, &["run", "sh", "-c", "printf %s \"$PATH\""]))
        .split(':')
        .map(String::from)
        .collect()
}

#[test]
fn the_nearest_tool_wins_over_parent_user_and_inherited() {
    let ws = Ws::new();
    tool(&ws.p("w/p/.jlog/bin"), "t", "project");
    tool(&ws.p("w/.jlog/bin"), "t", "parent");
    tool(&ws.p("w/.jlog/bin"), "u", "parent");
    tool(&ws.p("data/jlog/bin"), "t", "user");
    tool(&ws.p("data/jlog/bin"), "v", "user");
    assert_eq!(ok(ws.jlog("w/p", &["run", "t"])), "project");
    assert_eq!(ok(ws.jlog("w/p", &["run", "u"])), "parent");
    assert_eq!(ok(ws.jlog("w/p", &["run", "v"])), "user");
    // The inherited PATH comes last, in its order.
    let path = path_in(&ws, "w/p");
    let want = [
        ws.p("w/p/.jlog/bin"),
        ws.p("w/.jlog/bin"),
        ws.p("data/jlog/bin"),
    ];
    let want: Vec<String> = want.iter().map(|p| p.display().to_string()).collect();
    assert_eq!(&path[..3], &want[..]);
    assert_eq!(&path[3..], ["/usr/bin", "/bin"]);
}

#[test]
fn declared_paths_follow_the_bin_of_their_scope_in_order_and_missing_ones_are_skipped() {
    let ws = Ws::new();
    fs::create_dir_all(ws.p("p/.jlog/bin")).unwrap();
    fs::create_dir_all(ws.p("p/b")).unwrap();
    fs::create_dir_all(ws.p("p/a dir")).unwrap();
    fs::create_dir_all(ws.p("user extra")).unwrap();
    config(
        &ws.p("p"),
        r#"{"version": 1, "path": ["b", "missing", "a dir", "b", "./a dir/../b"]}"#,
    );
    fs::create_dir_all(ws.p("config/jlog")).unwrap();
    fs::write(
        ws.p("config/jlog/environment.json"),
        format!(
            r#"{{"version": 1, "path": ["{}"]}}"#,
            ws.p("user extra").display()
        ),
    )
    .unwrap();
    let path = path_in(&ws, "p/sub/deeper");
    let want: Vec<String> = [
        ws.p("p/.jlog/bin"),
        ws.p("p/b"),
        ws.p("p/a dir"),
        ws.p("user extra"),
    ]
    .iter()
    .map(|p| p.display().to_string())
    .collect();
    assert_eq!(&path[..4], &want[..]);
    assert_eq!(&path[4..], ["/usr/bin", "/bin"]);
    assert!(!path.iter().any(String::is_empty));
}

#[test]
fn a_path_with_a_space_and_a_symlinked_tool_work() {
    let ws = Ws::new();
    tool(&ws.p("real tools"), "real", "linked");
    fs::create_dir_all(ws.p("my proj/.jlog/bin")).unwrap();
    std::os::unix::fs::symlink(ws.p("real tools/real"), ws.p("my proj/.jlog/bin/t")).unwrap();
    assert_eq!(ok(ws.jlog("my proj", &["run", "t"])), "linked");
}

#[test]
fn the_search_crosses_git_and_stops_after_root_and_never_looks_at_siblings() {
    let ws = Ws::new();
    tool(&ws.p("top/.jlog/bin"), "above", "top");
    tool(&ws.p("top/w/.jlog/bin"), "shared", "workspace");
    tool(&ws.p("top/w/sibling/.jlog/bin"), "sib", "sibling");
    fs::create_dir_all(ws.p("top/w/p/.git")).unwrap();
    // The project is a git repository; its parent workspace still counts.
    assert_eq!(ok(ws.jlog("top/w/p", &["run", "shared"])), "workspace");
    assert_eq!(ok(ws.jlog("top/w/p", &["run", "above"])), "top");
    assert_eq!(
        out(ws.jlog("top/w/p", &["run", "sib"])).status.code(),
        Some(127)
    );
    // root: true in the workspace hides the folder above it.
    config(&ws.p("top/w"), r#"{"version": 1, "root": true}"#);
    assert_eq!(ok(ws.jlog("top/w/p", &["run", "shared"])), "workspace");
    assert_eq!(
        out(ws.jlog("top/w/p", &["run", "above"])).status.code(),
        Some(127)
    );
}

#[test]
fn a_worktree_finds_the_scopes_above_it() {
    let ws = Ws::new();
    tool(&ws.p("w/.jlog/bin"), "t", "workspace");
    // A worktree has a .git file, not a folder.
    fs::create_dir_all(ws.p("w/wt")).unwrap();
    fs::write(ws.p("w/wt/.git"), "gitdir: /nowhere\n").unwrap();
    assert_eq!(ok(ws.jlog("w/wt/src", &["run", "t"])), "workspace");
}

#[test]
fn the_search_stops_before_home() {
    let ws = Ws::new();
    tool(&ws.p("home/.jlog/bin"), "t", "home");
    tool(&ws.p("home/proj/.jlog/bin"), "p", "proj");
    assert_eq!(ok(ws.jlog("home/proj", &["run", "p"])), "proj");
    assert_eq!(
        out(ws.jlog("home/proj", &["run", "t"])).status.code(),
        Some(127)
    );
}

#[test]
fn it_works_with_no_git_no_agents_and_writes_no_file() {
    let ws = Ws::new();
    tool(&ws.p("p/.jlog/bin"), "t", "ok");
    fs::remove_dir(ws.p("data")).unwrap();
    fs::remove_dir(ws.p("config")).unwrap();
    let before = listing(&ws.root());
    assert_eq!(ok(ws.jlog("p", &["run", "t"])), "ok");
    assert_eq!(ok(ws.jlog("p", &["sh", "-cu", "t"])), "ok");
    assert_eq!(listing(&ws.root()), before);
}

fn listing(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() && !p.is_symlink() {
                stack.push(p.clone());
            }
            out.push(p);
        }
    }
    out.sort();
    out
}

#[test]
fn xdg_falls_back_to_home_when_unset_empty_or_relative() {
    let ws = Ws::new();
    tool(&ws.p("home/.local/share/jlog/bin"), "t", "fallback");
    fs::create_dir_all(ws.p("p")).unwrap();
    for value in [None, Some(""), Some("relative/data")] {
        let mut c = ws.jlog("p", &["run", "t"]);
        match value {
            Some(v) => c.env("XDG_DATA_HOME", v),
            None => c.env_remove("XDG_DATA_HOME"),
        };
        assert_eq!(ok(&mut c), "fallback", "{value:?}");
    }
    // With no home and no absolute XDG folder, there is no user folder.
    let mut c = ws.jlog("p", &["run", "t"]);
    c.env_remove("XDG_DATA_HOME").env_remove("HOME");
    assert_eq!(out(&mut c).status.code(), Some(127));
}

#[test]
fn env_values_are_set_with_paths_absolute_and_the_nearest_wins() {
    let ws = Ws::new();
    fs::create_dir_all(ws.p("config/jlog")).unwrap();
    fs::write(
        ws.p("config/jlog/environment.json"),
        r#"{"version": 1, "env": {"A": "user", "B": "user", "C": "user"}}"#,
    )
    .unwrap();
    config(
        &ws.p("w"),
        r#"{"version": 1, "env": {"B": "outer", "C": "outer"}}"#,
    );
    config(
        &ws.p("w/p"),
        r#"{"version": 1, "env": {"C": "inner", "CARGO_HOME": {"path": ".jlog/cargo"}}}"#,
    );
    let got = ok(ws.jlog(
        "w/p/src",
        &["run", "sh", "-c", "echo \"$A $B $C $CARGO_HOME\""],
    ));
    // The cargo folder does not exist, and is still set, and not made.
    let cargo = ws.p("w/p/.jlog/cargo");
    assert_eq!(got, format!("user outer inner {}", cargo.display()));
    assert!(!cargo.exists());
}

#[test]
fn a_bad_configuration_stops_before_the_command_and_shows_no_value() {
    let ws = Ws::new();
    let cases = [
        ("{", "not valid JSON"),
        (r#"{"version": 2}"#, "version 2 is not supported"),
        (r#"{"root": true}"#, "version is missing"),
        (r#"{"version": 1, "pth": []}"#, "unknown key"),
        (r#"{"version": 1, "path": "bin"}"#, "list of strings"),
        (
            r#"{"version": 1, "env": {"PATH": "sekrit-value-1"}}"#,
            "reserved",
        ),
        (
            r#"{"version": 1, "env": {"JLOG_RUN": "sekrit-value-1"}}"#,
            "reserved",
        ),
        (
            r#"{"version": 1, "env": {"JLOG_RESOLVED": "sekrit-value-1"}}"#,
            "reserved",
        ),
        (
            r#"{"version": 1, "env": {"CODEX_THREAD_ID": "sekrit-value-1"}}"#,
            "reserved",
        ),
        (
            r#"{"version": 1, "env": {"HOME": "sekrit-value-1"}}"#,
            "reserved",
        ),
        (
            r#"{"version": 1, "env": {"XDG_DATA_HOME": {"path": "sekrit"}}}"#,
            "reserved",
        ),
        (
            r#"{"version": 1, "env": {"XDG_CONFIG_HOME": "sekrit-value-1"}}"#,
            "reserved",
        ),
        (
            r#"{"version": 1, "env": {"API_KEY=sekrit-value-1": "x"}}"#,
            "not a valid name",
        ),
        (r#"{"version": 1, "env": {"X": 5}}"#, "string or an object"),
        (
            r#"{"version": 1, "env": {"X": {"path": "a", "b": 1}}}"#,
            "string or an object",
        ),
        (r#"{"version": 1, "root": "yes"}"#, "root must be"),
    ];
    for (json, says) in cases {
        config(&ws.p("p"), json);
        let marker = ws.p("ran");
        let line = format!("touch '{}'", marker.display());
        for args in [
            vec!["run", "sh", "-c", line.as_str()],
            vec!["sh", "-cu", line.as_str()],
            vec!["path"],
        ] {
            let o = out(ws.jlog("p", &args));
            let err = String::from_utf8_lossy(&o.stderr);
            assert_eq!(o.status.code(), Some(2), "{json} {args:?}");
            assert!(err.contains(says), "{json}: {err}");
            assert!(err.contains("environment.json"), "{err}");
            assert!(!err.contains("sekrit"), "{err}");
            assert!(!marker.exists(), "{json} ran the command");
        }
    }
}

#[test]
fn the_user_file_takes_only_absolute_paths() {
    let ws = Ws::new();
    fs::create_dir_all(ws.p("config/jlog")).unwrap();
    for json in [
        r#"{"version": 1, "path": ["rel"]}"#,
        r#"{"version": 1, "env": {"X": {"path": "rel"}}}"#,
    ] {
        fs::write(ws.p("config/jlog/environment.json"), json).unwrap();
        let o = out(ws.jlog("p", &["run", "true"]));
        assert_eq!(o.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&o.stderr).contains("absolute"));
    }
}

#[test]
fn a_nested_resolution_changes_nothing() {
    let ws = Ws::new();
    tool(&ws.p("p/.jlog/bin"), "t", "x");
    config(&ws.p("p"), r#"{"version": 1, "env": {"X": "1"}}"#);
    let show = "printf '%s|%s|%s' \"$PATH\" \"$X\" \"$JLOG_RESOLVED\"";
    let once = ok(ws.jlog("p", &["run", "sh", "-c", show]));
    let twice = ok(ws.jlog("p", &["run", JLOG, "run", JLOG, "sh", "-cu", show]));
    assert_eq!(once, twice);
}

#[test]
fn moving_to_a_sibling_takes_back_only_what_jlog_added() {
    let ws = Ws::new();
    tool(&ws.p("w/a/.jlog/bin"), "ta", "a");
    tool(&ws.p("w/b/.jlog/bin"), "tb", "b");
    config(
        &ws.p("w/a"),
        r#"{"version": 1, "env": {"KEEP": "a", "GONE": "a", "BACK": "a", "MINE": "a"}}"#,
    );
    config(&ws.p("w/b"), r#"{"version": 1, "env": {"KEEP": "b"}}"#);
    // In project A, the caller changes MINE and PATH, then goes to B and
    // calls jlog again.
    let script = format!(
        "export MINE=caller; export PATH=\"/caller/bin:$PATH\"; cd ../b && {JLOG} run sh -c \
         'printf \"%s|%s|%s|%s|%s\" \"$PATH\" \"$KEEP\" \"${{GONE-unset}}\" \"$BACK\" \"$MINE\"'"
    );
    let mut c = ws.jlog("w/a", &["run", "sh", "-c", &script]);
    c.env("BACK", "before");
    let got = ok(&mut c);
    let parts: Vec<&str> = got.split('|').collect();
    let b_bin = ws.p("w/b/.jlog/bin").display().to_string();
    assert_eq!(
        parts[0],
        format!("{b_bin}:/caller/bin:/usr/bin:/bin"),
        "A's bin goes, the caller's entry stays"
    );
    assert_eq!(parts[1], "b");
    assert_eq!(parts[2], "unset", "a variable that A added goes");
    assert_eq!(parts[3], "before", "a variable that A replaced comes back");
    assert_eq!(parts[4], "caller", "the caller's change stays");
}

#[test]
fn a_recipe_line_resolves_and_still_logs() {
    let ws = Ws::new();
    fs::create_dir_all(ws.p("p/.agents")).unwrap();
    tool(&ws.p("p/.jlog/bin"), "t", "from-project");
    let o = out(ws.jlog("p", &["sh", "-cu", "t; exit 3"]));
    assert_eq!(o.status.code(), Some(3));
    assert_eq!(stdout(&o), "from-project");
    let log = run_log(&ws.p("p"));
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["line"], "t; exit 3");
    assert_eq!(log[0]["exit"], 3);
}

#[test]
fn jlog_run_passes_the_exit_and_gives_127_for_a_missing_command() {
    let ws = Ws::new();
    fs::create_dir_all(ws.p("p/.agents")).unwrap();
    let o = out(ws.jlog("p", &["run", "sh", "-c", "exit 7"]));
    assert_eq!(o.status.code(), Some(7));
    assert_eq!(
        out(ws.jlog("p", &["run", "no-such-tool"])).status.code(),
        Some(127)
    );
    assert_eq!(out(ws.jlog("p", &["run"])).status.code(), Some(2));
    // jlog run logs nothing.
    assert!(!ws.p("p/.agents/log/just.jsonl").exists());
}

#[test]
fn the_helpers_of_jlog_see_the_resolved_path() {
    let ws = Ws::new();
    let real_git = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let mark = ws.p("git-called");
    let bin = ws.p("p/.jlog/bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(
        bin.join("git"),
        format!(
            "#!/bin/sh\necho x >> '{}'\nexec {} \"$@\"\n",
            mark.display(),
            real_git.trim()
        ),
    )
    .unwrap();
    fs::set_permissions(bin.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(ws.p("p/.agents")).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(ws.p("p"))
            .status()
            .unwrap()
            .success()
    );
    // The first line of a run records the tree with git.
    let o = out(ws
        .jlog("p", &["sh", "-cu", "true"])
        .env("JLOG_RUN", "r-helpers"));
    assert!(o.status.success(), "{o:?}");
    assert!(
        mark.exists(),
        "the tree record did not use the project's git"
    );
}

#[test]
fn jlog_path_shows_scopes_and_order_and_hides_text_values() {
    let ws = Ws::new();
    fs::create_dir_all(ws.p("w/p/.jlog/bin")).unwrap();
    config(
        &ws.p("w"),
        r#"{"version": 1, "root": true, "env": {"TOKEN": "sekrit-value-1", "D": {"path": "d"}}}"#,
    );
    let text = ok(ws.jlog("w/p", &["path"]));
    assert!(
        text.contains(&format!("{}  (.jlog/bin)", ws.p("w/p").display())),
        "{text}"
    );
    assert!(text.contains("(.jlog/environment.json, root)"), "{text}");
    assert!(
        text.contains(&format!("D = {}", ws.p("w/d").display())),
        "{text}"
    );
    assert!(text.contains("TOKEN = <text, not shown>"), "{text}");
    assert!(!text.contains("sekrit"));
    assert!(
        text.contains("then the inherited PATH, 2 entries"),
        "{text}"
    );
}

#[test]
fn jlog_env_redacts_a_resolved_secret() {
    let ws = Ws::new();
    fs::create_dir_all(ws.p("p/.agents")).unwrap();
    config(
        &ws.p("p"),
        r#"{"version": 1, "env": {"TOKEN": "sekrit-value-1"}}"#,
    );
    let text = ok(ws.jlog("p", &["env"]));
    assert!(text.contains("TOKEN"), "{text}");
    assert!(!text.contains("sekrit"), "{text}");
    let log = fs::read_to_string(ws.p("p/.agents/log/just.jsonl")).unwrap();
    assert!(!log.contains("sekrit"));
    // A line that prints the value is logged with the value redacted.
    ok(ws.jlog("p", &["sh", "-cu", "echo $TOKEN; true sekrit-value-1"]));
    let log = fs::read_to_string(ws.p("p/.agents/log/just.jsonl")).unwrap();
    assert!(!log.contains("sekrit"), "{log}");
}

#[test]
fn an_unset_path_stays_unset_with_nothing_to_add() {
    let ws = Ws::new();
    // env, not sh, because sh sets a PATH of its own when it has none.
    let mut c = ws.jlog("p", &["run", "/usr/bin/env"]);
    c.env_remove("PATH");
    assert!(!ok(&mut c).lines().any(|l| l.starts_with("PATH=")));
}

/// A project whose Justfile runs each line through jlog, with a fake agent
/// in `.jlog/bin` that runs two checks.
fn agent_project(ws: &Ws) -> PathBuf {
    let p = ws.p("w/p");
    fs::create_dir_all(p.join(".agents")).unwrap();
    let just = which("just");
    fs::write(
        p.join("Justfile"),
        format!(
            "set shell := [\"{JLOG}\", \"sh\", \"-cu\"]\n\
             export JLOG_RUN := env(\"JLOG_RUN\", uuid())\n\
             agent:\n    unset JLOG_RUN; exec fake-agent\n\
             check:\n    shared-tool\n    {just} inner\n\
             inner:\n    true\n"
        ),
    )
    .unwrap();
    // The agent prints whether it inherited a run, then runs two checks,
    // as an agent would, from a subfolder.
    let bin = p.join(".jlog/bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(
        bin.join("fake-agent"),
        format!(
            "#!/bin/sh\necho \"run=${{JLOG_RUN-none}}\"\nmkdir -p sub && cd sub && {just} check && {just} check\n"
        ),
    )
    .unwrap();
    fs::set_permissions(bin.join("fake-agent"), fs::Permissions::from_mode(0o755)).unwrap();
    tool(&ws.p("w/.jlog/bin"), "shared-tool", "shared");
    p
}

fn which(name: &str) -> String {
    let o = Command::new("sh")
        .args(["-c", &format!("command -v {name}")])
        .output()
        .unwrap();
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

#[test]
fn an_agent_recipe_gets_the_path_and_each_check_gets_its_own_run() {
    let ws = Ws::new();
    let p = agent_project(&ws);
    // The outer just runs under a run of its own, as when a person types
    // `just agent`.
    let mut c = common::cmd("just");
    c.args(["agent"])
        .current_dir(&p)
        .env(
            "PATH",
            format!(
                "/usr/bin:/bin:{}",
                Path::new(&which("just")).parent().unwrap().display()
            ),
        )
        .env("HOME", ws.p("home"))
        .env("XDG_DATA_HOME", ws.p("data"))
        .env("XDG_CONFIG_HOME", ws.p("config"));
    let o = c.output().unwrap();
    assert!(o.status.success(), "{o:?}");
    let text = stdout(&o);
    assert!(text.starts_with("run=none"), "{text}");
    assert_eq!(text.matches("shared").count(), 2, "{text}");

    let log = run_log(&p);
    let runs = |line: &str| -> Vec<String> {
        log.iter()
            .filter(|r| r["line"] == line)
            .map(|r| r["run"].as_str().unwrap().to_string())
            .collect()
    };
    let checks = runs("shared-tool");
    let inners = runs("true");
    assert_eq!(checks.len(), 2);
    assert_ne!(
        checks[0], checks[1],
        "two checks after the agent start share a run"
    );
    assert_eq!(checks, inners, "a nested recipe left the run of its check");
    let agent = runs("unset JLOG_RUN; exec fake-agent");
    assert_eq!(agent.len(), 1);
    assert!(!checks.contains(&agent[0]));
}

#[test]
fn a_secret_that_a_scope_replaces_stays_redacted() {
    let ws = Ws::new();
    fs::create_dir_all(ws.p("p/.agents")).unwrap();
    config(
        &ws.p("p"),
        r#"{"version": 1, "env": {"API_TOKEN": "sandbox-token-1"}}"#,
    );
    let mut c = ws.jlog(
        "p",
        &["sh", "-cu", "true prod-secret-123456 sandbox-token-1"],
    );
    c.env("API_TOKEN", "prod-secret-123456");
    let o = out(c);
    assert!(o.status.success(), "{o:?}");
    let log = fs::read_to_string(ws.p("p/.agents/log/just.jsonl")).unwrap();
    assert!(!log.contains("prod-secret"), "{log}");
    assert!(!log.contains("sandbox-token"), "{log}");
}

#[test]
fn a_scope_in_a_folder_that_others_can_write_to_is_not_used() {
    let ws = Ws::new();
    // A shared folder, as /tmp is, with a `.jlog` that anyone could add.
    tool(&ws.p("shared/.jlog/bin"), "sh", "HIJACKED");
    fs::create_dir_all(ws.p("shared/p")).unwrap();
    fs::set_permissions(ws.p("shared"), fs::Permissions::from_mode(0o1777)).unwrap();
    assert_eq!(ok(ws.jlog("shared/p", &["sh", "-cu", "echo real"])), "real");
    fs::set_permissions(ws.p("shared"), fs::Permissions::from_mode(0o755)).unwrap();
    // A bin folder that others can write to is skipped, with a warning.
    tool(&ws.p("w/.jlog/bin"), "t", "untrusted");
    fs::set_permissions(ws.p("w/.jlog/bin"), fs::Permissions::from_mode(0o777)).unwrap();
    let o = out(ws.jlog("w/p", &["run", "t"]));
    assert_eq!(o.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&o.stderr).contains("every user can write to it"));
}

#[test]
fn a_path_that_was_unset_is_unset_again_after_a_move() {
    let ws = Ws::new();
    fs::create_dir_all(ws.p("w/p/.jlog/bin")).unwrap();
    fs::create_dir_all(ws.p("w/q")).unwrap();
    let mut c = ws.jlog("w/p", &["run", "/usr/bin/env"]);
    c.env_remove("PATH");
    let env = ok(c);
    let get = |k: &str| {
        env.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .unwrap()
            .to_string()
    };
    assert_eq!(get("PATH"), ws.p("w/p/.jlog/bin").display().to_string());
    // The same environment, now in project q, with no scope.
    let mut c = ws.jlog("w/q", &["run", "/usr/bin/env"]);
    c.env("PATH", get("PATH"))
        .env("JLOG_RESOLVED", get("JLOG_RESOLVED"));
    let env = ok(c);
    assert!(!env.lines().any(|l| l.starts_with("PATH=")), "{env}");
    assert!(
        !env.lines().any(|l| l.starts_with("JLOG_RESOLVED=")),
        "{env}"
    );
}

#[test]
fn a_managed_folder_already_in_the_path_keeps_the_callers_copy_in_place() {
    let ws = Ws::new();
    fs::create_dir_all(ws.p("w/p/.jlog/bin")).unwrap();
    fs::create_dir_all(ws.p("w/q")).unwrap();
    let bin = ws.p("w/p/.jlog/bin").display().to_string();
    let script =
        format!("printf '%s|' \"$PATH\"; cd ../q && {JLOG} run sh -c 'printf %s \"$PATH\"'");
    let mut c = ws.jlog("w/p", &["run", "sh", "-c", &script]);
    c.env("PATH", format!("/usr/bin:{bin}:/bin"));
    let got = ok(c);
    let (here, there) = got.split_once('|').unwrap();
    assert_eq!(here, format!("{bin}:/usr/bin:{bin}:/bin"));
    assert_eq!(
        there,
        format!("/usr/bin:{bin}:/bin"),
        "the caller lost its order"
    );
}

#[test]
fn a_bad_record_from_the_caller_is_ignored() {
    let ws = Ws::new();
    let mut c = ws.jlog("p", &["run", "true"]);
    c.env("A", "x").env(
        "JLOG_RESOLVED",
        r#"{"path":["zz"],"env":{"A=B":{"set":"x"},"":{"set":"y"}}}"#,
    );
    let o = out(c);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let mut c = ws.jlog("p", &["run", "true"]);
    c.env("JLOG_RESOLVED", "not json");
    assert_eq!(out(c).status.code(), Some(0));
}

#[test]
fn jlog_path_counts_no_inherited_entry_for_an_empty_path() {
    let ws = Ws::new();
    let mut c = ws.jlog("p", &["path"]);
    c.env("PATH", "");
    assert!(ok(c).contains("then the inherited PATH, 0 entries"));
}

#[test]
fn a_group_may_write_to_a_scope_and_a_skipped_scope_ends_the_search() {
    let ws = Ws::new();
    tool(&ws.p("w/p/.jlog/bin"), "t", "grouped");
    for d in ["w/p/.jlog", "w/p/.jlog/bin"] {
        fs::set_permissions(ws.p(d), fs::Permissions::from_mode(0o775)).unwrap();
    }
    assert_eq!(ok(ws.jlog("w/p", &["run", "t"])), "grouped");
    // A scope that every user can write to is skipped, and so is each
    // scope above it, since it might have said root: true.
    tool(&ws.p("w/.jlog/bin"), "above", "above");
    fs::set_permissions(ws.p("w/p/.jlog/bin"), fs::Permissions::from_mode(0o777)).unwrap();
    let o = out(ws.jlog("w/p", &["run", "above"]));
    assert_eq!(o.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&o.stderr).contains("every user can write to it"));
}

#[test]
fn a_record_by_hand_cannot_take_back_a_reserved_name() {
    let ws = Ws::new();
    let mut c = ws.jlog("p", &["run", "sh", "-c", "printf %s \"$PATH\""]);
    c.env(
        "JLOG_RESOLVED",
        r#"{"path":[],"env":{"PATH":{"set":"/usr/bin:/bin","prev":"/evil"}}}"#,
    );
    assert_eq!(ok(c), "/usr/bin:/bin");
}
