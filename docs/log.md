---
title: The run log
---

Each line of a recipe adds one JSON line to the run log, `.agents/log/just.jsonl`:

```json
{"run":"48213-1794498589","argv":["just","test"],"line":"cargo test","exit":0,"start":"2026-11-12T10:00:00Z","end":"2026-11-12T10:00:04Z","kind":"line","harness":"terminal","session":"301-1794490000","child":false}
```

| Field | Meaning |
|---|---|
| `run` | The run ID. The section "Runs" says where it comes from. |
| `argv` | The `just` command as a list, with each argument whole. It is empty when the parent of `jlog` is not `just`. |
| `line` | The line of the recipe, before the shell expands it. It is null when `jlog` cannot tell which argument is the line. |
| `exit` | The exit code of the shell. A shell that a signal stopped gets 128 plus the number of the signal. |
| `start`, `end` | The times in UTC, to the second. |
| `kind` | `line` for a line of a recipe, and `backtick` for a backtick. The gates ignore backticks. |
| `harness` | `codex`, `claude` or `terminal`. |
| `session` | The ID of the session. The page [Sessions](sessions.md) says where it comes from. |
| `child` | True for a line of a fresh agent, which a harness starts for one task. It is null in Codex. |

`jlog` reads the argv of its parent only when the program of the parent is named `just`.

## Runs

A run is one `just check`, or one other call of `just`, with the inner `just` calls that it makes. The root Justfile exports one run ID, so that all of them share one run:

```just
export JLOG_RUN := env("JLOG_RUN", uuid())
```

The first `just` call makes the ID, and each inner call inherits it. A module must not have the line, because it would make a second ID. Without `JLOG_RUN`, the run is the process ID of the parent `just` and its start time. If the parent of `jlog` is not `just`, the parent counts in place of `just`.

Before the first line of a run starts, `jlog` computes the worktree tree and logs it in a record `tree`. The tree is null when git cannot compute it. The first line holds a lock until the record is in the log, so a parallel line of the run waits for it.

## The worktree tree

The worktree tree is the git tree of all files in the worktree, staged or not, without the files that git ignores. `jlog` computes it with a temporary index and a temporary object store. So the real index does not change, and `.git/objects` gets no new object:

```sh
cp <the real index> <tmp>/index
export GIT_INDEX_FILE=<tmp>/index
export GIT_OBJECT_DIRECTORY=<tmp>/objects
export GIT_ALTERNATE_OBJECT_DIRECTORIES=<the real .git/objects>
git add -A -- :/
git write-tree
```

git reads the objects that it has already from the real store, and writes the new ones into the temporary store. `jlog` then deletes the folder `<tmp>`.

The tree objects go away with the folder. So the log keeps the ID of each tree, but `git ls-tree` cannot read a tree that was never committed.

A `jlog` that is killed cannot delete its folder. So before it makes a folder, `jlog` deletes the old folders whose process no longer runs. It leaves a folder that changed in the last hour, or whose lock is held.

## The log folder

`jlog` goes up from the current folder to the first folder that holds `.agents/`. It does not call git, so each line costs little time. The search stops in two places:

- **At the top of a repository.** A folder that holds `.git` ends the search, so a repository without `.agents/` gets no log.
- **At a home folder.** `jlog` never logs into `~/.agents`, which holds the global skills.

If `jlog` finds no folder, it runs the line and logs nothing. On its first write, it makes `.agents/log/.gitignore` with the one line `*`. So `git add -A`, `git stash -u` and `git clean -fd` leave the log folder alone.

`jlog` writes no file through a symlink. The run log must be a regular file with one link.

## Secrets

A line can hold a secret, for example a value from a `.env` file. Before `jlog` logs `line` and `argv`, it removes the value of each variable of its environment that has 8 or more characters. It writes `<redacted>` in place of the value. These variables are safe, and `jlog` keeps their values:

```text
PATH SHELL HOME PWD OLDPWD TERM CI CODEX_THREAD_ID CLAUDE_CODE_SESSION_ID CLAUDE_CODE_CHILD_SESSION
```

`jlog` trusts these names whatever set them. So do not put a secret in one of them, for example with `PWD=...` in a `.env` file.

An argument of a recipe is in the environment only with `set export` or `set dotenv-load`. If the argument is not in the environment, `jlog` cannot know that it is a secret.
