# jlog

`jlog` keeps a run log of the recipes of `just`. The commit gates of the stack read the run log, to see which steps ran on which tree. The design of the gates is in [docs/spec.md](docs/spec.md).

`just` has no log of its own. So a Justfile sets `jlog` as its shell, and `jlog` runs each line with the real shell. Then it adds one JSON line to the run log, `.agents/log/just.jsonl`.

The shell form works now. The commands `skip`, `pass`, `env`, `tools` and `session`, and the plugins for each harness, come next.

## Terms

| Term | Meaning |
|---|---|
| Run log | The file `.agents/log/just.jsonl`, with one JSON line for each line that ran. |
| Log folder | The folder `.agents/log/`. Git ignores it. |
| Run | One call of `just`. All lines of one call have the same run ID. |
| Backtick | A command in backticks in a Justfile. `just` runs it to get its output. |

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

`jlog` knows the line for `sh`, `bash`, `zsh`, `dash` and `python`. Before the line, it accepts only short flags that let the line run, such as `-eu` or `+x`, and the options `-o NAME`, `-O NAME` in bash, and `-W VALUE` or `-X VALUE` in python. Each NAME must be on a short list that lets the line run, for example `errexit`, `nounset`, `pipefail` or `extglob`, so `-o noexec` gives a null line. The last group holds `c`, for example `-c`, `-cu` or `-euc`, and the line is the next argument. So these shells work, for example:

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
{"run":"48213-1794498589","argv":["just","test"],"line":"cargo test","exit":0,"start":"2026-11-12T10:00:00Z","end":"2026-11-12T10:00:04Z","kind":"line"}
```

| Field | Meaning |
|---|---|
| `run` | The process ID of `just` and its start time, in seconds since 1970. If the parent of `jlog` is not `just`, the parent counts in place of `just`. |
| `argv` | The `just` command as a list, with each argument whole. It is empty when the parent of `jlog` is not `just`. |
| `line` | The line of the recipe, before the shell expands it. It is null when `jlog` cannot tell which argument is the line. |
| `exit` | The exit code of the shell. A shell that a signal stopped gets 128 plus the number of the signal. |
| `start`, `end` | The times in UTC, to the second. |
| `kind` | `line` for a line of a recipe, and `backtick` for a backtick. The gates ignore backticks. |

A recipe with a shebang line, or with the attribute `[script]`, does not use the shell of the Justfile. So its lines do not run through `jlog`, and the run log does not show them. The lint of the stack finds such recipes.

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
PATH SHELL HOME TERM CI CODEX_THREAD_ID CLAUDE_CODE_SESSION_ID CLAUDE_CODE_CHILD_SESSION
```

Other values with 8 or more characters are redacted too. For example, `PWD` holds the current folder, and `LANG` can hold `en_US.UTF-8`. So a line that names the current folder, or the text `en_US.UTF-8`, logs `<redacted>` in its place.

An argument of a recipe is in the environment only with `set export` or `set dotenv-load`. If the argument is not in the environment, `jlog` cannot know that it is a secret.

## How jlog tells a backtick from a line

`just` gives a line of a recipe its own stdout. It gives a backtick a new pipe, and it reads the output from that pipe. So `jlog` compares its stdout with the stdout of its parent. If its stdout is a pipe that the parent does not share, the line is a backtick.

## Speed

`scripts/bench.sh` runs a recipe of many lines, first with `sh -cu` and then with `jlog sh -cu`. The rounds alternate, so that a change in the load of the machine hits both forms alike. Run it with `just bench`.

On 2026-11-12, on the machine of the author, `jlog` added 2.4 to 2.9 ms to each line. A small C program that only starts the shell and waits for it added 1.6 to 2.1 ms. So the start of one more process takes most of the time. The work of `jlog` itself takes about 0.7 to 1.3 ms. It finds the folder, reads the argv, redacts and writes. The machine was busy during the test, with a load average of 6 to 18.

## Development

Run each task with `just`:

| Recipe | What it does |
|---|---|
| `just build` | Builds the debug and the release binaries. |
| `just test` | Runs the unit tests and the labs. A lab runs a real `just` in a temporary git repository. |
| `just lint` | Runs clippy, with warnings as errors. |
| `just check` | Runs `fmt-check`, `lint`, `test`, `diff-check` and `prose-check`. |
| `just bench` | Measures the time that `jlog` adds to each line. |
| `just install` | Installs `jlog`, and copies the plugins to `~/.local/share/jlog/plugins/`. |

The labs need `just`, git and `script(1)`, and the bench needs Perl. `jlog` runs on macOS and Linux. The tests ran on macOS only.

## License

MIT.
