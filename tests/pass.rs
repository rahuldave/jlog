//! Labs for `jlog pass`, `jlog step` and `JLOG_RUN`, with a real `just`
//! and a temporary git repository.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{JLOG, records, run_log};
use serde_json::Value;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "lab")
        .env("GIT_AUTHOR_EMAIL", "lab@example.com")
        .env("GIT_COMMITTER_NAME", "lab")
        .env("GIT_COMMITTER_EMAIL", "lab@example.com")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The root Justfile, as the installer would write it: jlog as the shell,
/// one run ID for all inner calls, the steps, and `jlog pass` at the end.
fn justfile(steps: &str) -> String {
    format!(
        "set shell := [\"{JLOG}\", \"sh\", \"-cu\"]\n\
         export JLOG_RUN := env(\"JLOG_RUN\", uuid())\n\
         v := \"1\"\n\
         {steps}\n\
         check: lint typecheck\n  {JLOG} pass\n"
    )
}

const STEPS: &str =
    "lint:\n  true\n  JLOG step lint\ntypecheck:\n  JLOG skip typecheck \"no type checker\"\n";

/// A committed repository with the steps `lint` and `typecheck`, where a
/// profile provides `lint`.
fn repo(steps: &str, provided: &[&str]) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    git(d, &["init", "-q"]);
    fs::create_dir(d.join(".agents")).unwrap();
    let stack = serde_json::json!({"version": 2, "steps": ["lint", "typecheck"], "provided_steps": provided});
    fs::write(d.join(".agents/stack.json"), stack.to_string()).unwrap();
    fs::write(d.join("Justfile"), justfile(&steps.replace("JLOG", JLOG))).unwrap();
    fs::write(d.join("src.txt"), "one\n").unwrap();
    commit(d);
    t
}

fn commit(d: &Path) {
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "c"]);
}

