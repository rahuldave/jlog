You are starting the `jlog` project in ~/Projects/jlog. It is a new Rust binary made with `cargo new`, on branch `main`, with no commits yet. The repository is `rahuldave/jlog` on GitHub, and pushes to `main` are allowed.

## Read first

1. `docs/spec.md` in this repo. It is the full design of the commit gates. `jlog` is one part of it. Read these sections closely:
   - "The tools"
   - "The run log"
   - "Sessions and the environment"
   - "The worktree tree"
   - "The pass"
   - "The files that the installer writes"
   - "Acceptance Criteria", the `jlog` parts.
2. The folder ~/Projects in general, for context. Treat the other repos as read-only. Make no edits and no git writes, and do not run `git fetch`. The repos that matter:
   - **jnterfaces:** the installer, the `just` skill and its command contract (`.agents/skills/just/references/command_contract.md`), `scripts/shared_files.py` and `scripts/formats.json`.
   - **agent_gest_git_skills:** the hooks in `.claude/hooks` and `.codex`.
   - **jagt:** a Rust tool by the same author. Use its style, layout and Justfile as the model for jlog.
   - **jnprofile-python** and **jnprofile-typescript:** the profiles.
   - **nossaifd:** the writing guide `GUIDE.md` and `check.py`.

## What to build

The `jlog` part of the spec:

- **The shell form** that just calls, `jlog <shell> <flags> <line> [args...]`. It runs the line with the given shell and the positional arguments, and adds one JSON line to `.agents/log/just.jsonl`. It finds the log folder by going up from the current folder, with no call to `git`. It creates `.agents/log/.gitignore` with `*` on its first write.
- **The fields of each line** are the run ID, argv, line, exit, start, end and the backtick tag. Redact secrets from `line` and `argv` as the spec says.
- **The commands** `jlog skip`, `jlog pass`, `jlog env`, `jlog tools` and `jlog session`. Detect the session with `CODEX_THREAD_ID`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_CHILD_SESSION`, and `getsid(0)` as the fallback. Mark a session as seen with `.agents/log/sessions/<id>`.
- **The pass rules** of the section "The pass". Compute the worktree tree with a temporary index.
- **The plugins:** `plugins/claude/` and `plugins/codex/`, each its own uv project (`pyproject.toml`, `uv.lock`, `session.py`), run with `uv run --project <dir> --frozen`. They read only JSONL transcripts, never a harness's SQLite files, and write nothing in a harness's folders.
- **A Justfile with the usual recipes.** They are `build`, `test`, `lint`, `check` and `install`. `install` builds jlog and copies the plugins to `~/.local/share/jlog/plugins/`.
- **Speed:** each line should add only a millisecond or two over the shell itself. Measure it, and report the number.

Start small and keep it working at each step. A good first commit is the shell form with logging, plus tests. Write tests for each behavior, including labs with a real `just` and a temporary git repo.

## Rules of this user

- **Commit messages** end with the line "Coded using Claude". Add no Claude attribution lines.
- **Before each commit,** have an adversarial reviewer try to break the change. Use a subagent on the strongest model. Fix what it finds, with a test for each finding.
- **Use `just` for every operation,** and run the recipes yourself, not raw `cargo`.
- **Write each doc** (README, docs) by ~/Projects/nossaifd/GUIDE.md, and check it with ~/Projects/nossaifd/check.py.
- **Some folders are off limits.** Do not touch ~/.claude, the jj fork or the private repositories in ~/Projects. Read ~/.codex only in read-only mode. Opening its SQLite files counts as a write, so do not open them.
- **Never print the values of environment variables.** This machine has secrets in its environment.
- **If the spec is wrong or unclear** for jlog, write the problem down in your report. Do not change `docs/spec.md` on your own.
- **End each report with four headings, in short lines.** They are Done, Not done, Issues and For you.
