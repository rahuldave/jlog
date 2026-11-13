# jlog

`jlog` keeps a run log of the recipes of `just`. The commit gates of the stack read the run log, to see which steps ran on which tree. The design of the gates is in [docs/spec.md](docs/spec.md).

`just` has no log of its own. So a Justfile sets `jlog` as its shell, and `jlog` runs each line with the real shell. Then it adds one JSON line to the run log, `.agents/log/just.jsonl`.

The shell form, the commands `session`, `env`, `tools`, `skip`, `step` and `pass`, and the plugins for Claude Code and Codex work now.

## Terms

| Term | Meaning |
|---|---|
| Run log | The file `.agents/log/just.jsonl`, with one JSON line for each line that ran. |
| Log folder | The folder `.agents/log/`. Git ignores it. |
| Run | One `just check`, or one other call of `just`, with the inner `just` calls that it makes. All its lines have the same run ID. |
| Step | One recipe of `just check`, for example `lint` or `test`. The list is in `.agents/stack.json`. |
| Worktree tree | The git tree of all files in the worktree, staged or not, without the files that git ignores. |
| Pass | A record that says that every step of `just check` passed for one worktree tree. |
| Backtick | A command in backticks in a Justfile. `just` runs it to get its output. |
| Harness | The program that runs an agent, for example Claude Code or Codex. |
| Session | One session of a harness, or one terminal session of a person. |
| Fresh agent | An agent that a harness starts for one task, inside the session of its parent. |
| Plugin | A small Python program for one harness. It finds the transcript and the model of the session. |

## Install

`jlog` needs Rust and `just`. The tests also need git.

1. Clone the repository.
2. Run `just install` in the clone.

The recipe `install` builds `jlog` and installs it with `cargo install`, so the program goes to `~/.cargo/bin`. When the repository has the folder `plugins/`, the recipe also copies it to `~/.local/share/jlog/plugins/`.

## Use

Set `jlog` as the shell of the Justfile. Put the real shell and its flags after it:

```just
set shell := ["jlog", "sh", "-cu"]

test:
  cargo test
```

`jlog` knows the line for `sh`, `bash`, `zsh`, `dash` and `python`. Before the line, it accepts only short flags that let the line run, such as `-eu` or `+x`, and the options `-o NAME`, `-O NAME` in bash, and `-W VALUE` or `-X VALUE` in python. Each NAME must be on a short list that lets the line run, for example `errexit`, `nounset`, `pipefail` or `extglob`, so `-o noexec` gives a null line. The option can also end a group of flags, as in `-euo pipefail`. The last group holds `c`, for example `-c`, `-cu` or `-euc`, and the line is the next argument. So these shells work, for example:

```just
set shell := ["jlog", "sh", "-cu"]
set shell := ["jlog", "bash", "-eu", "-o", "pipefail", "-c"]
set shell := ["jlog", "python3", "-c"]
```

For any other form, the field `line` is null. A null line can make a gate fail, but a wrong line could make one pass. So `jlog` gives null in each of these cases:

- **Options after the flag,** for example `sh -c -eu`.
- **A long option,** for example `--norc`.
- **A flag that stops the line from running,** for example `-n`.
- **A line of the sh family that starts with `-` or `+`.**
- **Another shell,** for example `pwsh`, `cmd` or `node`, or a bare `python`, which can be Python 2.
- **A shell that did not start.**

`jlog` reads the argv of its parent only when the program of the parent is named `just`. A copy of `just` with another name, for example `just-1.50`, gives an empty argv.

A repository that already sets a shell keeps it, wrapped in `jlog`. For example, `["bash", "-c"]` becomes `["jlog", "bash", "-c"]`.

Each line of a recipe then adds a line like this one to the run log:

```json
{"run":"48213-1794498589","argv":["just","test"],"line":"cargo test","exit":0,"start":"2026-11-12T10:00:00Z","end":"2026-11-12T10:00:04Z","kind":"line","harness":"terminal","session":"301-1794490000","child":false}
```