fn just(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = common::cmd("just");
    c.args(args).current_dir(dir).env_remove("JLOG_RUN");
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn passes(d: &Path) -> Vec<Value> {
    records(d, "pass")
}

#[test]
fn just_check_writes_a_pass_for_the_tree_with_one_run() {
    let r = repo(STEPS, &["lint"]);
    let out = just(r.path(), &["check"], &[]);
    assert!(out.status.success(), "{out:?}");
    let p = passes(r.path());
    assert_eq!(p.len(), 1, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(p[0]["tree"], git(r.path(), &["rev-parse", "HEAD^{tree}"]));
    assert_eq!(p[0]["steps"], serde_json::json!(["lint", "typecheck"]));
    let log = run_log(r.path());
    assert!(
        log.iter().all(|l| l["run"] == p[0]["run"]),
        "every line has the run of the pass"
    );
    let trees = records(r.path(), "tree");
    assert_eq!(
        trees.len(),
        1,
        "only the first line of the run logs the tree"
    );
    assert_eq!(trees[0]["tree"], p[0]["tree"]);
    assert_eq!(trees[0]["run"], p[0]["run"]);
}

#[test]
fn no_pass_for_options_variables_or_a_direct_run() {
    let r = repo(STEPS, &["lint"]);
    for (args, env) in [
        (&["--no-deps", "check"][..], &[][..]),
        (&["check"], &[("JUST_NO_DEPS", "true")]),
        (&["v=x", "check"], &[]),
        (&["check"], &[("JUST_DOTENV_FILENAME", "x.env")]),
        (&["check"], &[("JUST_JUSTFILE_NAME", "Justfile")]),
        (&["check"], &[("JUST_NO_DOTENV", "true")]),
    ] {
        let out = just(r.path(), args, env);
        assert!(!out.status.success(), "{args:?} {env:?}");
        assert!(passes(r.path()).is_empty(), "{args:?} {env:?} gave a pass");
    }
    let out = common::cmd(JLOG)
        .arg("pass")
        .current_dir(r.path())
        .env_remove("JLOG_RUN")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("must be a line of `just check`"));
    assert!(passes(r.path()).is_empty());
    assert!(
        !records(r.path(), "no-pass").is_empty(),
        "a refused pass logs its reasons"
    );
}

#[test]
fn no_pass_with_an_unstaged_change_or_an_untracked_file() {
    let r = repo(STEPS, &["lint"]);
    fs::write(r.path().join("src.txt"), "two\n").unwrap();
    let out = just(r.path(), &["check"], &[]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unstaged changes or untracked files: src.txt"));
    // A no-pass prints the times too, after its reasons.
    let reason = err.find("no pass: ").unwrap();
    let times = err.find("jlog: times: lint ").expect("the times line");
    assert!(reason < times, "{err}");
    assert!(
        err[times..].lines().next().unwrap().contains("; total "),
        "{err}"
    );
    git(r.path(), &["checkout", "--", "src.txt"]);
    fs::write(r.path().join("new.txt"), "n\n").unwrap();
    assert!(!just(r.path(), &["check"], &[]).status.success());
    assert!(passes(r.path()).is_empty());
    // A staged change passes: the tree is the tree of the commit to come.
    git(r.path(), &["add", "new.txt"]);
    assert!(just(r.path(), &["check"], &[]).status.success());
    assert_eq!(passes(r.path())[0]["tree"], git(r.path(), &["write-tree"]));
}

#[test]
fn a_skip_of_a_step_that_an_installed_profile_provides_fails_the_pass() {
    // The stub profile provides typecheck, so its skip does not count.
    let r = repo(STEPS, &["lint", "typecheck"]);
    let out = just(r.path(), &["check"], &[]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("`typecheck` was skipped, but an installed profile provides it")
    );
    assert!(passes(r.path()).is_empty());
}

#[test]
fn a_step_without_its_marker_fails_the_pass() {
    let steps = "lint:\n  true\ntypecheck:\n  JLOG skip typecheck \"none\"\n";
    let r = repo(steps, &["lint"]);
    let out = just(r.path(), &["check"], &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("`lint` did not finish"));
}

#[test]
fn just_color_does_not_matter() {
    let r = repo(STEPS, &["lint"]);
    let out = just(r.path(), &["check"], &[("JUST_COLOR", "always")]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(passes(r.path()).len(), 1);
}

#[test]
fn steps_in_inner_just_calls_and_modules_share_the_run_of_the_root() {
    // As the index of jnterfaces does: each step calls just again, here for
    // the module of a profile, which has no JLOG_RUN line of its own.
    let steps = "mod python\n\
                 lint:\n  just --justfile \"{{justfile()}}\" python lint\n\
                 typecheck:\n  just --justfile \"{{justfile()}}\" python typecheck\n";
    let r = repo(steps, &["lint", "typecheck"]);
    let module = format!(
        "set shell := [\"{JLOG}\", \"sh\", \"-cu\"]\n\
         lint:\n  true\n  {JLOG} step lint\n\
         typecheck:\n  true\n  {JLOG} step typecheck\n"
    );
    fs::write(r.path().join("python.just"), module).unwrap();
    commit(r.path());
    let out = just(r.path(), &["check"], &[]);
    assert!(out.status.success(), "{out:?}");
    let p = passes(r.path());
    assert_eq!(p.len(), 1);
    let log = run_log(r.path());
    let inner: Vec<_> = log
        .iter()
        .filter(|l| l["argv"].as_array().is_some_and(|a| a.len() > 2))
        .collect();
    assert!(!inner.is_empty());
    assert!(
        log.iter().all(|l| l["run"] == p[0]["run"]),
        "the inner calls share the run"
    );
    // A recipe of a module, called in the same process, also gets the run.
    let out = just(r.path(), &["python::lint"], &[]);
    assert!(out.status.success(), "{out:?}");
    let last = records(r.path(), "step").pop().unwrap();
    assert_ne!(last["run"], p[0]["run"], "a new call is a new run");
    assert!(uuid_like(last["run"].as_str().unwrap()), "{}", last["run"]);
}

fn uuid_like(s: &str) -> bool {
    s.len() == 36 && s.bytes().filter(|&b| b == b'-').count() == 4
}

#[test]
fn a_run_id_set_by_hand_cannot_reuse_an_old_run_of_another_tree() {
    let r = repo(STEPS, &["lint"]);
    assert!(just(r.path(), &["check"], &[]).status.success());
    let old = passes(r.path())[0]["run"].as_str().unwrap().to_string();
    // An old run of the same tree is a valid run.
    let out = just(r.path(), &["check"], &[("JLOG_RUN", &old)]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(passes(r.path()).len(), 2);
    // The same ID on another tree gives no pass.
    fs::write(r.path().join("src.txt"), "changed\n").unwrap();
    commit(r.path());
    let out = just(r.path(), &["check"], &[("JLOG_RUN", &old)]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("the worktree tree changed during the run, or the run is of another tree")
    );
    assert_eq!(passes(r.path()).len(), 2);
}

#[test]
fn a_change_during_the_run_fails_the_pass() {
    let steps = "lint:\n  echo more >> src.txt\n  git add src.txt\n  JLOG step lint\ntypecheck:\n  JLOG skip typecheck \"none\"\n";
    let r = repo(steps, &["lint"]);
    let out = just(r.path(), &["check"], &[]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("the worktree tree changed during the run")
    );
}

#[test]
fn a_missing_step_list_fails_the_pass() {
    let r = repo(STEPS, &["lint"]);
    fs::write(r.path().join(".agents/stack.json"), "{\"version\": 2}").unwrap();
    commit(r.path());
    let out = just(r.path(), &["check"], &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("has no list `steps`"));
}

#[test]
fn without_jlog_run_the_run_is_the_parent_just() {
    let r = repo(STEPS, &["lint"]);
    fs::write(
        r.path().join("Justfile"),
        format!("set shell := [\"{JLOG}\", \"sh\", \"-cu\"]\na:\n  true\n"),
    )
    .unwrap();
    assert!(just(r.path(), &["a"], &[]).status.success());
    let run = run_log(r.path())[0]["run"].as_str().unwrap().to_string();
    assert!(!uuid_like(&run) && run.split('-').count() == 2, "{run}");
}

#[test]
fn a_bad_step_or_pass_is_a_usage_error() {
    let r = repo(STEPS, &["lint"]);
    for args in [
        &["step"][..],
        &["step", "a", "b"],
        &["step", "a b"],
        &["pass", "x"],
    ] {
        let out = common::cmd(JLOG)
            .args(args)
            .current_dir(r.path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn a_step_in_a_shebang_recipe_is_refused() {
    // The script goes on after the failed line, and its lines do not run
    // through jlog, so its step marker proves nothing.
    let steps = "lint:\n  #!/bin/sh\n  false\n  JLOG step lint\ntypecheck:\n  JLOG skip typecheck \"none\"\n";
    let r = repo(steps, &["lint"]);
    let out = just(r.path(), &["check"], &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("jlog step must be a line of its own"));
    assert!(records(r.path(), "step").is_empty());
    assert!(passes(r.path()).is_empty());
}

#[test]
fn a_backtick_of_the_outer_just_does_not_take_the_run() {
    let steps = "x := `echo hi`\nlint:\n  true\n  JLOG step lint\ntypecheck:\n  JLOG skip typecheck \"none\"\n";
    let r = repo(steps, &["lint"]);
    let out = just(r.path(), &["check"], &[]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        records(r.path(), "tree").len(),
        1,
        "the backtick computed no tree"
    );
    assert_eq!(passes(r.path()).len(), 1);
}

#[test]
fn a_run_id_with_quotes_can_pass() {
    let r = repo(STEPS, &["lint"]);
    let out = just(r.path(), &["check"], &[("JLOG_RUN", "a \"quoted\" \\ run")]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(passes(r.path())[0]["run"], "a \"quoted\" \\ run");
}

#[test]
fn parallel_steps_start_after_the_tree_of_the_run() {
    // `typecheck` changes and stages a file. With [parallel], the first tree
    // must still be the tree before any line ran, so the run gives no pass.
    let steps = "lint:\n  true\n  JLOG step lint\n\
                 typecheck:\n  echo changed > src.txt\n  git add src.txt\n  JLOG skip typecheck \"none\"\n";
    let r = repo(steps, &["lint"]);
    let text = fs::read_to_string(r.path().join("Justfile")).unwrap();
    fs::write(
        r.path().join("Justfile"),
        text.replace("check: lint typecheck", "[parallel]\ncheck: lint typecheck"),
    )
    .unwrap();
    commit(r.path());
    let head = git(r.path(), &["rev-parse", "HEAD^{tree}"]);
    for _ in 0..3 {
        git(r.path(), &["reset", "-q", "--hard", "HEAD"]);
        just(r.path(), &["check"], &[]);
        assert_eq!(records(r.path(), "tree").last().unwrap()["tree"], head);
    }
    assert!(passes(r.path()).is_empty());
}

#[test]
fn a_pass_from_a_program_that_a_line_runs_does_not_count() {
    // Two shells that do not exec put jlog pass two levels under the line.
    let r = repo(STEPS, &["lint"]);
    let text = fs::read_to_string(r.path().join("Justfile")).unwrap();
    let deep = format!("sh -c 'true; sh -c \"true; {JLOG} pass\"'");
    fs::write(
        r.path().join("Justfile"),
        text.replace(&format!("{JLOG} pass"), &deep),
    )
    .unwrap();
    commit(r.path());
    let out = just(r.path(), &["check"], &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("must be a line of `just check`"));
    assert!(passes(r.path()).is_empty());
}

/// A repository whose `lint` step runs one program, which goes on after a
/// failure and then runs `jlog step lint` itself.
fn repo_with_program(name: &str, body: &str) -> tempfile::TempDir {
    let steps = format!("lint:\n  ./{name}\ntypecheck:\n  JLOG skip typecheck \"none\"\n");
    let r = repo(&steps, &["lint"]);
    let path = r.path().join(name);
    fs::write(&path, body.replace("JLOG", JLOG)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    commit(r.path());
    r
}

fn refused(r: &Path) {
    let out = just(r, &["check"], &[]);
    assert!(!out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("must be a line of its own"),
        "{out:?}"
    );
    assert!(passes(r).is_empty());
}

#[test]
fn a_step_from_a_script_that_the_line_runs_is_refused() {
    let r = repo_with_program("run.sh", "#!/bin/sh\nfalse\necho goes on\nJLOG step lint\n");
    refused(r.path());
}

#[test]
fn a_step_from_a_python_program_that_the_line_runs_is_refused() {
    let body = "#!/usr/bin/env python3\nimport subprocess\nsubprocess.run(['false'])\nsubprocess.run(['JLOG', 'step', 'lint'])\n";
    let r = repo_with_program("suite.py", body);
    refused(r.path());
}

#[test]
fn a_step_in_a_script_recipe_run_by_jlog_is_refused() {
    let steps = "set script-interpreter := [\"JLOG\", \"sh\", \"-u\"]\n\
                 [script]\nlint:\n  false\n  echo goes on\n  JLOG step lint\n\
                 typecheck:\n  JLOG skip typecheck \"none\"\n";
    let r = repo(steps, &["lint"]);
    refused(r.path());
}

#[test]
fn a_step_after_another_command_on_its_line_is_refused() {
    let steps = "lint:\n  false || JLOG step lint\ntypecheck:\n  JLOG skip typecheck \"none\"\n";
    let r = repo(steps, &["lint"]);
    refused(r.path());
}

#[test]
fn a_shell_with_joined_flags_and_pipefail_can_pass() {
    let r = repo(STEPS, &["lint"]);
    let text = fs::read_to_string(r.path().join("Justfile")).unwrap();
    let shell = format!("[\"{JLOG}\", \"bash\", \"-euo\", \"pipefail\", \"-c\"]");
    let text = text.replace(&format!("[\"{JLOG}\", \"sh\", \"-cu\"]"), &shell);
    assert!(text.contains("pipefail"));
    fs::write(r.path().join("Justfile"), text).unwrap();
    commit(r.path());
    let out = just(r.path(), &["check"], &[]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(passes(r.path()).len(), 1);
}

/// The files in `dir` and its subfolders. A test compares the sets, not
/// the counts, because git may pack or prune old objects meanwhile.
fn files(dir: &Path) -> std::collections::BTreeSet<PathBuf> {
    let mut out = std::collections::BTreeSet::new();
    for e in fs::read_dir(dir).unwrap().flatten() {
        if e.file_type().unwrap().is_dir() {
            out.extend(files(&e.path()));
        } else {
            out.insert(e.path());
        }
    }
    out
}

/// The files that are in `after` but not in `before`.
fn new_files(
    before: &std::collections::BTreeSet<PathBuf>,
    after: &std::collections::BTreeSet<PathBuf>,
) -> Vec<PathBuf> {
    after.difference(before).cloned().collect()
}

#[test]
fn a_run_leaves_no_temporary_folder_and_no_new_object() {
    let r = repo(STEPS, &["lint"]);
    fs::write(r.path().join("src.txt"), "an edit\n").unwrap();
    fs::write(r.path().join("untracked.txt"), "new\n").unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let before = files(&r.path().join(".git/objects"));
    let out = just(
        r.path(),
        &["check"],
        &[("TMPDIR", tmp.path().to_str().unwrap())],
    );
    assert!(!out.status.success(), "the unstaged edit fails the pass");
    assert_eq!(records(r.path(), "tree").len(), 1);
    assert_eq!(
        new_files(&before, &files(&r.path().join(".git/objects"))),
        Vec::<PathBuf>::new()
    );
    // Only the private folder of the user stays, and it is empty.
    let mut top: Vec<_> = fs::read_dir(tmp.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    top.sort();
    let own = format!("jlog-{}", unsafe { libc::geteuid() });
    assert_eq!(top, vec![own.clone()]);
    let left: Vec<_> = fs::read_dir(tmp.path().join(own))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// A `git` first in PATH that acts as git before 2.31: `rev-parse` prints
/// the unknown flag `--path-format=absolute` back and gives relative paths.
#[test]
fn git_older_than_2_31_still_gives_the_tree() {
    let r = repo(STEPS, &["lint"]);
    let real = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let bin = tempfile::tempdir().unwrap();
    let fake = bin.path().join("git");
    fs::write(
        &fake,
        format!(
            "#!/bin/sh\nif [ \"$1\" = rev-parse ] && [ \"$2\" = --path-format=absolute ]; then\n  \
             echo --path-format=absolute; shift 2; exec {real} rev-parse \"$@\"\nfi\nexec {real} \"$@\"\n",
            real = real.trim()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    // A file that git ignores, added with `-f` and not yet committed, is in
    // the tree only when jlog starts from a copy of the real index.
    fs::write(r.path().join(".git/info/exclude"), "ign\n").unwrap();
    fs::write(r.path().join("ign"), "i\n").unwrap();
    git(r.path(), &["add", "-f", "ign"]);
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap()
    );
    let out = just(r.path(), &["check"], &[("PATH", &path)]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(passes(r.path()).len(), 1);
    let trees = records(r.path(), "tree");
    assert_eq!(trees[0]["tree"], git(r.path(), &["write-tree"]));
}

/// In a shared /tmp, another user can take the name `jlog-<uid>` first.
#[test]
fn a_taken_private_folder_still_gives_the_tree() {
    let r = repo(STEPS, &["lint"]);
    let tmp = tempfile::tempdir().unwrap();
    let own = tmp
        .path()
        .join(format!("jlog-{}", unsafe { libc::geteuid() }));
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &own).unwrap();
    let out = just(
        r.path(),
        &["check"],
        &[("TMPDIR", tmp.path().to_str().unwrap())],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(passes(r.path()).len(), 1);
    assert_eq!(
        fs::read_dir(tmp.path()).unwrap().count(),
        2,
        "no folder is left beside the link and its target"
    );
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

/// A pre-commit hook gets `GIT_INDEX_FILE=.git/index`, relative to the top
/// of the worktree, while `.agents` can sit in a subfolder.
#[test]
fn a_relative_index_from_a_hook_is_found_from_a_subfolder() {
    let t = tempfile::tempdir().unwrap();
    let top = t.path();
    git(top, &["init", "-q"]);
    fs::write(top.join("a"), "1\n").unwrap();
    git(top, &["add", "a"]);
    git(top, &["commit", "-q", "-m", "one"]);
    let sub = top.join("sub");
    fs::create_dir_all(sub.join(".agents")).unwrap();
    fs::write(
        sub.join("Justfile"),
        format!("set shell := [\"{JLOG}\", \"sh\", \"-cu\"]\nx:\n  true\n"),
    )
    .unwrap();
    // Only a copy of the real index holds this file: git ignores it.
    fs::write(top.join(".git/info/exclude"), "ign\n.agents/log/\n").unwrap();
    fs::write(top.join("ign"), "i\n").unwrap();
    git(top, &["add", "-f", "ign"]);
    git(top, &["add", "sub"]);
    let out = just(&sub, &["x"], &[("GIT_INDEX_FILE", ".git/index")]);
    assert!(out.status.success(), "{out:?}");
    let trees = records(&sub, "tree");
    assert_eq!(trees[0]["tree"], git(top, &["write-tree"]));
}

fn jlog_times(dir: &Path, args: &[&str]) -> Output {
    let mut c = common::cmd(JLOG);
    c.arg("times")
        .args(args)
        .current_dir(dir)
        .env_remove("JLOG_RUN");
    c.output().unwrap()
}

#[test]
fn each_step_skip_and_pass_logs_its_time() {
    let steps = "lint:\n  sleep 0.3\n  JLOG step lint\n\
                 typecheck:\n  JLOG skip typecheck \"no type checker\"\n";
    let r = repo(steps, &["lint"]);
    let out = just(r.path(), &["check"], &[]);
    assert!(out.status.success(), "{out:?}");
    let tree = &records(r.path(), "tree")[0];
    let step = &records(r.path(), "step")[0];
    let skip = &records(r.path(), "skip")[0];
    let pass = &passes(r.path())[0];
    let lint = step["seconds"].as_f64().unwrap();
    assert!((0.3..10.0).contains(&lint), "{step}");
    // The step starts at the tree, and the skip at the step.
    assert_eq!(step["start"], tree["time"]);
    assert_eq!(skip["start"], step["time"]);
    let gap = skip["at"].as_f64().unwrap() - step["at"].as_f64().unwrap();
    assert!(
        (skip["seconds"].as_f64().unwrap() - gap).abs() < 0.002,
        "{skip}"
    );
    let total = pass["seconds"].as_f64().unwrap();
    assert!(total >= lint, "{pass}");
    assert_eq!(pass["times"][0]["step"], "lint");
    assert_eq!(pass["times"][0]["kind"], "step");
    assert_eq!(pass["times"][0]["seconds"], step["seconds"]);
    assert_eq!(pass["times"][1]["kind"], "skip");
    // `jlog pass` ends `just check` with one line of the times, on stderr.
    let err = String::from_utf8_lossy(&out.stderr);
    let last = err.lines().last().unwrap_or("");
    assert!(
        last.starts_with("jlog: times: lint ")
            && last.contains(", typecheck ")
            && last.contains(" s (skip); total "),
        "{err}"
    );
    let line = run_log(r.path())
        .into_iter()
        .find(|l| l["line"] == "sleep 0.3")
        .unwrap();
    assert!(line["seconds"].as_f64().unwrap() >= 0.3, "{line}");

    let out = jlog_times(r.path(), &[]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    let run = pass["run"].as_str().unwrap();
    assert!(
        text.starts_with(&format!("run {run} (just check): passed\n")),
        "{text}"
    );
    assert!(
        text.contains("  lint ") && text.contains("  skip\n") && text.contains("  total "),
        "{text}"
    );
    // A run given by its ID prints the same table.
    assert_eq!(jlog_times(r.path(), &[run]).stdout, out.stdout);
}

#[test]
fn jlog_times_names_the_step_that_failed_and_its_time() {
    // `check` runs the step `typecheck` as an inner `just` call, as in
    // Gossil, so the failed line names it.
    let steps = "lint:\n  true\n  JLOG step lint\n\
                 typecheck:\n  sleep 0.2\n  false\n  JLOG step typecheck\n";
    let r = repo(steps, &["lint"]);
    let text = fs::read_to_string(r.path().join("Justfile")).unwrap();
    let inner = format!("check: lint\n  just typecheck\n  {JLOG} pass\n");
    let jf = text.replace(&format!("check: lint typecheck\n  {JLOG} pass\n"), &inner);
    assert_ne!(jf, text);
    fs::write(r.path().join("Justfile"), jf).unwrap();
    commit(r.path());
    let out = just(r.path(), &["check"], &[]);
    assert!(!out.status.success());
    assert!(passes(r.path()).is_empty());
    let out = jlog_times(r.path(), &[]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().next().unwrap().ends_with(": failed"), "{text}");
    let failed = text.lines().find(|l| l.ends_with("  failed")).unwrap();
    assert!(failed.trim_start().starts_with("typecheck "), "{text}");
    let secs: f64 = failed.split_whitespace().nth(1).unwrap().parse().unwrap();
    assert!(secs >= 0.2, "{text}");
    assert!(text.ends_with("the line that failed: false\n"), "{text}");
}

#[test]
fn a_failure_outside_a_step_names_no_step() {
    // `setup` is not a step. Its failure must not name `lint`, which never ran.
    let steps = "setup:\n  false\nlint:\n  true\n  JLOG step lint\n\
                 typecheck:\n  JLOG skip typecheck \"none\"\n";
    let r = repo(steps, &["lint"]);
    let text = fs::read_to_string(r.path().join("Justfile")).unwrap();
    fs::write(
        r.path().join("Justfile"),
        text.replace("check: lint", "check: setup lint"),
    )
    .unwrap();
    commit(r.path());
    just(r.path(), &["check"], &[]);
    let text = String::from_utf8_lossy(&jlog_times(r.path(), &[]).stdout).to_string();
    assert!(text.contains("(just check): failed"), "{text}");
    assert!(
        text.contains("  (failed) ") && !text.contains("  lint "),
        "{text}"
    );
}

#[test]
fn jlog_times_from_a_recipe_shows_the_last_check() {
    // `just times` and `just build` after `just check` make runs of their own,
    // which are not checks.
    let steps = format!("{STEPS}times:\n  JLOG times\nbuild:\n  true\nbad:\n  false\n");
    let r = repo(&steps, &["lint"]);
    assert!(just(r.path(), &["check"], &[]).status.success());
    let run = passes(r.path())[0]["run"].as_str().unwrap().to_string();
    just(r.path(), &["build"], &[]);
    // A failed command after the check does not hide it either.
    assert!(!just(r.path(), &["bad"], &[]).status.success());
    assert!(!just(r.path(), &["times", "nosuch"], &[]).status.success());
    let out = just(r.path(), &["times"], &[]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with(&format!("run {run} (just check): passed\n")),
        "{text}"
    );
}

#[test]
fn a_tolerated_failure_names_no_step_of_the_list() {
    // A `-` line fails without ending the recipe. jlog cannot tell, so the
    // run shows as failed, but it names no step that did not run.
    let steps = format!("{STEPS}tol:\n  JLOG step lint\n  -false\n");
    let r = repo(&steps, &["lint"]);
    assert!(just(r.path(), &["tol"], &[]).status.success());
    let text = String::from_utf8_lossy(&jlog_times(r.path(), &[]).stdout).to_string();
    assert!(text.contains("(just tol): failed"), "{text}");
    assert!(
        text.contains("  (failed after lint) ") && !text.contains("typecheck"),
        "{text}"
    );
}

#[test]
fn jlog_times_needs_a_run_of_the_log() {
    let r = repo(STEPS, &["lint"]);
    let out = jlog_times(r.path(), &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    just(r.path(), &["check"], &[]);
    let out = jlog_times(r.path(), &["no-such-run"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no record of the run no-such-run"));
    assert_eq!(jlog_times(r.path(), &["a", "b"]).status.code(), Some(2));
    // jlog times logs nothing.
    let before = run_log(r.path()).len();
    jlog_times(r.path(), &[]);
    assert_eq!(run_log(r.path()).len(), before);
}

#[test]
fn a_cut_off_write_does_not_stop_jlog_times() {
    let r = repo(STEPS, &["lint"]);
    assert!(just(r.path(), &["check"], &[]).status.success());
    let log = r.path().join(".agents/log/just.jsonl");
    let mut bytes = fs::read(&log).unwrap();
    bytes.extend_from_slice(b"{\"kind\":\"line\",\"line\":\"\xc3");
    fs::write(&log, bytes).unwrap();
    let out = jlog_times(r.path(), &[]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("(just check): passed"));
}
