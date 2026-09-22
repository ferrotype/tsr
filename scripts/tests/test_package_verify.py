"""Archive consumer execution follows Cargo's selected output artifact."""
import json
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
            self.assertEqual(package_verify.consumer_executable(log, manifest), selected)
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
                     [message, {**message, 'executable': str(root / 'consumer-two')}]]
            for messages in cases:
                with self.subTest(messages=messages):
                    log.write_text('\n'.join(map(json.dumps, messages)))
                    with self.assertRaisesRegex(ValueError, 'exactly one'):
                        package_verify.consumer_executable(log, manifest)


if __name__ == '__main__':
    unittest.main()
