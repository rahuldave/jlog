//! The time of each step of a run, from the records of the run log. A step
//! takes the time from the record before it in the run, the tree or the step
//! or skip before it, to its own record. `just` runs the dependencies of a
//! recipe one at a time, so that is the time of the step. Steps that run at
//! the same time, as with `[parallel]`, get wrong times: each one gets the
//! time since the record before it.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::time;

/// True for the records that start the time of the next step.
fn is_mark(r: &Value) -> bool {
    matches!(r["kind"].as_str(), Some("tree" | "step" | "skip"))
}

/// The field `at` of the last tree, step or skip of the records of one run.
pub fn last_mark(records: &[Value]) -> Option<f64> {
    records.iter().rev().find(|r| is_mark(r))?["at"].as_f64()
}

/// The field `at` of the last tree, step or skip of the run `run` in the run
/// log at `path`. It reads the log from its end, so a long log costs little.
pub fn last_mark_in_file(path: &Path, run: &str) -> Option<f64> {
    use std::io::{Read, Seek, SeekFrom};
    const BLOCK: u64 = 64 * 1024;
    let quoted = serde_json::to_string(run).ok()?;
    let mut f = std::fs::File::open(path).ok()?;
    let mut pos = f.metadata().ok()?.len();
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = BLOCK.min(pos);
        pos -= n;
        let mut buf = vec![0; n as usize];
        f.seek(SeekFrom::Start(pos)).ok()?;
        f.read_exact(&mut buf).ok()?;
        buf.extend_from_slice(&carry);
        let mut parts: Vec<&[u8]> = buf.split(|&b| b == b'\n').collect();
        // The first part can be the end of a line that starts in the block before.
        let head = if pos > 0 {
            parts.remove(0).to_vec()
        } else {
            Vec::new()
        };
        for line in parts.iter().rev() {
            let Ok(s) = std::str::from_utf8(line) else {
                continue;
            };
            if !s.contains(quoted.as_str()) {
                continue;
            }
            let Ok(r) = serde_json::from_str::<Value>(s) else {
                continue;
            };
            if r["run"] == run && is_mark(&r) {
                return r["at"].as_f64();
            }
        }
        if pos == 0 {
            return None;
        }
        carry = head;
    }
}

/// The first recipe of a `just` command: the first word after `just` that is
/// no option, no `NAME=VALUE` and no value of `-f`, `--justfile`, `-d` or
/// `--working-directory`. The words after it are its arguments, or more
/// recipes.
pub fn recipe(argv: &Value) -> Option<&str> {
    let mut words = argv.as_array()?.iter().skip(1).filter_map(Value::as_str);
    while let Some(w) = words.next() {
        if matches!(w, "-f" | "--justfile" | "-d" | "--working-directory") {
            words.next();
        } else if !w.starts_with('-') && !w.contains('=') {
            return Some(w);
        }
    }
    None
}

/// The run of the last check in the log, from records in the order of the
/// log. A run is a check when it has a step, a skip, a pass or a no-pass. A
/// run with only a failed line is a check when one of its lines comes from
/// `just check`, as when the first step fails. So a failed `just build` or
/// a `just times` does not hide the check before it.
pub fn last_run(all: &[Value]) -> Option<String> {
    let run_of = |r: &Value| r["run"].as_str().map(str::to_string);
    for r in all.iter().rev() {
        match r["kind"].as_str() {
            Some("step" | "skip" | "pass" | "no-pass") => return run_of(r),
            Some("line") if r["exit"].as_i64().is_some_and(|e| e != 0) => {
                let from_check = all.iter().any(|o| {
                    o["run"] == r["run"]
                        && o["kind"] == "line"
                        && recipe(&o["argv"]) == Some("check")
                });
                if from_check {
                    return run_of(r);
                }
            }
            _ => {}
        }
    }
    None
}

/// The time of one step or skip, as the pass and the no-pass log it.
#[derive(Debug, Serialize, PartialEq)]
pub struct Time {
    pub step: String,
    pub kind: String,
    pub seconds: Option<f64>,
}

/// The total time of the run, from its tree to `now`, and the time of each
/// step and skip, in the order of the log.
#[derive(Debug, Serialize, PartialEq)]
pub struct Summary {
    pub seconds: Option<f64>,
    pub times: Vec<Time>,
}

pub fn summary(records: &[Value], now: f64) -> Summary {
    let start = records
        .iter()
        .find(|r| r["kind"] == "tree")
        .and_then(|r| r["at"].as_f64());
    Summary {
        seconds: start.map(|s| time::between(s, now)),
        times: marks(records),
    }
}

