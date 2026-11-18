---
title: Sessions
---

A session is one session of a harness, such as Claude Code or Codex, or one terminal session of a person. Each line of the run log names its session, so a gate can tell which agent ran what.

## Where the session comes from

`jlog` finds the session in the environment, in this order:

| Harness | Where the ID comes from |
|---|---|
| Codex | `CODEX_THREAD_ID`. |
| Claude Code | `CLAUDE_CODE_SESSION_ID`, or else `JLOG_SESSION`, which a hook can set. `child` is true when `CLAUDE_CODE_CHILD_SESSION` is set and is not `0`. |
| A terminal | The process ID of the leader of the terminal session, with its start time. So all `just` calls in one terminal share one session. |

The first line of a new session also logs the records `session`, `env` and `tools`. `jlog` marks the session as seen with an empty file in `.agents/log/sessions/`, so it logs each session only once.

This work makes the first line of a session slower, by about 0.4 to 0.9 s. A failure in this work never fails the line.

## The commands

Each command prints one JSON record, and adds it to the run log when it finds a log folder. Each record has `kind`, `run`, `harness`, `session`, `child` and `time`.

| Command | What it logs |
|---|---|
| `jlog session` | The fields `transcript`, `model` and `parent`, from the plugin of the harness. A terminal has no plugin. |
| `jlog env` | The name of each variable, and its value only for the safe list of the page [Run log](log.md). Each other value shows as `redacted`. |
| `jlog tools` | The version of each program of the tools table of the spec, with `--version`. |

A command with no log folder still prints and exits with 0. A failed write exits with 1, and a wrong argument exits with 2.

**`jlog env`** also tells, for each variable, whether its value changed since the last snapshot. It shows `changed` as true or false, or as null when it has nothing to compare with. The list `gone` names the variables that the last snapshot had and this one has not. `jlog` compares keyed hashes, so a reader of the hashes cannot test a guess of a value.

**`jlog tools`** runs all programs at once, with a limit of 3 seconds. It sets `UV_PYTHON_DOWNLOADS=never`, so uv downloads no Python. A missing program shows `"error": "not found"`.

With no argument, `session`, `env` and `tools` are commands. With arguments, they are the shell form, so `jlog env sh -c LINE` still runs a shell.

## The plugins

The folder `plugins/` of the repository holds one plugin for each harness, `claude` and `codex`. A plugin is a small Python program that finds the transcript and the model of the session. `just install` copies the plugins to `~/.local/share/jlog/plugins/`, or to the folder in `JLOG_PLUGINS`. `jlog` looks for them in the same place when it runs, so set `JLOG_PLUGINS` for both or for neither. It must be an absolute path.

A plugin reads only JSONL files, and writes nothing in the folders of a harness:

- **The plugin for Claude Code** looks for `projects/*/<session>.jsonl` in `CLAUDE_CONFIG_DIR`, or else in `~/.claude`. The model is the model of the last answer.
- **The plugin for Codex** looks for `sessions/*/*/*/rollout-*-<thread>.jsonl` in `CODEX_HOME`, or else in `~/.codex`. The model comes from the last `turn_context` line. The plugin never opens the SQLite files of Codex.

`jlog` runs a plugin with `uv`, with a limit of 5 seconds. A plugin reads at most the last 8 MB of a transcript.
