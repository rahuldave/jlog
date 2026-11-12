"""The Claude Code plugin of jlog.

It finds the transcript of the session in CLAUDE_CODE_SESSION_ID, or in
JLOG_SESSION, and the model of the last answer. It prints one JSON object. It
reads only JSONL files, and it writes nothing.
"""

import json
import os
import re
import sys
from pathlib import Path

# A session ID: letters, digits, `.`, `_` and `-`, so it cannot name another folder.
ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,199}\Z")
# The most bytes read from the end of a transcript.
TAIL = 8 << 20
CHUNK = 1 << 16


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
            # The first part may be the end of a line that starts before
            # this block. It is whole only at the start of the file.
            rest = parts[0]
            for line in reversed(parts[1:]):
                if line.strip():
                    yield line
        if stop == 0 and rest.strip():
            yield rest


def config_dir(env):
    d = env.get("CLAUDE_CONFIG_DIR")
    return Path(d) if d else Path(env.get("HOME", "~")).expanduser() / ".claude"


def find_transcript(config, session):
    """The newest `projects/<folder>/<session>.jsonl`, or None."""
    projects = config / "projects"
    found = []
    try:
        folders = list(projects.iterdir())
    except OSError:
        return None
    for d in folders:
        p = d / (session + ".jsonl")
        try:
            if p.is_file():
                found.append((p.stat().st_mtime, str(p)))
        except OSError:
            continue
    return max(found)[1] if found else None


def last_model(path):
    """The model of the last answer in the transcript, or None."""
    for line in lines_from_end(path):
        if b'"model"' not in line:
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            continue
        if not isinstance(obj, dict) or obj.get("type") != "assistant":
            continue
        message = obj.get("message")
        model = message.get("model") if isinstance(message, dict) else None
        # Claude Code writes `<synthetic>` for a message that no model wrote.
        if isinstance(model, str) and model and not model.startswith("<"):
            return model
    return None


def find(env):
    out = {"harness": "claude", "session": None, "transcript": None, "model": None, "parent": None}
    session = (env.get("CLAUDE_CODE_SESSION_ID") or env.get("JLOG_SESSION") or "").strip()
    if not session:
        out["error"] = "no session: CLAUDE_CODE_SESSION_ID and JLOG_SESSION are unset"
        return out
    out["session"] = session
    if not ID.match(session):
        out["error"] = "the session ID is not a plain name"
        return out
    path = find_transcript(config_dir(env), session)
    if path is None:
        out["error"] = "no transcript for the session"
        return out
    out["transcript"] = path
    try:
        out["model"] = last_model(path)
    except OSError as e:
        out["error"] = "could not read the transcript: " + (e.strerror or str(e))
    return out


def main():
    print(json.dumps(find(os.environ)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
