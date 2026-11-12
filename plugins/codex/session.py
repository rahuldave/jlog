"""The Codex plugin of jlog.

It finds the rollout file of the thread in CODEX_THREAD_ID, the model of its
last turn, and its parent thread when the thread is a fresh agent. It prints
one JSON object. It reads only JSONL files, never the SQLite files of Codex,
and it writes nothing.
"""

import json
import os
import re
import sys
from pathlib import Path

# A thread ID: letters, digits and `-`, so it cannot name another folder or
# act as a pattern.
ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9-]{0,199}\Z")
TAIL = 8 << 20
CHUNK = 1 << 16
# The most bytes read from the first line, the session_meta line.
HEAD = 1 << 20


def lines_from_end(path, limit=TAIL):
    """Yields the whole lines of a file, last first, from its last `limit` bytes."""
    with open(path, "rb") as f:
        end = f.seek(0, os.SEEK_END)
        stop = max(0, end - limit)
        rest = b""
        pos = end
        while pos > stop:
            step = min(CHUNK, pos - stop)
            pos -= step
            f.seek(pos)
            block = f.read(step) + rest
            parts = block.split(b"\n")
            rest = parts[0]
            for line in reversed(parts[1:]):
                if line.strip():
                    yield line
        if stop == 0 and rest.strip():
            yield rest


def codex_home(env):
    d = env.get("CODEX_HOME")
    return Path(d) if d else Path(env.get("HOME", "~")).expanduser() / ".codex"


def find_rollout(home, thread):
    """The rollout file of the thread: the newest by name, or None."""
    name = "rollout-*-" + thread + ".jsonl"
    found = list(home.glob("sessions/*/*/*/" + name)) + list(home.glob("archived_sessions/" + name))
    found = [p for p in found if p.is_file()]
    return str(max(found, key=lambda p: p.name)) if found else None


def parent_of(path):
    """The parent thread from the session_meta line, or None."""
    with open(path, "rb") as f:
        first = f.readline(HEAD)
    try:
        obj = json.loads(first)
    except ValueError:
        return None
    payload = obj.get("payload") if isinstance(obj, dict) else None
    if not isinstance(payload, dict) or obj.get("type") != "session_meta":
        return None
    source = payload.get("source")
    if not isinstance(source, dict):
        return None
    sub = source.get("subagent")
    spawn = sub.get("thread_spawn") if isinstance(sub, dict) else None
    parent = spawn.get("parent_thread_id") if isinstance(spawn, dict) else None
    return parent if isinstance(parent, str) and parent else None


def last_model(path):
    """The model of the last turn_context line, or None."""
    for line in lines_from_end(path):
        if b'"turn_context"' not in line:
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            continue
        if not isinstance(obj, dict) or obj.get("type") != "turn_context":
            continue
        payload = obj.get("payload")
        model = payload.get("model") if isinstance(payload, dict) else None
        if isinstance(model, str) and model:
            return model
    return None


def find(env):
    out = {"harness": "codex", "session": None, "transcript": None, "model": None, "parent": None}
    thread = (env.get("CODEX_THREAD_ID") or "").strip()
    if not thread:
        out["error"] = "no thread: CODEX_THREAD_ID is unset"
        return out
    out["session"] = thread
    if not ID.match(thread):
        out["error"] = "the thread ID is not a plain name"
        return out
    path = find_rollout(codex_home(env), thread)
    if path is None:
        out["error"] = "no rollout file for the thread"
        return out
    out["transcript"] = path
    try:
        out["parent"] = parent_of(path)
        out["model"] = last_model(path)
    except OSError as e:
        out["error"] = "could not read the rollout file: " + (e.strerror or str(e))
    return out


def main():
    print(json.dumps(find(os.environ)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
