"""Archive consumer execution follows Cargo's selected output artifact."""
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import package_verify


class ConsumerArtifactTests(unittest.TestCase):
    def test_target_override_ignores_a_stale_default_binary(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stale = root / 'build/debug/package_consumer'
            stale.parent.mkdir(parents=True)
            stale.write_text('old consumer')
            selected = root / 'build/aarch64-apple-darwin/debug/package_consumer'
            selected.parent.mkdir(parents=True)
            selected.write_text('current consumer')
            manifest = root / 'consumer/Cargo.toml'
            message = {'reason': 'compiler-artifact', 'manifest_path': str(manifest),
                       'target': {'name': 'package_consumer', 'kind': ['bin']},
                       'executable': str(selected)}
            log = root / 'native.log'
            log.write_text('   Compiling package_consumer\n' + json.dumps(message) + '\n')
            self.assertEqual(package_verify.built_executable(log, manifest, 'package_consumer'), selected)
            self.assertNotEqual(selected, stale)

    def test_missing_wrong_target_and_ambiguous_artifacts_fail(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            manifest = root / 'consumer/Cargo.toml'
            message = {'reason': 'compiler-artifact', 'manifest_path': str(manifest),
                       'target': {'name': 'package_consumer', 'kind': ['bin']},
                       'executable': str(root / 'consumer-one')}
            log = root / 'native.log'
            cases = [[], [{**message, 'manifest_path': str(root / 'other/Cargo.toml')}],
                     [{**message, 'target': {'name': 'package_consumer', 'kind': ['example']}}],
                     [message, {**message, 'executable': str(root / 'consumer-two')}],
                     [{**message, 'target': {'name': 'tsrust', 'kind': ['bin']}}]]
            for messages in cases:
                with self.subTest(messages=messages):
                    log.write_text('\n'.join(map(json.dumps, messages)))
                    with self.assertRaisesRegex(ValueError, 'exactly one'):
                        package_verify.built_executable(log, manifest, 'package_consumer')


class CliSmokeTests(unittest.TestCase):
    """The packaged tsrust must print the pin's version, emit and type check."""

    def fake(self, root, version, emit, error):
        for name, text in (('version', version), ('emit', emit), ('error', error)):
            (root / name).write_text(text)
        script = root / 'tsrust'
        script.write_text(f"""#!/bin/sh
case "$1" in
  --version) cat '{root}/version' ;;
  hello.ts) mkdir -p out && cat '{root}/emit' > out/hello.js ;;
  bad.ts) cat '{root}/error'; exit 2 ;;
esac
""")
        script.chmod(0o755)
        return script

    def test_expected_observations_pass_and_each_difference_fails(self):
        good = (package_verify.CLI_VERSION, package_verify.CLI_EMIT, package_verify.CLI_ERROR)
        cases = {'pass': good, '--version': ('Version 7.0.0\n', *good[1:]),
                 'compile': (good[0], 'const y = x + 1;\n', good[2]), 'type error': (*good[:2], '')}
        for name, observed in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                executable = self.fake(root, *observed)
                if name == 'pass':
                    result = package_verify.cli_smoke(executable, root / 'smoke', dict(os.environ))
                    self.assertEqual(result['compile']['emitted'], {'out/hello.js': package_verify.CLI_EMIT})
                else:
                    with self.assertRaisesRegex(ValueError, 'packaged tsrust ' + name):
                        package_verify.cli_smoke(executable, root / 'smoke', dict(os.environ))


if __name__ == '__main__':
    unittest.main()