/// The summary as one line, as `jlog pass` prints it at the end of `just
/// check`: `times: lint 4.1 s, test 191.3 s (skip); total 195.6 s`. The `;`
/// keeps the total apart from a step named `total`.
pub fn one_line(s: &Summary) -> String {
    let parts: Vec<String> = s
        .times
        .iter()
        .map(|t| {
            let skip = if t.kind == "skip" { " (skip)" } else { "" };
            format!("{} {}{skip}", t.step, secs(t.seconds))
        })
        .collect();
    let total = format!("total {}", secs(s.seconds));
    if parts.is_empty() {
        return format!("times: {total}");
    }
    format!("times: {}; {total}", parts.join(", "))
}

fn marks(records: &[Value]) -> Vec<Time> {
    records
        .iter()
        .filter(|r| matches!(r["kind"].as_str(), Some("step" | "skip")))
        .map(|r| Time {
            step: r["step"].as_str().unwrap_or("?").to_string(),
            kind: r["kind"].as_str().unwrap_or("?").to_string(),
            seconds: r["seconds"].as_f64(),
        })
        .collect()
}

/// What `jlog times` prints for one run.
#[derive(Debug, PartialEq)]
pub struct Table {
    /// The `just` command of the run, as text.
    pub argv: Option<String>,
    /// `passed`, `no pass`, `failed` or `not finished`.
    pub outcome: &'static str,
    /// The steps and skips, then the failed part, if any, with the kind `failed`.
    pub rows: Vec<Time>,
    /// The line that failed.
    pub failed_line: Option<String>,
    pub total: Option<f64>,
}

/// The name of the failed part. It is the recipe of the `just` command of the
/// failed line when that recipe is a step of the list `steps`, as for `just
/// integration`. Else it is `(failed after STEP)`, with the last step or
/// skip before the line, or `(failed)` when there is none.
fn failed_step(line: &Value, steps: &[String], after: Option<&str>) -> String {
    match recipe(&line["argv"]) {
        Some(r) if steps.iter().any(|s| s == r) => r.to_string(),
        _ => after.map_or_else(|| "(failed)".to_string(), |s| format!("(failed after {s})")),
    }
}

/// The table of one run. `steps` is the list of `.agents/stack.json`.
pub fn table(records: &[Value], steps: &[String]) -> Table {
    let mut rows = marks(records);
    let start = records
        .iter()
        .find(|r| r["kind"] == "tree")
        .and_then(|r| r["at"].as_f64());
    let end_of = |kind: &str| records.iter().rev().find(|r| r["kind"] == kind);
    let last = records.iter().rposition(is_mark);
    let failed = records
        .iter()
        .enumerate()
        .skip(last.map_or(0, |i| i + 1))
        .find(|(_, r)| r["kind"] == "line" && r["exit"].as_i64().is_some_and(|e| e != 0))
        .map(|(_, r)| r);
    let (outcome, end) = if let Some(p) = end_of("pass") {
        ("passed", p["at"].as_f64())
    } else if let Some(p) = end_of("no-pass") {
        ("no pass", p["at"].as_f64())
    } else if let Some(f) = failed {
        let after = records[..last.map_or(0, |i| i + 1)]
            .iter()
            .rev()
            .find(|r| matches!(r["kind"].as_str(), Some("step" | "skip")))
            .and_then(|r| r["step"].as_str());
        let name = failed_step(f, steps, after);
        let from = last.and_then(|i| records[i]["at"].as_f64());
        let to = f["at"].as_f64();
        rows.push(Time {
            step: name,
            kind: "failed".to_string(),
            seconds: from.zip(to).map(|(a, b)| time::between(a, b)),
        });
        ("failed", to)
    } else {
        (
            "not finished",
            records.iter().rev().find_map(|r| r["at"].as_f64()),
        )
    };
    let argv = records
        .iter()
        .filter_map(|r| r["argv"].as_array())
        .find(|a| !a.is_empty())
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        });
    Table {
        argv,
        outcome,
        rows,
        failed_line: failed
            .filter(|_| outcome == "failed")
            .map(|f| f["line"].as_str().unwrap_or("?").to_string()),
        total: start.zip(end).map(|(a, b)| time::between(a, b)),
    }
}

fn secs(s: Option<f64>) -> String {
    s.map_or_else(|| "-".to_string(), |s| format!("{s:.1} s"))
}