| Field | Meaning |
|---|---|
| `run` | The value of `JLOG_RUN` when it is set. Otherwise the process ID of `just` and its start time, in seconds since 1970. If the parent of `jlog` is not `just`, the parent counts in place of `just`. |
| `argv` | The `just` command as a list, with each argument whole. It is empty when the parent of `jlog` is not `just`. |
| `line` | The line of the recipe, before the shell expands it. It is null when `jlog` cannot tell which argument is the line. |
| `exit` | The exit code of the shell. A shell that a signal stopped gets 128 plus the number of the signal. |
| `start`, `end` | The times in UTC, to the second. |
| `kind` | `line` for a line of a recipe, and `backtick` for a backtick. The gates ignore backticks. |
| `harness` | `codex`, `claude` or `terminal`. |
| `session` | The ID of the session. The section "Sessions" says where it comes from. |
| `child` | True for a line of a fresh agent. It is null in Codex, where the record `session` names the parent. |

A recipe with a shebang line, or with the attribute `[script]`, does not use the shell of the Justfile. So its lines do not run through `jlog`, and the run log does not show them. The lint of the stack finds such recipes.

## Runs

The root Justfile exports one run ID, so that the inner `just` calls of one `just check` share one run:

```just
export JLOG_RUN := env("JLOG_RUN", uuid())
```

The first `just` call makes the ID, and each inner call inherits it. A module must not have the line, because it would make a second ID. Without `JLOG_RUN`, the run is the process ID of the parent `just` and its start time.

Before the first line of a run starts, `jlog` computes the worktree tree and logs it in a record `tree`. The tree is null when git cannot compute it. `jlog` marks the run as seen with an empty file, `.agents/log/runs/run-<id>`, made only if the file is new.

The first line locks the marker until the record `tree` is in the log, so a parallel line of the run waits for it. The lock goes before the first line runs, because that line can be an inner `just` call. A backtick takes no part in this, because the backticks of the outer `just` do not get the exported `JLOG_RUN`.

A person can set `JLOG_RUN` by hand, so `jlog pass` compares the first tree with the tree at the end. So an old ID cannot pass a new tree.

`jlog` computes the tree with a temporary index, so the real index does not change:

```sh
cp <the real index> <tmp>
GIT_INDEX_FILE=<tmp> git add -A -- :/
GIT_INDEX_FILE=<tmp> git write-tree
```

The copy of the real index keeps its record of each file, so git hashes only the files that changed. With a split index, each copy leaves a `sharedindex` file in `.git`, which git removes after two weeks. Each tree also writes the files that git does not ignore into the objects of git, as `git write-tree` does. A tracked file that git ignores stays in the tree, as in a commit. When an entry of the real index is marked assume-unchanged or skip-worktree, git would not look at that file. Then `jlog` starts the temporary index with `git read-tree HEAD` instead, as the spec says.

## Sessions

`jlog` finds the session in the environment, in this order:

| Harness | Where the ID comes from |
|---|---|
| Codex | `CODEX_THREAD_ID`. |
| Claude Code | `CLAUDE_CODE_SESSION_ID`, or else `JLOG_SESSION`, which a hook can set. `child` is true when `CLAUDE_CODE_CHILD_SESSION` is set and is not `0`. |
| A terminal | `getsid(0)`, the process ID of the leader of the terminal session, with its start time. So all `just` calls in one terminal share one session. When `getsid` gives 0, as in some containers, the parent of `jlog` counts in place of the leader. |

The first line of a new session also logs the records `session`, `env` and `tools`. `jlog` marks the session as seen with an empty file, `.agents/log/sessions/<harness>-<id>`. It makes the file only if the file is new, so two lines that start at once log the session only once. An ID with other characters than letters, digits, `.`, `_` and `-` gets a hash in place of itself in the name of the file.

This work makes the first line of a session slower, by about 0.4 to 0.9 s on the machine of the author. Most of the time goes to the runs of `--version`. A failure in this work never fails the line. A signal to `jlog` during this work stops it, and `jlog` then ends with that signal.

