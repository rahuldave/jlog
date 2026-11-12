"""Tests of the Codex plugin, on a fake CODEX_HOME."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
SCRIPT = HERE / "session.py"
sys.path.insert(0, str(HERE))
import session  # noqa: E402

THREAD = "019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b"
PARENT = "019a0000-0000-7000-8000-000000000001"


def tree(root):
    out = {}
    for dirpath, dirs, files in os.walk(root):
        for name in dirs + files:
            st = os.lstat(os.path.join(dirpath, name))
            out[os.path.join(dirpath, name)] = (st.st_size, st.st_mode, st.st_mtime_ns, st.st_ctime_ns)
    st = os.lstat(root)
    out[str(root)] = (st.st_mode, st.st_mtime_ns, st.st_ctime_ns)
    return out


def run(env):
    full = {"PATH": os.environ.get("PATH", "")}
    full.update(env)
    out = subprocess.run([sys.executable, "-I", str(SCRIPT)], env=full, capture_output=True, check=True)
    return json.loads(out.stdout)


def meta(source):
    return {"type": "session_meta", "payload": {"id": THREAD, "session_id": THREAD, "cwd": "/p", "cli_version": "0.163.0", "source": source}}


def turn(model):
    return {"type": "turn_context", "payload": {"model": model, "cwd": "/p"}}


class Plugin(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.home = Path(self.tmp.name) / "codex"
        day = self.home / "sessions" / "2026" / "11" / "12"
        day.mkdir(parents=True)
        self.rollout = day / ("rollout-2026-11-12T10-00-00-" + THREAD + ".jsonl")
        # A SQLite file that the plugin must never open.
        (self.home / "state_5.sqlite").write_bytes(b"SQLite format 3\0")

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, lines):
        self.rollout.write_text("".join(json.dumps(x) + "\n" for x in lines))

    def env(self, **extra):
        e = {"CODEX_HOME": str(self.home), "CODEX_THREAD_ID": THREAD, "HOME": self.tmp.name}
        e.update(extra)
        return e

    def test_finds_the_rollout_and_the_model_of_the_last_turn(self):
        self.write([
            meta("cli"),
            turn("gpt-old"),
            {"type": "event_msg", "payload": {"type": "task_complete", "last_agent_message": "turn_context gpt-fake"}},
            turn("gpt-new"),
            {"type": "response_item", "payload": {"type": "message"}},
        ])
        out = run(self.env())
        self.assertEqual(out["harness"], "codex")
        self.assertEqual(out["session"], THREAD)
        self.assertEqual(out["transcript"], str(self.rollout))
        self.assertEqual(out["model"], "gpt-new")
        self.assertIsNone(out["parent"])
        self.assertNotIn("error", out)

    def test_a_fresh_agent_names_its_parent(self):
        spawn = {"subagent": {"thread_spawn": {"parent_thread_id": PARENT, "depth": 1, "agent_nickname": "r", "agent_path": "/x", "agent_role": "default"}}}
        self.write([meta(spawn), turn("gpt-x")])
        self.assertEqual(run(self.env())["parent"], PARENT)

    def test_an_archived_rollout_is_found(self):
        arch = self.home / "archived_sessions"
        arch.mkdir()
        target = arch / self.rollout.name
        target.write_text(json.dumps(meta("cli")) + "\n")
        self.assertEqual(run(self.env())["transcript"], str(target))

    def test_writes_nothing_in_the_folder_of_the_harness(self):
        self.write([meta("cli"), turn("m")])
        before = tree(self.home)
        run(self.env())
        self.assertEqual(tree(self.home), before)

    def test_a_thread_id_that_is_a_pattern_or_a_path_is_refused(self):
        self.write([meta("cli"), turn("m")])
        for bad in ["*", "../x", "a?b", "[a]"]:
            out = run(self.env(CODEX_THREAD_ID=bad))
            self.assertIsNone(out["transcript"], bad)
            self.assertIn("not a plain name", out["error"])

    def test_no_thread_and_no_rollout_are_errors_not_failures(self):
        env = self.env()
        del env["CODEX_THREAD_ID"]
        self.assertIn("unset", run(env)["error"])
        self.assertEqual(run(self.env())["error"], "no rollout file for the thread")

    def test_a_broken_first_line_gives_no_parent(self):
        self.rollout.write_bytes(b"not json\n" + json.dumps(turn("m")).encode() + b"\n")
        out = run(self.env())
        self.assertIsNone(out["parent"])
        self.assertEqual(out["model"], "m")

    def test_lines_from_end_gives_each_line_once_across_blocks(self):
        lines = [("line %d " % i + "y" * (i * 997 % 70000)).encode() for i in range(60)]
        self.rollout.write_bytes(b"\n".join(lines) + b"\n")
        self.assertEqual(list(session.lines_from_end(self.rollout)), lines[::-1])


if __name__ == "__main__":
    unittest.main()
