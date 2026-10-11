"""Child-lint shared-library environment controls; no compiler builds."""
import argparse
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools.phase7.checkchildren import run


class ChildLintEnvironment(unittest.TestCase):
    def test_linux_host_libraries_precede_caller_path_without_losing_environment(self):
        original = {'LD_LIBRARY_PATH': '/caller/lib', 'CARGO_HOME': '/caller/cargo'}
        with patch.dict(os.environ, original, clear=True), patch.object(run.sys, 'platform', 'linux'):
            env = run.runtime_environment('/pinned/sysroot')
            self.assertEqual(env['LD_LIBRARY_PATH'], '/pinned/sysroot/lib:/caller/lib')
            self.assertEqual(env['CARGO_HOME'], '/caller/cargo')
            self.assertEqual(dict(os.environ), original)

    def test_empty_loader_path_does_not_add_current_directory(self):
        for platform, variable in [('linux', 'LD_LIBRARY_PATH'), ('darwin', 'DYLD_LIBRARY_PATH')]:
            with self.subTest(platform=platform), patch.dict(os.environ, {variable: ''}, clear=True), \
                    patch.object(run.sys, 'platform', platform):
                self.assertEqual(run.runtime_environment('/pinned/sysroot')[variable],
                                 '/pinned/sysroot/lib')

    def test_direct_fixture_receives_runtime_environment(self):
        env = {'LD_LIBRARY_PATH': '/pinned/sysroot/lib'}
        class InvocationObserved(Exception):
            pass
        def observe(*args, **kwargs):
            self.assertIs(kwargs['env'], env)
            raise InvocationObserved
        with patch.object(run.subprocess, 'run', side_effect=observe):
            with self.assertRaises(InvocationObserved):
                run.fixtures(Path('/checker'), 'pinned-nightly', env)

    def test_workspace_wrapper_receives_same_runtime_environment(self):
        env = {'LD_LIBRARY_PATH': '/pinned/sysroot/lib', 'CARGO_HOME': '/caller/cargo'}
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / 'check.log'
            def observe(command, **kwargs):
                self.assertEqual(kwargs['env']['LD_LIBRARY_PATH'], env['LD_LIBRARY_PATH'])
                self.assertEqual(kwargs['env']['CARGO_HOME'], env['CARGO_HOME'])
                self.assertEqual(kwargs['env']['RUSTC_WORKSPACE_WRAPPER'], '/checker')
                kwargs['stdout'].write('checkchildren: 2 workers, 1 dispatched methods, 0 early returns\n')
                return argparse.Namespace(returncode=0)
            with patch.object(run.sys, 'argv', ['run.py', 'check', '--output', str(output)]), \
                    patch.object(run, 'build', return_value=('pinned-nightly', Path('/checker'), env)), \
                    patch.object(run, 'fixtures') as fixtures, \
                    patch.object(run.subprocess, 'run', side_effect=observe), \
                    patch('builtins.print'):
                self.assertEqual(run.main(), 0)
            fixtures.assert_called_once_with(Path('/checker'), 'pinned-nightly', env)


if __name__ == '__main__':
    unittest.main()