/// The table as text: a line for the run, a line for each step, and the total.
pub fn render(run: &str, table: &Table) -> String {
    let width = table
        .rows
        .iter()
        .map(|t| t.step.chars().count())
        .chain([5])
        .max()
        .unwrap_or(5);
    let mut out = match &table.argv {
        Some(argv) => format!("run {run} ({argv}): {}\n", table.outcome),
        None => format!("run {run}: {}\n", table.outcome),
    };
    for t in &table.rows {
        let note = match t.kind.as_str() {
            "skip" => "  skip",
            "failed" => "  failed",
            _ => "",
        };
        out += &format!("  {:<width$}  {:>10}{note}\n", t.step, secs(t.seconds));
    }
    out += &format!("  {:<width$}  {:>10}\n", "total", secs(table.total));
    if let Some(line) = &table.failed_line {
        let short: String = line.chars().take(200).collect();
        out += &format!("the line that failed: {short}\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn steps(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn run() -> Vec<Value> {
        vec![
            json!({"kind": "tree", "at": 100.0}),
            json!({"kind": "line", "exit": 0, "at": 101.0}),
            json!({"kind": "step", "step": "lint", "at": 104.5, "seconds": 4.5}),
            json!({"kind": "skip", "step": "typecheck", "at": 104.6, "seconds": 0.1}),
        ]
    }

    #[test]
    fn the_last_mark_is_the_last_tree_step_or_skip() {
        assert_eq!(last_mark(&run()), Some(104.6));
        assert_eq!(last_mark(&run()[..2]), Some(100.0));
        assert_eq!(last_mark(&[json!({"kind": "line", "at": 1.0})]), None);
        // A tree from an older jlog has no `at`, so the step gets no time.
        assert_eq!(last_mark(&[json!({"kind": "tree"})]), None);
    }

    #[test]
    fn the_last_run_is_the_last_run_of_a_check() {
        // After `just check`, a `just build`, a failed `just build` and a
        // `just times` make runs too. They are not checks.
        let all = vec![
            json!({"kind": "tree", "run": "check"}),
            json!({"kind": "step", "run": "check", "step": "lint"}),
            json!({"kind": "pass", "run": "check"}),
            json!({"kind": "tree", "run": "build"}),
            json!({"kind": "line", "run": "build", "exit": 0, "argv": ["just", "build"]}),
            json!({"kind": "line", "run": "bad-build", "exit": 2, "argv": ["just", "build"]}),
            json!({"kind": "line", "run": "typo", "exit": 1, "argv": ["just", "times", "nosuch"]}),
            json!({"kind": "tree", "run": "times"}),
        ];
        assert_eq!(last_run(&all).as_deref(), Some("check"));
        // A check that failed in its first step has only a failed line. One
        // of its lines comes from `just check`.
        let mut more = all.clone();
        more.push(json!({"kind": "line", "run": "first", "exit": 0, "argv": ["just", "check"]}));
        more.push(json!({"kind": "line", "run": "first", "exit": 2, "argv": ["just", "lint"]}));
        more.push(json!({"kind": "tree", "run": "later"}));
        assert_eq!(last_run(&more).as_deref(), Some("first"));
        assert_eq!(last_run(&all[3..]), None);
    }

    #[test]
    fn the_recipe_is_the_first_word_that_is_no_option() {
        let r = |a: Value| recipe(&a).map(str::to_string);
        assert_eq!(r(json!(["just", "check"])).as_deref(), Some("check"));
        assert_eq!(
            r(json!(["just", "test", "integration"])).as_deref(),
            Some("test")
        );
        assert_eq!(
            r(json!(["just", "--justfile", "lint", "-q", "v=1", "fmt"])).as_deref(),
            Some("fmt")
        );
        assert_eq!(
            r(json!(["just", "-d", "x", "python", "lint"])).as_deref(),
            Some("python")
        );
        assert_eq!(r(json!(["just", "--color=never"])), None);
        assert_eq!(r(json!([])), None);
        assert_eq!(r(json!(null)), None);
    }

    #[test]
    fn a_recipe_argument_is_not_the_failed_step() {
        // `just test integration`: `integration` is an argument of `test`.
        let r = vec![
            json!({"kind": "tree", "at": 0.0}),
            json!({"kind": "step", "step": "lint", "at": 1.0, "seconds": 1.0}),
            json!({"kind": "line", "exit": 1, "at": 3.0, "line": "false integration", "argv": ["just", "test", "integration"]}),
        ];
        let t = table(&r, &steps(&["lint", "test", "integration"]));
        assert_eq!(t.rows.last().unwrap().step, "test");
        // In `just check`, the failed part is named after the step before it.
        let mut r = r;
        r[2]["argv"] = json!(["just", "check"]);
        let t = table(&r, &steps(&["lint", "test", "integration"]));
        assert_eq!(t.rows.last().unwrap().step, "(failed after lint)");
    }

    #[test]
    fn the_last_mark_is_read_from_the_end_of_the_file() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("just.jsonl");
        let mut text = String::new();
        text += &json!({"kind": "tree", "run": "r", "at": 1.5}).to_string();
        text += "\n";
        // Lines of other runs that fill many blocks of 64 KiB.
        for i in 0..5000 {
            text += &json!({"kind": "line", "run": "other", "at": i, "line": "x".repeat(40)})
                .to_string();
            text += "\n";
        }
        text += &json!({"kind": "step", "run": "other", "at": 9.0}).to_string();
        text += "\n";
        std::fs::write(&path, &text).unwrap();
        assert_eq!(last_mark_in_file(&path, "r"), Some(1.5));
        assert_eq!(last_mark_in_file(&path, "other"), Some(9.0));
        assert_eq!(last_mark_in_file(&path, "none"), None);
        // A run whose name is inside another value does not count.
        assert_eq!(last_mark_in_file(&path, "x"), None);
        assert_eq!(last_mark_in_file(&d.path().join("missing"), "r"), None);
    }

    #[test]
    fn the_summary_has_the_total_and_each_step() {
        let s = summary(&run(), 110.25);
        assert_eq!(s.seconds, Some(10.25));
        assert_eq!(s.times.len(), 2);
        assert_eq!(
            s.times[0],
            Time {
                step: "lint".into(),
                kind: "step".into(),
                seconds: Some(4.5)
            }
        );
        assert_eq!(s.times[1].kind, "skip");
    }

    #[test]
    fn the_summary_fits_on_one_line() {
        assert_eq!(
            one_line(&summary(&run(), 110.25)),
            "times: lint 4.5 s, typecheck 0.1 s (skip); total 10.2 s"
        );
        let old = summary(
            &[
                json!({"kind": "tree"}),
                json!({"kind": "step", "step": "lint"}),
            ],
            5.0,
        );
        assert_eq!(one_line(&old), "times: lint -; total -");
        assert_eq!(one_line(&summary(&[], 1.0)), "times: total -");
    }

    #[test]
    fn a_failed_line_after_the_last_mark_is_the_failed_step() {
        let mut r = run();
        r.push(json!({"kind": "line", "exit": 0, "at": 200.0}));
        r.push(json!({"kind": "line", "exit": 101, "at": 599.6, "line": "cargo test --test it", "argv": ["just", "integration"]}));
        let t = table(&r, &steps(&["lint", "typecheck", "integration", "docs"]));
        assert_eq!(t.outcome, "failed");
        assert_eq!(t.total, Some(499.6));
        let last = t.rows.last().unwrap();
        assert_eq!(
            (last.step.as_str(), last.kind.as_str(), last.seconds),
            ("integration", "failed", Some(495.0))
        );
        assert_eq!(t.failed_line.as_deref(), Some("cargo test --test it"));
        let text = render("r1", &t);
        assert!(
            text.starts_with("run r1 (just integration): failed\n"),
            "{text}"
        );
        assert!(
            text.ends_with("the line that failed: cargo test --test it\n"),
            "{text}"
        );
        assert!(text.contains("integration     495.0 s  failed"), "{text}");
        assert!(text.contains("typecheck         0.1 s  skip"), "{text}");
    }

    #[test]
    fn a_failed_line_names_no_step_that_did_not_run() {
        // `check: setup lint`, where `setup` is not a step and fails: the
        // command is `just check`, so no step is named.
        let r = vec![
            json!({"kind": "tree", "at": 0.0}),
            json!({"kind": "line", "exit": 1, "at": 2.0, "line": "false", "argv": ["just", "check"]}),
        ];
        let t = table(&r, &steps(&["lint", "test"]));
        let last = t.rows.last().unwrap();
        assert_eq!((last.step.as_str(), last.seconds), ("(failed)", Some(2.0)));
        assert!(render("r", &t).starts_with("run r (just check): failed\n"));
    }

    #[test]
    fn a_failed_line_before_a_later_step_is_not_the_end() {
        // A line can fail without ending the run, as with `-` in front of it.
        let mut r = vec![
            json!({"kind": "tree", "at": 0.0}),
            json!({"kind": "line", "exit": 1, "at": 1.0}),
        ];
        r.push(json!({"kind": "step", "step": "lint", "at": 2.0, "seconds": 2.0}));
        let t = table(&r, &steps(&["lint"]));
        assert_eq!(t.outcome, "not finished");
        assert_eq!(t.rows.len(), 1);
    }

    #[test]
    fn a_pass_or_a_no_pass_ends_the_run() {
        let mut r = run();
        r.push(json!({"kind": "pass", "at": 105.0}));
        let t = table(&r, &[]);
        assert_eq!((t.outcome, t.total), ("passed", Some(5.0)));
        let mut r = run();
        r.push(json!({"kind": "no-pass", "at": 106.0}));
        assert_eq!(table(&r, &[]).outcome, "no pass");
    }

    #[test]
    fn records_of_an_older_jlog_show_no_time() {
        let r = vec![
            json!({"kind": "tree"}),
            json!({"kind": "step", "step": "lint"}),
        ];
        let t = table(&r, &[]);
        assert_eq!(t.total, None);
        let text = render("old", &t);
        assert!(text.contains("  lint            -\n"), "{text}");
    }
}