## Commands

Each command prints one JSON record, and adds it to the run log when it finds a log folder. Each record has `kind`, `run`, `harness`, `session`, `child` and `time`.

A command takes the run from `JLOG_RUN`, or else from the nearest `just` above it, or null when there is none. A command with no log folder still prints and exits with 0. A failed write exits with 1, and a wrong argument exits with 2.

| Command | What it logs |
|---|---|
| `jlog session` | The fields `transcript`, `model` and `parent`, from the plugin of the harness. A terminal has no plugin. |
| `jlog env` | The name of each variable, and its value only for the safe list of the section "Secrets". Each other value shows as `redacted`. |
| `jlog tools` | The version of each program of the tools table of the spec, with `--version`. |
| `jlog skip <step> <reason>` | A step with nothing to do, and the reason. A step is a name of letters, digits, `-` and `_`. |
| `jlog step <step>` | The end of a step that passed. It is the last line of the recipe of each step. |
| `jlog pass` | A pass for the worktree tree, when each rule of the section "The pass" holds. It is the last line of `check`. |

With no argument, `session`, `env` and `tools` are commands. With arguments, they are the shell form, so `jlog env sh -c LINE` still runs a shell. The names `skip`, `step` and `pass` are always commands. So a typo such as `jlog tools x` runs a program named `tools`, and the run log shows a failed line with exit 127.

**`jlog env`** also tells, for each variable, whether its value changed since the last snapshot. It shows `changed` as true or false, and as null when it has nothing to compare with. The list `gone` names the variables that the last snapshot had and this one has not.

`jlog` compares keyed hashes, HMAC-SHA256 of the name and the value. The key is a random file, `~/.local/share/jlog/key`, that only its owner can read. `jlog` uses it only when `HOME` is an absolute path to a folder that the user owns. So a relative `HOME` cannot put the key inside a repository, and `sudo` cannot make the folder for root. The hashes go into `.agents/log/env-hashes.json`, not into the run log. Without the key, a reader of that file cannot test a guess of a value.

**`jlog tools`** runs all programs at once, with a limit of 3 seconds. It sets `UV_PYTHON_DOWNLOADS=never`, so no download starts. A missing program shows `"error": "not found"`. The version is the first line of the output that holds a digit.

**`jlog skip`** removes secrets from the step and the reason, as for `line`. It also logs the argv of the nearest `just`.

## The pass

The recipes of the steps end with `jlog step`, and `check` ends with `jlog pass`:

```just
lint:
    just --justfile "{{justfile()}}" python lint
    jlog step lint

typecheck:
    jlog skip typecheck "no type checker in this repository"

check: lint typecheck
    jlog pass
```

`just` stops a recipe at its first failed line. So `jlog step lint` runs only when each line of `lint` before it passed.

`jlog step`, `jlog skip` and `jlog pass` count only as a line of their own. A `jlog` whose parent is `just` must run them, and its line must start with the command, for example `jlog step lint`. Between them, only the shell that this `jlog` started may stand. So each of these fails:

- **A step in a script,** such as a shebang recipe, a `[script]` recipe, or a script that a line runs. A script goes on after a failed line.
- **A step that a program runs,** such as a test suite.
- **A step after another command,** such as `false || jlog step lint`.

The line must start with the bare word `jlog`, or with a path to `jlog` that has no space. So `exec jlog step lint` and `VAR=1 jlog step lint` fail too.

A failed command in a pipe does not fail the line in `sh`, so `cargo test | tee log` passes when the tests fail. Use a shell with `pipefail` for such lines, for example `["jlog", "bash", "-eu", "-o", "pipefail", "-c"]`. `jlog pass` writes the record `pass` with the tree only if all of these hold:

