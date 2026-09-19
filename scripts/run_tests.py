#!/usr/bin/env python3
"""Discover both Python test locations, including legacy registration shims."""
from pathlib import Path
import sys
import unittest


def cases(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from cases(item)
        else:
            yield item


def discover(root):
    scripts = root / 'scripts'
    sys.path.insert(0, str(scripts))
    unique = {}
    for directory in (scripts / 'tests', scripts):
        loader = unittest.TestLoader()
        suite = loader.discover(str(directory), top_level_dir=str(directory))
        for case in cases(suite):
            # Some legacy root modules also have shims in scripts/tests. Run
            # every test identity once, without hiding import/discovery errors.
            unique.setdefault(case.id(), case)
    return unittest.TestSuite(unique.values())


if __name__ == '__main__':
    result = unittest.TextTestRunner().run(discover(Path(__file__).resolve().parents[1]))
    sys.exit(0 if result.wasSuccessful() else 1)
