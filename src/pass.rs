//! `jlog pass`, the last line of `just check`. It writes a pass for the
//! worktree tree only when every rule of the spec holds. Otherwise it prints
//! each reason, logs them, and exits with 1.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

/// The variables that change what `just check` runs. Other `JUST_`
/// variables, such as `JUST_COLOR`, do not matter.
/// The list of the spec, and the variables of just 1.50 that choose another
/// Justfile or another environment: `JUST_JUSTFILE_NAME`, `JUST_CEILING`
/// and `JUST_NO_DOTENV`.
pub const FORBIDDEN: [&str; 9] = [
    "JUST_NO_DEPS",
    "JUST_SHELL",
    "JUST_JUSTFILE",
    "JUST_JUSTFILE_NAME",
    "JUST_CEILING",
    "JUST_WORKING_DIRECTORY",
    "JUST_DOTENV_FILENAME",
    "JUST_DOTENV_PATH",
    "JUST_NO_DOTENV",
];

/// The keys of `.agents/stack.json` that the pass reads.
#[derive(Deserialize)]
pub struct Stack {
    /// The steps of `just check`, in order.
    pub steps: Vec<String>,
    /// The steps that an installed profile provides. A skip of one of them
    /// fails the pass.
    #[serde(default)]
    pub provided_steps: Vec<String>,
}

pub fn read_stack(root: &Path) -> Result<Stack, String> {
    let path = root.join(".agents/stack.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("{} has no list `steps` of strings: {e}", path.display()))
}

/// True for the argv of `just check` exactly: no option, no `NAME=VALUE`.
pub fn is_just_check(argv: &[String]) -> bool {
    match argv {
        [just, check] => {
            Path::new(just).file_name().is_some_and(|n| n == "just") && check == "check"
        }
        _ => false,
    }
}

/// The reasons why the records of one run give no pass for its steps.
pub fn step_problems(run_records: &[Value], stack: &Stack) -> Vec<String> {
    let mut problems = Vec::new();
    for r in run_records {
        let kind = r["kind"].as_str().unwrap_or("");
        if kind == "line" && r["exit"] != 0 {
            problems.push(format!(
                "a line failed with exit {}: {}",
                r["exit"], r["line"]
            ));
        }
    }
    let has = |kind: &str, step: &str| {
        run_records
            .iter()
            .any(|r| r["kind"] == kind && r["step"] == step)
    };
    for step in &stack.steps {
        let provided = stack.provided_steps.contains(step);
        let skipped = has("skip", step);
        if skipped && provided {
            problems.push(format!(
                "the step `{step}` was skipped, but an installed profile provides it"
            ));
        } else if !has("step", step) && !skipped {
            problems.push(format!(
                "the step `{step}` did not finish: its recipe must end with `jlog step {step}`, or `jlog skip {step} REASON`"
            ));
        }
    }
    problems
}

/// The changes that `git status` shows outside the index: unstaged changes,
/// and untracked files that git does not ignore.
pub fn unstaged(root: &Path) -> Result<Vec<String>, String> {
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(parse_status(&out.stdout))
}

pub fn parse_status(raw: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let mut fields = raw.split(|&b| b == 0).filter(|f| !f.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let (x, y) = (entry[0], entry[1]);
        // A rename or a copy, staged or not, has the old path in the next field.
        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            fields.next();
        }
        if y != b' ' {
            found.push(String::from_utf8_lossy(&entry[3..]).into_owned());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stack(steps: &[&str], provided: &[&str]) -> Stack {
        Stack {
            steps: steps.iter().map(|s| s.to_string()).collect(),
            provided_steps: provided.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn only_just_check_itself_counts() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_just_check(&v(&["just", "check"])));
        assert!(is_just_check(&v(&["/opt/bin/just", "check"])));
        for bad in [
            &["just", "--no-deps", "check"][..],
            &["just", "v=x", "check"],
            &["just", "check", "x"],
            &["just"],
            &["justx", "check"],
            &["just", "lint"],
        ] {
            assert!(!is_just_check(&v(bad)), "{bad:?}");
        }
    }

    #[test]
    fn each_step_needs_its_marker_or_a_skip_that_no_profile_provides() {
        let s = stack(&["lint", "typecheck", "test"], &["lint", "test"]);
        let ok = [
            json!({"kind": "line", "exit": 0, "line": "ruff check ."}),
            json!({"kind": "step", "step": "lint"}),
            json!({"kind": "skip", "step": "typecheck"}),
            json!({"kind": "step", "step": "test"}),
        ];
        assert_eq!(step_problems(&ok, &s), Vec::<String>::new());

        let skipped_provided = [
            json!({"kind": "skip", "step": "lint"}),
            json!({"kind": "skip", "step": "typecheck"}),
            json!({"kind": "step", "step": "test"}),
        ];
        let p = step_problems(&skipped_provided, &s);
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("`lint` was skipped"));

        let missing = [
            json!({"kind": "step", "step": "lint"}),
            json!({"kind": "skip", "step": "typecheck"}),
        ];
        assert!(step_problems(&missing, &s)[0].contains("`test` did not finish"));
    }

    #[test]
    fn a_failed_line_fails_the_pass_and_a_failed_backtick_does_not() {
        let s = stack(&["lint"], &[]);
        let recs = [
            json!({"kind": "backtick", "exit": 1, "line": "git describe"}),
            json!({"kind": "line", "exit": 2, "line": "ruff check ."}),
            json!({"kind": "step", "step": "lint"}),
        ];
        let p = step_problems(&recs, &s);
        assert_eq!(p.len(), 1);
        assert!(p[0].starts_with("a line failed with exit 2"));
    }

    #[test]
    fn status_finds_unstaged_and_untracked_but_not_staged() {
        let raw = b"M  staged\0 M unstaged\0?? new\0R  to\0from\0RM moved\0old\0A  added\0 R new.txt\0Rfile.txt\0?? a b\0";
        assert_eq!(
            parse_status(raw),
            ["unstaged", "new", "moved", "new.txt", "a b"]
        );
    }
}
