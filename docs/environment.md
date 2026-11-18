---
title: The PATH and the environment
---

Each call of `jlog` first sets its PATH and its environment, and then runs. The call can be a recipe line, `jlog run`, or a command such as `jlog env`. The programs that `jlog` itself starts, such as git and the plugins, get the same environment.

The tools and the variables come from scopes. A scope is a folder that holds `.jlog/bin` or `.jlog/environment.json`.

## How jlog finds the scopes

`jlog` goes up from the current folder and collects the scopes. It does not call git, and it needs no `.agents` folder. The search:

- **Crosses the top of a repository.** A workspace folder above several repositories can hold tools for all of them.
- **Never looks at a sibling folder.** Only the current folder and the folders above it count.
- **Stops after a scope with `"root": true`.** The folders above that scope do not count.
- **Stops before a home folder or the root of the file system.**
- **Stops at a folder that every user can write to,** such as `/tmp`. Anyone could put a `.jlog` there.
- **Stops at a scope that another user owns.** The folder `.jlog`, its `bin` and its file must belong to you or to root, and not every user may write to them. `jlog` warns on stderr and uses no scope from there up.

## The order of the PATH

The PATH then has these entries, in this order:

1. The `.jlog/bin` of the nearest scope, then the folders in its `path` list.
2. The same for each scope above it, nearest first.
3. `$XDG_DATA_HOME/jlog/bin`, then the folders in the `path` list of the user file.
4. The PATH that `jlog` got.

`jlog` makes each entry absolute, skips a folder that does not exist, and adds each folder once. A folder that the inherited PATH holds already also keeps its place there, so it can appear twice. When you leave the project, the PATH is again the one you had.

## The file .jlog/environment.json

The file holds data only. This example puts a Python and a Node folder in the PATH, and gives Rust its own folders:

```json
{
  "version": 1,
  "root": false,
  "path": [".venv/bin", "node_modules/.bin"],
  "env": {
    "CARGO_HOME": {"path": ".jlog/cargo"},
    "RUSTUP_HOME": {"path": ".jlog/rustup"}
  }
}
```

| Key | Meaning |
|---|---|
| `version` | Must be `1`. Another number stops `jlog` with an error. |
| `root` | True to stop the search at this scope. False by default. |
| `path` | A list of folders for the PATH. A relative folder is based on the folder that holds `.jlog`. |
| `env` | The variables to set. A value is a string, or an object with one key, `path`. A `path` value is made absolute. |

The user file, `$XDG_CONFIG_HOME/jlog/environment.json`, has the same keys, but each path in it must be absolute.

For a variable, the nearest scope wins. A file cannot set `PATH`, `HOME`, `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, any `JLOG_` variable, or the session variables of Claude Code and Codex.

`jlog` stops with exit code 2 before it runs anything when a file is not valid. The error names the file and the key, and never a value.

`jlog` never installs a tool or sources an activation script. It adds a `.venv` or a `node_modules` folder only when a file names it.

## Moving between projects

`jlog` keeps a record of what it added in the variable `JLOG_RESOLVED`. The next call takes back only that, and then sets the environment again. Suppose an agent starts in project A and then runs `jlog` in project B:

- The `.jlog/bin` of A leaves the PATH, and the one of B comes in.
- A variable that A set goes back to its old value, or is unset. This holds when the caller did not change it.
- A change that the caller made after the first call stays.

A call in the same folder changes nothing, so nested calls are safe.

## jlog run and jlog path

`jlog run <command> [args...]` sets the environment, then runs the command in place of `jlog`. The command keeps its exit code, its signals and its terminal, and no line goes into the run log. A missing command exits with 127.

`jlog path` prints the scopes, the user folders, the order of the PATH and the names of the variables, with where each comes from. It never prints the value of a string variable, and it runs nothing.

## Start an agent through just

A recipe can start an agent, so the agent and each program that it starts find the tools of the project:

```just
set shell := ["jlog", "sh", "-cu"]

agent:
    unset JLOG_RUN; exec claude
```

The first `just` and `jlog` must be in the PATH already. The recipe unsets `JLOG_RUN`, so each `just check` that the agent runs later gets its own run ID.

A shebang recipe or a script recipe does not go through the shell of the Justfile. Such a recipe needs a PATH that is ready already, or a call of `jlog run`.