- **The call.** The nearest `just` above `jlog pass` has the argv `just check` exactly, with no option and no `NAME=VALUE`.
- **No variable that changes the run.** None of `JUST_NO_DEPS`, `JUST_SHELL`, `JUST_JUSTFILE`, `JUST_JUSTFILE_NAME`, `JUST_CEILING`, `JUST_WORKING_DIRECTORY`, `JUST_DOTENV_FILENAME`, `JUST_DOTENV_PATH` and `JUST_NO_DOTENV` is set. Other `JUST_` variables, such as `JUST_COLOR`, do not matter.
- **Each step.** For each step in the list `steps` of `.agents/stack.json`, the run has a record `step` for it. A skip also counts, but only for a step that is not in the list `provided_steps`, the steps that an installed profile provides.
- **No failed line.** No line of the run has an exit code other than 0. A failed backtick does not count.
- **The same tree.** The worktree tree now equals the tree of the record `tree` of the run.
- **Nothing unstaged.** The worktree has no unstaged change, and no untracked file that git does not ignore.

The lines of an inner `just` call count, because they share the run. Otherwise, `jlog pass` prints each reason, logs the record `no-pass` with the list `reasons`, and exits with 1. The record `.agents/stack.json` has these two keys for the pass:

```json
{"steps": ["fmt-check", "lint", "typecheck", "test", "diff-check"], "provided_steps": ["fmt-check", "lint", "test"]}
```

## Plugins

The folder `plugins/` holds one plugin for each harness, `claude` and `codex`. Each one is a uv project of its own, with `pyproject.toml`, `uv.lock` and `session.py`, and it needs no package. `jlog` runs a plugin like this, with a limit of 5 seconds:

```sh
uv run --project <plugins>/<harness> --frozen --no-cache --quiet python <plugins>/<harness>/session.py
```

The option `--no-cache` lets the plugin run in a sandbox whose home is read-only. `just install` makes the environment of each plugin, so a run needs no write.

The folder of the plugins is `JLOG_PLUGINS`, or else `~/.local/share/jlog/plugins`. Each must be an absolute path. Before the run, `jlog` removes each variable that starts with `UV_` or `PYTHON`, and `VIRTUAL_ENV`. So a setting of the repository, such as `UV_PROJECT_ENVIRONMENT=.venv`, cannot move the environment of the plugin into the repository.

A plugin reads only JSONL files, and it writes nothing in the folders of a harness:

- **The plugin for Claude Code** looks for `projects/*/<session>.jsonl` in `CLAUDE_CONFIG_DIR`, or else in `~/.claude`. The model is the model of the last answer.
- **The plugin for Codex** looks for `sessions/*/*/*/rollout-*-<thread>.jsonl` in `CODEX_HOME`, or else in `~/.codex`. The model comes from the last `turn_context` line. The parent comes from the `session_meta` line of a fresh agent. The plugin never opens the SQLite files of Codex.

Each plugin refuses an ID that is not a plain name, so an ID cannot name another folder. It reads at most the last 8 MB of a transcript. `jlog` keeps only the fields `transcript`, `model`, `parent` and `error` of the output.

## Exit codes and signals

`jlog` exits with the exit code of the shell. If a signal stops the shell, `jlog` stops with the same signal, and the run log shows 128 plus the number of the signal. A signal that dumps core, for example SIGSEGV, is an exception. For such a signal, `jlog` exits with 128 plus the number, so that `jlog` itself does not crash.

While the shell runs, `jlog` does not stop for SIGINT or SIGQUIT, as `system(3)` does, because the terminal sends them to the shell too. `jlog` sends SIGTERM and SIGHUP on to the shell, and then waits to log the exit. `just` sends SIGTERM to its child only, which is `jlog`, so without this step the shell would go on after `just` stopped.

## How jlog finds the log folder

`jlog` goes up from the current folder to the first folder that holds `.agents/`. It does not call git, so each line costs little time. The search stops in two places:

- **At the top of a repository.** A folder that holds `.git` ends the search, so a repository without `.agents/` gets no log.
- **At a home folder.** `jlog` never logs into `~/.agents`, which holds the global skills. A home folder is the folder in `HOME`, the home folder of root, or a folder right under `/Users` or `/home`. The check works when `HOME` is unset or wrong.

