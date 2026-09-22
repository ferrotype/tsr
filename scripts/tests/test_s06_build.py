"""Retry only the bounded temporary-export cleanup race, without rerunning work."""
import errno
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s06_build


class ExportCleanupTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory()))
        (self.root / "data").mkdir()
        (self.root / "data/upstream.json").write_text('{"pin":"test-pin"}')
        self.temporary = Mock(name=str(self.root / "export"))
        self.temporary.name = str(self.root / "export")
        self.enterContext(patch.object(s06_build, "ROOT", self.root))
        self.enterContext(patch.object(s06_build, "verified_upstream", return_value=Path("/upstream")))
        self.enterContext(patch.object(s06_build, "go_environment", return_value={"GO": "test"}))
        self.enterContext(patch.object(s06_build.tempfile, "TemporaryDirectory", return_value=self.temporary))
        self.command = self.enterContext(patch.object(s06_build, "command", return_value=b"archive"))
        self.enterContext(patch.object(s06_build.tarfile, "open"))
        self.copy = self.enterContext(patch.object(s06_build.shutil, "copyfile"))
        self.sleep = self.enterContext(patch.object(s06_build.time, "sleep"))

    def test_transient_nonempty_cleanup_does_not_repeat_export_or_body(self):
        self.temporary.cleanup.side_effect = [OSError(errno.ENOTEMPTY, "race"), None]
        body_calls = 0
        with s06_build.oracle_export() as (checkout, env, pin):
            body_calls += 1
            self.assertEqual(checkout, Path(self.temporary.name))
            self.assertEqual(env, {"GO": "test"})
            self.assertEqual(pin, "test-pin")
        self.assertEqual(body_calls, 1)
        self.command.assert_called_once_with(
            ["git", "archive", "test-pin", *s06_build.EXPORT_PATHS], cwd=Path("/upstream"))
        self.assertEqual(self.copy.call_count, 4)
        self.assertEqual(self.temporary.cleanup.call_count, 2)
        self.sleep.assert_called_once_with(0.05)

    def test_successful_cleanup_does_not_wait(self):
        with s06_build.oracle_export():
            pass
        self.temporary.cleanup.assert_called_once_with()
        self.sleep.assert_not_called()

    def test_body_failure_survives_successful_cleanup_retry(self):
        failure = RuntimeError("oracle failed")
        self.temporary.cleanup.side_effect = [OSError(errno.ENOTEMPTY, "race"), None]
        with self.assertRaises(RuntimeError) as raised:
            with s06_build.oracle_export():
                raise failure
        self.assertIs(raised.exception, failure)
        self.assertEqual(self.temporary.cleanup.call_count, 2)
        self.command.assert_called_once()

    def test_unrelated_cleanup_error_is_not_retried(self):
        failure = OSError(errno.EACCES, "denied")
        self.temporary.cleanup.side_effect = failure
        with self.assertRaises(OSError) as raised:
            with s06_build.oracle_export():
                pass
        self.assertIs(raised.exception, failure)
        self.temporary.cleanup.assert_called_once_with()
        self.sleep.assert_not_called()

    def test_third_nonempty_error_propagates_with_body_error_as_context(self):
        body_error = RuntimeError("oracle failed")
        failures = [OSError(errno.ENOTEMPTY, "race") for _ in range(3)]
        self.temporary.cleanup.side_effect = failures
        with self.assertRaises(OSError) as raised:
            with s06_build.oracle_export():
                raise body_error
        self.assertIs(raised.exception, failures[-1])
        self.assertIs(raised.exception.__context__, body_error)
        self.assertEqual(self.temporary.cleanup.call_count, 3)
        self.assertEqual(self.sleep.call_count, 2)

    def test_export_failure_is_not_retried(self):
        failure = OSError(errno.ENOTEMPTY, "export failure")
        self.command.side_effect = failure
        with self.assertRaises(OSError) as raised:
            with s06_build.oracle_export():
                self.fail("failed export must not yield")
        self.assertIs(raised.exception, failure)
        self.command.assert_called_once()
        self.temporary.cleanup.assert_called_once_with()
        self.sleep.assert_not_called()


class TemporaryDirectoryCleanupTests(unittest.TestCase):
    def test_recreated_metadata_is_removed_on_retry_without_touching_siblings(self):
        with tempfile.TemporaryDirectory() as parent:
            sibling = Path(parent) / "retained"
            sibling.write_text("unrelated")
            temporary = tempfile.TemporaryDirectory(prefix="export-", dir=parent)
            cases = Path(temporary.name) / "cases"
            cases.mkdir()
            rmdir = os.rmdir
            recreated = False

            def recreate_before_removal(path, *args, **kwargs):
                nonlocal recreated
                if Path(path).name == "cases" and not recreated:
                    recreated = True
                    (cases / ".DS_Store").write_bytes(b"Finder metadata")
                return rmdir(path, *args, **kwargs)

            try:
                with patch.object(os, "rmdir", side_effect=recreate_before_removal), \
                     patch.object(temporary, "cleanup", wraps=temporary.cleanup) as cleanup, \
                     patch.object(s06_build.time, "sleep") as sleep:
                    s06_build._cleanup_export(temporary)
                self.assertTrue(recreated)
                self.assertEqual(cleanup.call_count, 2)
                sleep.assert_called_once_with(0.05)
                self.assertFalse(Path(temporary.name).exists())
                self.assertEqual(sibling.read_text(), "unrelated")
            finally:
                temporary.cleanup()


if __name__ == "__main__":
    unittest.main()
