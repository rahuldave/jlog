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
