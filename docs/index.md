---
title: jlog
---

`jlog` is the shell of every `just` recipe. It runs each line of a recipe with the real shell, and keeps a run log of what ran. The commit gates of the stack read that log, to check that the lint, the tests and the reviews ran for a tree.

`just` has no log of its own. So a Justfile sets `jlog` as its shell:

```just
set shell := ["jlog", "bash", "-euo", "pipefail", "-c"]

test:
  cargo test
```

Each line then runs as before, and `jlog` adds one JSON line to the run log, `.agents/log/just.jsonl`.

Before it runs anything, `jlog` also sets the PATH and the environment of the project. It reads them from the `.jlog` folders above the current folder. So each recipe line, and each program that it starts, finds the tools of the project first.

The gates stop the honest mistakes of agents. They do not stop an agent that cheats.

## What the pages cover

| Page | What it covers |
|---|---|
| [Shell](shell.md) | The shell form, the shells and flags that `jlog` knows, and why lines with a pipe need `pipefail`. |
| [Environment](environment.md) | How `jlog` sets the PATH and the variables from `.jlog` folders, and the commands `jlog run` and `jlog path`. |
| [Run log](log.md) | The fields of each line of the log, the run ID, the worktree tree, the log folder and the secrets. |
| [Sessions](sessions.md) | How `jlog` finds the session of a harness, and the commands `session`, `env` and `tools`. |
| [Pass](pass.md) | The commands `jlog step`, `jlog skip` and `jlog pass`, and the rules that a pass checks. |

## Terms

| Term | Meaning |
|---|---|
| Run log | The file `.agents/log/just.jsonl`, with one JSON line for each line that ran. |
| Run | One `just check`, or one other call of `just`, with the inner `just` calls that it makes. All its lines have the same run ID. |
| Step | One recipe of `just check`, for example `lint` or `test`. The list is in `.agents/stack.json`. |
| Worktree tree | The git tree of all files in the worktree, staged or not, without the files that git ignores. |
| Pass | A record that says that every step of `just check` passed for one worktree tree. |
| Harness | The program that runs an agent, for example Claude Code or Codex. |
| Session | One session of a harness, or one terminal session of a person. |
| Scope | A folder that holds `.jlog/bin` or `.jlog/environment.json`. It gives the tools and the variables of the folders below it. |

## Install

`jlog` needs Rust and `just`. The tests also need git.

1. Clone the repository: `git clone https://github.com/rahuldave/jlog`.
2. Run `just install` in the clone.

The recipe builds `jlog` and installs it with `cargo install`, so the program goes to `~/.cargo/bin`. It also copies the plugins for Claude Code and Codex to `~/.local/share/jlog/plugins/`, and makes the environment of each one.

To install one release, check out its tag first, for example `git checkout v0.2.0`. Each release has a tag `vX.Y.Z` that matches the version in `Cargo.toml`.

`jlog` runs on macOS and Linux.

## More

The [README](https://github.com/rahuldave/jlog#readme) holds the full reference, and these pages follow it. The design of the commit gates is in [docs/spec.md](https://github.com/rahuldave/jlog/blob/main/docs/spec.md).
