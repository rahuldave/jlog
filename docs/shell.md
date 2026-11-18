---
title: The shell form
---

Set `jlog` as the shell of the Justfile, and put the real shell and its flags after it:

```just
set shell := ["jlog", "sh", "-cu"]

test:
  cargo test
```

`just` then calls `jlog sh -cu "cargo test"`. `jlog` runs the shell, waits for it, and logs the line with its exit code.

A repository that already sets a shell keeps it, wrapped in `jlog`. For example, `["bash", "-c"]` becomes `["jlog", "bash", "-c"]`.

## The shells that jlog knows

`jlog` knows the line for `sh`, `bash`, `zsh`, `dash`, `python3` and `python3.N`. These shells work, for example:

```just
set shell := ["jlog", "sh", "-cu"]
set shell := ["jlog", "bash", "-eu", "-o", "pipefail", "-c"]
set shell := ["jlog", "python3", "-c"]
```

Before the line, `jlog` accepts only flags that let the line run:

- **Short flags,** such as `-eu` or `+x`.
- **The options `-o NAME`,** and `-O NAME` in bash. NAME must be on a short list, for example `errexit`, `nounset`, `pipefail` or `extglob`. The option can also end a group of flags, as in `-euo pipefail`.
- **The options `-W VALUE` and `-X VALUE`** in python.

The last group of flags holds `c`, for example `-c`, `-cu` or `-euc`, and the line is the next argument.

## When the line is null

For any other form, the field `line` of the log is null. A null line can make a gate fail, but a wrong line could make one pass. So `jlog` gives null in each of these cases:

- **Options after the flag,** for example `sh -c -eu`.
- **A long option,** for example `--norc`.
- **A flag that stops the line,** for example `-n`, or `-o noexec`.
- **A line of the sh family that starts with `-` or `+`.**
- **Another shell,** for example `pwsh`, `cmd` or `node`, or a bare `python`, which can be Python 2.
- **A shell that did not start.**

## Use pipefail for lines with a pipe

In `sh` without `pipefail`, a pipe fails only when its last command fails. So with `sh -cu`, the line `cargo test | tee out.txt` succeeds when the tests fail. The run log then records exit 0, and a gate counts the tests as passed.

Some `sh`, such as dash on Debian and Ubuntu, has no `pipefail`. So use bash as the shell:

```just
set shell := ["jlog", "bash", "-euo", "pipefail", "-c"]
```

## Recipes that skip jlog

A recipe with a shebang line, or with the attribute `[script]`, does not use the shell of the Justfile. So its lines do not run through `jlog`, and the run log does not show them. The lint of the stack finds such recipes.

## Backticks

A backtick is a command in backticks in a Justfile. `just` runs it to get its output, and it also runs through `jlog`. The log marks it with `"kind": "backtick"`, and the gates ignore it.

`jlog` tells a backtick from a line by its stdout. `just` gives a line of a recipe its own stdout, but gives a backtick a new pipe. So when the stdout of `jlog` is a pipe that its parent does not share, the line is a backtick.

## Exit codes and signals

`jlog` exits with the exit code of the shell. If a signal stops the shell, `jlog` stops with the same signal, and the run log shows 128 plus the number of the signal. A signal that dumps core, for example SIGSEGV, is an exception. For such a signal, `jlog` exits with 128 plus the number, so that `jlog` itself does not crash.

While the shell runs, `jlog` does not stop for SIGINT or SIGQUIT. The terminal sends them to the shell too. `jlog` sends SIGTERM and SIGHUP on to the shell, and then waits to log the exit. `just` sends SIGTERM only to its child, which is `jlog`. So without this step, the shell would go on after `just` stopped.

## Speed

On the machine of the author, `jlog` adds about 3 to 4.5 ms to each line. About half of that is the start of one more process. Run `just bench` in the clone to measure it on your machine.