`jlog` also refuses an `.agents` folder whose real path is inside the `.agents` of a home folder, for example in a project under `~/.agents/skills`. It checks the folder that it opens, so a symlink put in place after the search changes nothing. `jlog` makes `.agents/log` and the run log itself. It opens the folder once, with no symlink followed, and makes both files inside that open folder. So a symlink in place of the folder or of a file gets no write. The run log must be a regular file with one link.

If `jlog` finds no folder, it runs the line and logs nothing. On its first write, it makes `.agents/log/.gitignore` with the one line `*`. So `git add -A`, `git stash -u` and `git clean -fd` leave the log folder alone.

## Secrets

A line can hold a secret, for example a value from a `.env` file. Before `jlog` logs `line` and `argv`, it removes the value of each variable of its environment that has 8 or more characters. It trims spaces and newlines from each value first. It writes `<redacted>` in place of the value. These variables are safe, and `jlog` keeps their values:

```text
PATH SHELL HOME PWD OLDPWD TERM CI CODEX_THREAD_ID CLAUDE_CODE_SESSION_ID CLAUDE_CODE_CHILD_SESSION
```

`jlog` trusts these names whatever set them. So do not put a secret in one of them, for example with `PWD=...` in a `.env` file.

Other values with 8 or more characters are redacted too. For example, `LANG` can hold `en_US.UTF-8`, so a line that holds the text `en_US.UTF-8` logs `<redacted>` in its place.

An argument of a recipe is in the environment only with `set export` or `set dotenv-load`. If the argument is not in the environment, `jlog` cannot know that it is a secret.

## How jlog tells a backtick from a line

`just` gives a line of a recipe its own stdout. It gives a backtick a new pipe, and it reads the output from that pipe. So `jlog` compares its stdout with the stdout of its parent. If its stdout is a pipe that the parent does not share, the line is a backtick.

## Speed

`scripts/bench.sh` runs a recipe of many lines, first with `sh -cu` and then with `jlog sh -cu`. The rounds alternate, so that a change in the load of the machine hits both forms alike. Run it with `just bench`.

On 2026-11-12, on the machine of the author, `jlog` added 4.1 to 4.5 ms to each line. A small C program that only starts the shell and waits for it added 1.9 to 2.3 ms. So the start of one more process takes about half of the time. The machine was busy during the test, with a load average of 5 to 16, and an earlier run at a load of 6 to 9 gave 2.7 to 3.6 ms.

The work of `jlog` itself takes about 1.8 to 2.2 ms of that time. It finds the folder, reads the argv, finds the session and the run, redacts and writes. The marker of the run and its lock add no CPU time that a test of 1,000 lines could measure. The bench runs as a terminal session with a home folder of its own, and its best round leaves out the first line of the session.

The first line of each run also computes the worktree tree. That takes one `git add -A` on a copy of the index, so its cost grows with the changed files, not with the whole repository.

## Development

Run each task with `just`:

| Recipe | What it does |
|---|---|
| `just build` | Builds the debug and the release binaries. |
| `just test` | Runs the unit tests and the labs. A lab runs a real `just` in a temporary git repository. |
| `just lint` | Runs clippy, with warnings as errors. |
| `just check` | Runs `fmt-check`, `lint`, `test`, `diff-check`, `prose-check`, and the checks of both plugins. |
| `just claude::check`, `just codex::check` | Checks the lock file of a plugin, and runs its tests on fake transcripts. |
| `just bench` | Measures the time that `jlog` adds to each line. |
| `just install` | Installs `jlog`, and runs `install-plugins`. |
| `just install-plugins` | Copies `pyproject.toml`, `uv.lock` and `session.py` of each plugin to `~/.local/share/jlog/plugins/`, and makes the environment of each plugin. |

The labs need `just`, git, uv and `script(1)`, and the bench needs Perl. The tests remove the variables of each harness, so a test run inside Claude Code or Codex reads no real transcript. `jlog` runs on macOS and Linux. The CI workflow in `.github/workflows/ci.yml` runs `just check` on both, for each push to `main` and each PR.

## License

MIT.
