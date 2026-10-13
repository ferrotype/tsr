"""Child-lint shared-library environment controls; no compiler builds."""
import argparse
import os
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import patch

from tools.phase7.checkchildren import run


class ChildLintEnvironment(unittest.TestCase):
    def test_colored_cargo_cold_and_cached_summaries_are_observed(self):
        source = (run.HERE / 'fixtures/returns.rs').read_text()
        names = re.findall(r'// expect: (\w+)', source)
        planted = '\n'.join('checkchildren: Checker::' + name + ' returns before' for name in names)
        summary = '\x1b[1m\x1b[92mnote\x1b[0m\x1b[1m: checkchildren: 1 workers, 18 dispatched methods, 0 early returns\x1b[0m'
        results = [
            argparse.Namespace(returncode=1, stderr=planted + '\nearly returns'),
            argparse.Namespace(returncode=0, stderr='1 reviewed allowances'),
            argparse.Namespace(returncode=1, stderr='empty allowance reason'),
            argparse.Namespace(returncode=1, stderr='no dispatch workers found'),
            argparse.Namespace(returncode=0, stderr=summary),
            argparse.Namespace(returncode=0, stderr='\x1b[92mFresh\x1b[0m checkchildren_fixture\n' + summary),
        ]
        with patch.object(run.subprocess, 'run', side_effect=results) as invoked, patch('builtins.print'):
            run.fixtures(Path('/checker'), 'pinned-nightly', {})
        self.assertEqual(invoked.call_count, 6)

    def test_color_normalization_does_not_create_a_missing_or_failed_summary(self):
        pattern = r'note: checkchildren: 1 workers, \d+ dispatched methods, 0 early returns'
        for text in ('\x1b[92mnote\x1b[0m: unrelated',
                     '\x1b[92mnote\x1b[0m: checkchildren: 1 workers, 18 dispatched methods, 1 early returns'):
            self.assertIsNone(re.search(pattern, run.diagnostic_text(text)))

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
