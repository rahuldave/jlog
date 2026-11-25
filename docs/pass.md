---
title: The pass
---

A pass is a record that says that every step of `just check` passed for one worktree tree. The commit gates look for a pass for the tree that they commit.

## step, skip and pass

The recipe of each step ends with `jlog step`, and `check` ends with `jlog pass`:

```just
lint:
    just --justfile "{{justfile()}}" python lint
    jlog step lint

typecheck:
    jlog skip typecheck "no type checker in this repository"

check: lint typecheck
    jlog pass
```

| Command | What it logs |
|---|---|
| `jlog step <step>` | The end of a step that passed. |
| `jlog skip <step> <reason>` | A step with nothing to do, and the reason. |
| `jlog pass` | A pass for the worktree tree, when each rule below holds. |
| `jlog times [RUN]` | Nothing. It prints the time of each step of a run. |

`just` stops a recipe at its first failed line. So `jlog step lint` runs only when each line of `lint` before it passed.

A step is a name of letters, digits, `-` and `_`. The list of the steps is in `.agents/stack.json`:

```json
{"steps": ["fmt-check", "lint", "typecheck", "test", "diff-check"], "provided_steps": ["fmt-check", "lint", "test"]}
```

The list `provided_steps` names the steps that an installed profile gives.

## Each command must be a line of its own

`jlog step`, `jlog skip` and `jlog pass` count only as a line of their own, which `just` runs. The line must start with the command. So each of these fails:

- **A step in a script,** such as a shebang recipe, a `[script]` recipe, or a script that a line runs. A script goes on after a failed line.
- **A step that a program runs,** such as a test suite.
- **A step after another command,** such as `false || jlog step lint`.
- **A step with a prefix,** such as `exec jlog step lint` or `VAR=1 jlog step lint`.

## The rules of the pass

`jlog pass` writes the record `pass` with the tree only if all of these hold:

- **The call.** The nearest `just` above `jlog pass` has the argv `just check` exactly, with no option and no `NAME=VALUE`.
- **No variable that changes the run.** None of `JUST_NO_DEPS`, `JUST_SHELL`, `JUST_JUSTFILE`, `JUST_JUSTFILE_NAME`, `JUST_CEILING`, `JUST_WORKING_DIRECTORY`, `JUST_DOTENV_FILENAME`, `JUST_DOTENV_PATH` and `JUST_NO_DOTENV` is set.
- **Each step.** For each step in the list `steps`, the run has a record `step` for it. A skip also counts, but only for a step that is not in the list `provided_steps`.
- **No failed line.** No line of the run has an exit code other than 0. A failed backtick does not count.
- **The same tree.** The worktree tree now equals the tree of the record `tree` of the run.
- **Nothing unstaged.** The worktree has no unstaged change, and no untracked file that git does not ignore.

The lines of an inner `just` call count, because they share the run. A person can set `JLOG_RUN` by hand. The rule of the same tree means that an old run ID cannot pass a new tree.

When a rule fails, `jlog pass` prints each reason, logs the record `no-pass` with the list `reasons`, and exits with 1.

A failed command in a pipe does not fail the line in `sh`. So use a shell with `pipefail`, as the page [Shell](shell.md) says.

## The time of each step

A record `step` or `skip` also logs how long the step took:

- **`start`.** The time of the record before it in the run. That record is the tree of the run, or the step or skip before it.
- **`seconds`.** The time from that record to this one, to the millisecond.

`just` runs the dependencies of `check` one at a time, so that is the time of the step. A recipe that is not a step adds its time to the next step. Two cases give wrong times:

- **Steps that run at the same time,** as with `[parallel]`. Each step gets the time since the record before it, so a step that ends second gets only the gap after the first one.
- **A `JLOG_RUN` set by hand to an old run.** The first step then gets the time since the last record of that old run.

The records `pass` and `no-pass` also log `seconds`, the time from the tree of the run to the record. They also log `times`, a list with the step, the kind and the seconds of each step and skip.

`jlog pass` also prints these times on one line to stderr, as its last line. So the output of `just check` already shows them:

```text
jlog: times: lint 4.1 s, typecheck 0.0 s (skip), test 191.3 s, integration 495.0 s; total 690.4 s
```

A `;` comes before the total, so a step named `total` stays apart. A time that the log does not have shows as `-`, as for a run that an older `jlog` began. The line comes for a no-pass too, after its reasons. A step that fails stops `just check` before `jlog pass`, so for a failed step, run `jlog times`.

`jlog times` prints this list for the last run of a check, and logs nothing. The last run of a check is the last run with a step, a skip, a pass or a no-pass, or with a failed line and a line of `just check`. So a `just build` or a `just times` after `just check` does not hide the check, even when it fails. `jlog times RUN` prints the list for the run `RUN`:

```text
run 3f2c9a7e-1b9d-4c56-a0f1-2d8e5b7c9a10 (just check): failed
  lint                      4.1 s
  typecheck                 0.0 s  skip
  test                    191.3 s
  (failed after test)     495.0 s  failed
  total                   690.4 s
the line that failed: cargo test --test integration
```

The first line gives the `just` command of the run and its outcome, which is `passed`, `no pass`, `failed` or `not finished`. A run is failed when a line failed after the last step. The time from the last step to the failed line is then the failed part. It is named for the step when the failed line ran in an inner call such as `just integration`. Otherwise, as when `check` lists the steps as its dependencies, it is named `(failed after lint)`, after the step before it, or `(failed)` when there is none. The line that failed shows which step it was.

A line with `-` in front of it can fail without ending the recipe. `jlog` cannot tell, so such a run also shows as failed. A run that an older `jlog` logged has no times, so `jlog times` prints `-`.
