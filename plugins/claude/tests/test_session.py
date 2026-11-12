"""Tests of the Claude Code plugin, on a fake CLAUDE_CONFIG_DIR."""

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

SID = "5f0c2a9e-1111-4222-8333-944455556666"


def tree(root):
    """Each path under root with its size, mode and times, to see any write."""
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


class Plugin(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.config = Path(self.tmp.name) / "claude"
        self.project = self.config / "projects" / "-Users-someone-proj"
        self.project.mkdir(parents=True)
        self.transcript = self.project / (SID + ".jsonl")

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, lines):
        self.transcript.write_text("".join(json.dumps(x) + "\n" for x in lines))

    def env(self, **extra):
        e = {"CLAUDE_CONFIG_DIR": str(self.config), "CLAUDE_CODE_SESSION_ID": SID, "HOME": self.tmp.name}
        e.update(extra)
        return e

    def test_finds_the_transcript_and_the_last_model(self):
        self.write([
            {"type": "user", "message": {"content": "hi"}},
            {"type": "assistant", "message": {"model": "claude-old-1"}},
            {"type": "assistant", "message": {"model": "claude-new-2"}},
            {"type": "assistant", "message": {"model": "<synthetic>"}},
            {"type": "user", "message": {"content": "the word \"model\" in a user line"}},
        ])
        out = run(self.env())
        self.assertEqual(out["harness"], "claude")
        self.assertEqual(out["session"], SID)
        self.assertEqual(out["transcript"], str(self.transcript))
        self.assertEqual(out["model"], "claude-new-2")
        self.assertIsNone(out["parent"])
        self.assertNotIn("error", out)

    def test_jlog_session_is_the_fallback(self):
        self.write([{"type": "assistant", "message": {"model": "m"}}])
        env = self.env(JLOG_SESSION=SID)
        del env["CLAUDE_CODE_SESSION_ID"]
        self.assertEqual(run(env)["transcript"], str(self.transcript))

    def test_writes_nothing_in_the_folder_of_the_harness(self):
        self.write([{"type": "assistant", "message": {"model": "m"}}])
        before = tree(self.config)
        run(self.env())
        self.assertEqual(tree(self.config), before)

    def test_a_session_id_that_names_another_folder_is_refused(self):
        (self.config / "secret.jsonl").write_text("{}\n")
        out = run(self.env(CLAUDE_CODE_SESSION_ID="../../secret"))
        self.assertIsNone(out["transcript"])
        self.assertIn("not a plain name", out["error"])

    def test_no_session_and_no_transcript_are_errors_not_failures(self):
        env = self.env()
        del env["CLAUDE_CODE_SESSION_ID"]
        self.assertIn("unset", run(env)["error"])
        self.assertEqual(run(self.env())["error"], "no transcript for the session")

    def test_a_broken_line_and_a_huge_line_are_skipped(self):
        self.transcript.write_bytes(
            json.dumps({"type": "assistant", "message": {"model": "kept"}}).encode() + b"\n"
            + b'{"model": broken\n'
            + json.dumps({"type": "user", "pad": "x" * 300000}).encode() + b"\n"
        )
        self.assertEqual(run(self.env())["model"], "kept")

    def test_lines_from_end_gives_each_line_once_across_blocks(self):
        lines = [("line %d " % i + "y" * (i * 997 % 70000)).encode() for i in range(60)]
        self.transcript.write_bytes(b"\n".join(lines) + b"\n")
        self.assertEqual(list(session.lines_from_end(self.transcript)), lines[::-1])
        self.transcript.write_bytes(b"\n".join(lines))
        self.assertEqual(list(session.lines_from_end(self.transcript)), lines[::-1])

    def test_the_newest_of_two_transcripts_wins(self):
        self.write([{"type": "assistant", "message": {"model": "old"}}])
        other = self.config / "projects" / "-other" / (SID + ".jsonl")
        other.parent.mkdir()
        other.write_text(json.dumps({"type": "assistant", "message": {"model": "new"}}) + "\n")
        os.utime(self.transcript, (1000, 1000))
        self.assertEqual(run(self.env())["model"], "new")


if __name__ == "__main__":
    unittest.main()
