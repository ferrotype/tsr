"""CI artifacts retain their source roster while executable paths relocate."""
from pathlib import Path
import json
import shutil
import tempfile
import sys
import unittest
from unittest.mock import patch
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.phase5.harness import bundle


class BundleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.build = self.root / 'build'
        self.build.mkdir()
        self.output = self.root / 'artifact'
        self.prepared = {}
        for name in ('fourslash-tests','lsp-tests','test2json','phase5_testserver'):
            path = self.build/name
            path.write_bytes(name.encode())
            path.chmod(0o755)
        for suite in bundle.SUITES:
            self.prepared[suite] = dict(suite=suite, binary=str(self.build/f'{suite}-tests'),
                test2json=str(self.build/'test2json'), server=str(self.build/'phase5_testserver'),
                cwd='/old/checkout/upstream/tsc',stage='/old/stage',go='/old/go',
                compiled_sources=[f'upstream/tsc/internal/{suite}/example_test.go'],goos='linux',goarch='amd64')
        bundle.export_bundle(self.prepared,self.output)
        self.checkout = self.root/'new-checkout'
        (self.checkout/'upstream/tsc').mkdir(parents=True)

    def test_relocate_after_artifact_download(self):
        downloaded=self.root/'downloaded'
        shutil.copytree(self.output,downloaded)
        shutil.rmtree(self.build)
        for path in downloaded.iterdir():
            path.chmod(0o644)
        manifests=bundle.relocate_bundle(downloaded,self.checkout)
        for suite,path in manifests.items():
            info=json.loads(Path(path).read_text())
            self.assertEqual(info['cwd'],str(self.checkout/'upstream/tsc'))
            self.assertEqual(info['stage'],str(downloaded))
            self.assertEqual(info['compiled_sources'],self.prepared[suite]['compiled_sources'])
            self.assertNotIn('go',info)
            for key in ('binary','server','test2json'):
                executable=Path(info[key])
                self.assertEqual(executable.parent,downloaded)
                self.assertTrue(executable.stat().st_mode & 0o111)
        self.assertEqual(manifests,bundle.relocate_bundle(downloaded,self.checkout))

    def test_prepare_reuses_server_and_test2json_for_second_suite(self):
        from tools.phase5.harness import runner
        with patch.object(runner,'prepare',side_effect=[self.prepared['fourslash'],self.prepared['lsp']]) as prepare:
            bundle.prepare_bundle(self.root/'fresh-artifact',self.root/'stage')
        self.assertEqual(prepare.call_count,2)
        self.assertEqual(prepare.call_args_list[1].kwargs,
            {'prebuilt_server':self.prepared['fourslash']['server'],
             'prebuilt_test2json':self.prepared['fourslash']['test2json']})

    def test_runtime_uses_relocated_checkout_without_go(self):
        from tools.phase5.harness import runner
        manifests=bundle.relocate_bundle(self.output,self.checkout)
        info=json.loads(Path(manifests['fourslash']).read_text())
        with patch.dict('os.environ',{'TSR_UPSTREAM_ROOT':'/old/checkout','TSR_LSP_SERVER':'/old/server'}):
            native=runner._runtime_env(info,self.root/'native',native=True)
            rust=runner._runtime_env(info,self.root/'rust',native=False)
        self.assertEqual(native['TSR_UPSTREAM_ROOT'],str(self.checkout/'upstream/tsc'))
        self.assertNotIn('TSR_LSP_SERVER',native)
        self.assertEqual(rust['TSR_LSP_SERVER'],info['server'])

    def test_changed_artifact_rejected(self):
        (self.output/'phase5_testserver').write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError,'bytes changed'):
            bundle.relocate_bundle(self.output,self.checkout)

    def test_path_escape_rejected(self):
        manifest=self.output/'fourslash.portable.json'
        info=json.loads(manifest.read_text());info['binary']='../build/fourslash-tests'
        manifest.write_text(json.dumps(info))
        with self.assertRaisesRegex(ValueError,'path'):
            bundle.relocate_bundle(self.output,self.checkout)

    def test_shared_executable_conflict_rejected(self):
        alternative=self.build/'different-test2json';alternative.write_bytes(b'wrong')
        self.prepared['lsp']['test2json']=str(alternative)
        with self.assertRaisesRegex(ValueError,'conflicting'):
            bundle.export_bundle(self.prepared,self.root/'conflict')


if __name__ == '__main__':
    unittest.main()
