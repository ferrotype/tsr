"""Checkerbench provenance checks without compilers or benchmark children."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import s08_checkerbench as checker
import s08_census_runtime as runtime
from s08_oracle import canonical, digest


class Fingerprints(unittest.TestCase):
    def test_capture_inputs_include_the_census_runtime_and_its_observers(self):
        captured = set(checker.sources())
        self.assertIn('scripts/s08_census_runtime.py', captured)
        for name in runtime.OBSERVER_SOURCES:
            self.assertIn('tools/s08/oracle/families/' + name, captured)

    def test_directory_inputs_include_nested_files_and_detect_edits(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            names = ('.cargo/config.toml', 'tools/s08/p4/executor.rs',
                     'tools/s08/p5/nested/display.rs', 'tools/s08/p7/child.rs',
                     'tools/s08/oracle/families/runtime_allocations.go')
            for name in names:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('initial')
            with patch.object(checker, 'ROOT', root):
                initial = checker.sources()
                self.assertEqual(set(initial), set(names))
                for name in names:
                    (root / name).write_text('changed')
                    self.assertNotEqual(checker.sources()[name], initial[name])

    def test_build_records_the_observer_bytes_supplied_to_the_compiler(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk = root / 'sdk/src/runtime'
            sdk.mkdir(parents=True)
            signature = 'func {}(size uintptr, typ *_type, needzero bool) unsafe.Pointer {{}}\n'
            (sdk / 'malloc.go').write_text(signature.format('mallocgc'))
            (sdk / 'malloc_generated.go').write_text(''.join(
                signature.format(f'mallocgcSmallNoScanSC{i}') for i in range(14)))
            source = root / 'tools/s08/oracle/families'
            source.mkdir(parents=True)
            for name in runtime.OBSERVER_SOURCES:
                (source / name).write_text('package fixture // ' + name)
            executable = root / 'rust-executable'
            executable.write_bytes(b'rust fixture')
            destination = root / 'build'

            def command(args, **kwargs):
                if args[:2] == ['go', 'version']:
                    return b'go version go1.27.1 darwin/arm64\n'
                if args[:3] == ['go', 'env', 'GOROOT']:
                    return str(root / 'sdk').encode()
                if args[:3] == ['go', 'test', '-c']:
                    Path(args[args.index('-o') + 1]).write_bytes(b'go fixture')
                return b'fixture\n'

            def overlay(directory, upstream):
                path = directory / 'overlay.json'
                path.write_bytes(canonical({'Replace': {}}))
                return path

            with patch.object(checker, 'ROOT', root), patch.object(runtime, 'ROOT', root), \
                    patch.object(checker, 'command', side_effect=command), \
                    patch.object(runtime, 'command', side_effect=command), \
                    patch.object(checker, 'write_overlay', side_effect=overlay), \
                    patch.object(checker, 'verified_upstream', return_value=root / 'upstream'), \
                    patch.object(checker, 'cargo_executable', return_value=executable), \
                    patch.object(checker, 'native_environment', return_value={}), \
                    patch.object(checker, 'go_environment', return_value={}), \
                    patch.object(checker, 'reject_concurrent_builds'):
                report = checker.build(destination, modes=['alloc'])
                observer = report['binaries']['go-alloc']
                expected = {name: digest((source / name).read_bytes()) for name in runtime.OBSERVER_SOURCES}
                self.assertEqual(observer['observer_sources_sha256'], expected)
                checker.verify_runtime_observer(destination, observer)


if __name__ == '__main__':
    unittest.main()
