"""Guard workflow/job env contexts before GitHub rejects the workflow.

Step env and action inputs can use runner/steps/env. Workflow and job env
cannot: runner-local paths must be exported from a step via GITHUB_ENV.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]


def invalid_env_contexts(text):
    allowed = {0: {'github', 'inputs', 'vars', 'secrets'},
               4: {'github', 'inputs', 'vars', 'secrets', 'needs', 'strategy', 'matrix'}}
    scope = None
    invalid = []
    for number, line in enumerate(text.splitlines(), 1):
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        indent = len(line) - len(line.lstrip())
        if scope is not None and indent <= scope:
            scope = None
        if line.strip() == 'env:' and indent in allowed:
            scope = indent
        if scope is None:
            continue
        for expression in re.findall(r'\$\{\{(.*?)\}\}', line):
            expression = re.sub(r"'(?:[^']|'')*'", "''", expression)
            contexts = re.findall(r'(?<![\w.])([A-Za-z_]\w*)\s*(?:\.|\[)', expression)
            forbidden = set(contexts) - allowed[scope]
            if forbidden or re.search(r'\bhashFiles\s*\(', expression):
                invalid.append((number, sorted(forbidden or {'hashFiles'})))
    return invalid


class WorkflowContexts(unittest.TestCase):
    def test_workflow_and_job_env_only_use_available_contexts(self):
        for workflow in sorted((ROOT / '.github/workflows').glob('*.yml')):
            with self.subTest(workflow=workflow.name):
                self.assertEqual(invalid_env_contexts(workflow.read_text()), [])

    def test_controls_reject_runner_steps_env_and_hashfiles_before_step_scope(self):
        for indent in ('', '    '):
            for expression in ('runner.temp', 'steps.build.outputs.dir', 'env.CACHE', "hashFiles('*.lock')"):
                with self.subTest(indent=indent, expression=expression):
                    text = f'{indent}env:\n{indent}  CACHE: ${{{{ {expression} }}}}\n'
                    self.assertTrue(invalid_env_contexts(text))
        self.assertTrue(invalid_env_contexts('env:\n  X: ${{ matrix.target }}\n'))
        self.assertEqual(invalid_env_contexts('    env:\n      X: ${{ matrix.target }}\n'), [])
        self.assertEqual(invalid_env_contexts('        env:\n          X: ${{ runner.temp }}\n'), [])

    def test_runner_local_paths_are_exported_before_their_consumers(self):
        fuzz = (ROOT / '.github/workflows/phase7-fuzz.yml').read_text()
        self.assertIn('echo "FUZZ_OUTPUT=$RUNNER_TEMP/', fuzz)
        self.assertIn('echo "FUZZ_BUILD=$RUNNER_TEMP/', fuzz)
        self.assertIn('>> "$GITHUB_ENV"', fuzz)
        self.assertLess(fuzz.index('Set runner-local output paths'), fuzz.index('Build and preserve'))


if __name__ == '__main__':
    unittest.main()
