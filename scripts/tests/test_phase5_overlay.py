"""The carried patch cannot replace native test assertions."""
from pathlib import Path
import json
import sys
import tempfile
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'tools/phase5/harness'))
import overlay


class OverlayTests(unittest.TestCase):
    def test_unique_anchors_fail_closed(self):
        for source in ('absent', 'xx'):
            with self.assertRaises(ValueError):
                overlay.replace_once(source, 'x', 'replacement')

    def test_no_test_case_or_baseline_is_replaced(self):
        with tempfile.TemporaryDirectory() as tmp:
            patches = json.loads(overlay.create(tmp).read_text())['Replace']
            self.assertFalse(any('/internal/fourslash/tests/' in key or '/testdata/' in key for key in patches))
            expected = {
                'internal/repo/paths.go',
                'internal/testutil/lsptestutil/lspclient.go',
                'internal/fourslash/statebaseline.go',
                'internal/testutil/baseline/baseline.go',
                'internal/testutil/lsptestutil/rust_client.go',
                'internal/testutil/lsptestutil/rust_mappers.go',
                'internal/fourslash/rust_state.go',
                'internal/testutil/baseline/tsr_reporting.go',
            }
            self.assertEqual(set(patches), {str(overlay.UPSTREAM / path) for path in expected})
            baseline = Path(patches[str(overlay.UPSTREAM / 'internal/testutil/baseline/baseline.go')]).read_text()
            original = (overlay.UPSTREAM / 'internal/testutil/baseline/baseline.go').read_text()
            self.assertEqual(baseline.count('t.Error('), original.count('t.Error('))
            self.assertEqual(baseline.count('t.Errorf('), original.count('t.Errorf('))
            self.assertEqual(baseline.count('*writeError = fmt.Errorf'), 4)
            client = Path(patches[str(overlay.UPSTREAM / 'internal/testutil/lsptestutil/lspclient.go')]).read_text()
            self.assertIn('native server reached in Rust mode', client)
            self.assertIn('return newRustClient', client)

    def test_repository_root_override_validates_and_preserves_default(self):
        with tempfile.TemporaryDirectory() as tmp:
            patches=json.loads(overlay.create(tmp).read_text())['Replace']
            source=Path(patches[str(overlay.UPSTREAM/'internal/repo/paths.go')]).read_text()
        original=(overlay.UPSTREAM/'internal/repo/paths.go').read_text()
        self.assertIn('if !filepath.IsAbs(root)',source)
        self.assertIn('os.Stat(filepath.Join(root, "go.mod"))',source)
        self.assertIn('if err != nil || !info.Mode().IsRegular()',source)
        self.assertEqual(source[source.index('\n\t_, filename, _, ok := runtime.Caller(0)'):],
            original[original.index('\n\t_, filename, _, ok := runtime.Caller(0)'):])

    def test_state_writer_method_bodies_are_original(self):
        source = (overlay.UPSTREAM / 'internal/fourslash/statebaseline.go').read_text()
        expected = source[source.index('func (f *FourslashTest) printProjectsDiff'):]
        expected = expected.replace('(f *FourslashTest)', '(f *projectionWriter)').replace('snapshot *project.Snapshot', 'snapshot *wireSnapshot').replace('*compiler.Program', '*wireProgram').replace('map[string]projectInfo', 'map[string]*wireProgram')
        self.assertTrue(overlay.state_adapter().endswith(expected))
