"""The quality gate must execute root suites as well as registered suites."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from run_tests import cases, discover


class Discovery(unittest.TestCase):
    def test_all_root_tests_are_in_the_gate_exactly_once(self):
        root = Path(__file__).resolve().parents[2]
        actual = [case.id() for case in cases(discover(root))]
        self.assertEqual(len(actual), len(set(actual)))
        for path in (root / 'scripts').glob('test_*.py'):
            expected = unittest.defaultTestLoader.loadTestsFromName(path.stem)
            for case in cases(expected):
                self.assertIn(case.id(), actual, path.name)
        self.assertTrue(any(name.startswith('test_s10.') for name in actual))
        self.assertFalse(any('_FailedTest' in name for name in actual))
