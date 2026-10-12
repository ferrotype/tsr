"""Portable PR CI routing/merge controls; no compiler or browser builds."""
import json
from pathlib import Path
import re
import tempfile
import unittest

from scripts import parity
from scripts.tests.test_parity_embed import fixture_identity


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / '.github/workflows/ci.yml'


def job(text, name):
    match = re.search(r'^  ' + re.escape(name) + r':\n(.*?)(?=^  [\w-]+:|\Z)', text, re.M | re.S)
    if match is None:
        raise AssertionError('missing workflow job: ' + name)
    return match.group(1)


def matrix_rows(section):
    return [dict(field.strip().split(': ', 1) for field in row.split(','))
            for row in re.findall(r'^          - \{ (.+) \}$', section, re.M)]


class PortableCIControls(unittest.TestCase):
    def setUp(self):
        self.text = WORKFLOW.read_text()
        self.build = job(self.text, 'portable-build')
        self.shards = job(self.text, 'portable-parity')
        self.rows = matrix_rows(self.shards)

    def test_portable_rosters_stay_in_pr_ci_with_complete_indexed_matrices(self):
        self.assertRegex(self.text, r'(?m)^  pull_request:$')
        expected = {'compiler-embed': 4, 'compiler-wasm': 8,
                    'transpile-embed': 1, 'transpile-wasm': 1}
        self.assertEqual(len(self.rows), sum(expected.values()))
        self.assertEqual({row['suite'] for row in self.rows}, set(expected))
        for suite, count in expected.items():
            rows = [row for row in self.rows if row['suite'] == suite]
            self.assertEqual([row['shard'] for row in rows], [f'{i}/{count}' for i in range(1, count + 1)])
            self.assertTrue(all(row['runtime'] == suite.rsplit('-', 1)[1] for row in rows))
        self.assertIn('needs: [runner, portable-build]', self.shards)
        self.assertIn('if: ${{ !cancelled() }}', self.shards)
        self.assertNotIn('workflow_dispatch', self.shards)
        self.assertIn('--shard "$SHARD" --jobs "$JOBS"', self.shards)

    def test_each_consumer_is_built_once_and_transferred_with_its_layout(self):
        self.assertIn('runtime: [embed, wasm]', self.build)
        self.assertNotIn('scripts/parity.py run', self.build)
        self.assertNotIn('cargo ', self.shards)
        self.assertEqual(self.build.count('python3 scripts/s10_build.py --checker'), 1)
        self.assertEqual(self.build.count('python3 tools/phase7/portable/wasm-compiler/build.py'), 1)
        self.assertIn('key: portable-${{ matrix.runtime }}', self.build)
        self.assertIn('name: portable-consumer-embed', self.build)
        self.assertIn('path: target/release/s10_rust_consumer', self.build)
        self.assertIn('name: portable-consumer-wasm', self.build)
        self.assertIn('            target/s10/checker\n            results/wasm-compiler-artifact', self.build)
        self.assertIn('name: portable-consumer-${{ matrix.runtime }}\n          path: portable', self.shards)
        self.assertIn('consumer=portable/s10_rust_consumer\n            chmod +x "$consumer"', self.shards)
        self.assertIn('consumer=tools/phase7/portable/wasm-consumer', self.shards)
        self.assertIn('${{ github.workspace }}/portable/target/s10/checker', self.shards)
        self.assertIn('${{ github.workspace }}/portable/results/wasm-compiler-artifact', self.shards)
        # The checkout's wrapper keeps its sibling JS files and shared imports.
        self.assertNotIn('cp tools/phase7/portable/wasm-consumer', self.shards)
        self.assertIn('node-version: ${{ steps.node.outputs.version }}', self.shards)

    def test_timeouts_leave_setup_and_upload_headroom_and_results_are_unique(self):
        self.assertIn('timeout-minutes: 90', self.build)
        self.assertIn('timeout-minutes: 15', self.build)
        self.assertIn('timeout-minutes: ${{ matrix.timeout }}', self.shards)
        self.assertIn('timeout-minutes: ${{ matrix.run_timeout }}', self.shards)
        for row in self.rows:
            expected = (60, 45, 4) if row['runtime'] == 'embed' else (90, 75, 2)
            self.assertEqual(tuple(int(row[key]) for key in ('timeout', 'run_timeout', 'jobs')), expected)
        self.assertIn('--output "results/$SUITE/$(echo "$SHARD" | tr / -)"', self.shards)
        self.assertIn('name: parity-portable-${{ matrix.suite }}-${{ strategy.job-index }}', self.shards)
        check = job(self.text, 'parity-check')
        self.assertIn('portable-parity', check)
        self.assertIn('pattern: parity-*', check)
        self.assertIn('merge-multiple: true', check)
        self.assertIn('--results results/"$suite"/*', check)

    def test_actual_portable_shard_merge_requires_exact_full_coverage(self):
        variants = [f'compiler/case-{i:02d}.ts' for i in range(33)]
        with tempfile.TemporaryDirectory() as directory:
            for suite in parity.PORTABLE_SUITES:
                rows = [row for row in self.rows if row['suite'] == suite]
                outputs = []
                for row in rows:
                    selected, spec = parity.shard(variants, row['shard'])
                    output = Path(directory) / suite / row['shard'].replace('/', '-')
                    output.mkdir(parents=True)
                    meta = {'suite': suite, 'pin': 'fixture-pin', 'shard': spec,
                            'total': len(variants), 'variants': selected, 'partial': False,
                            'portable_identity': fixture_identity(suite)}
                    (output / 'meta.json').write_text(json.dumps(meta))
                    (output / 'results.ndjson').write_text(''.join(
                        json.dumps({'id': variant + '/error', 'state': 'pass'}) + '\n'
                        for variant in selected))
                    outputs.append(output)
                with self.subTest(suite=suite):
                    meta, merged = parity.merge(suite, outputs)
                    self.assertEqual(meta['total'], len(variants))
                    self.assertEqual(meta['portable_identity'], fixture_identity(suite))
                    self.assertEqual({row['id'] for row in merged}, {name + '/error' for name in variants})
                    with self.assertRaisesRegex(SystemExit, 'every shard'):
                        parity.merge(suite, outputs + outputs[:1])
                    if len(outputs) > 1:
                        with self.assertRaisesRegex(SystemExit, 'every shard'):
                            parity.merge(suite, outputs[1:])
                    result = outputs[0] / 'results.ndjson'
                    result.write_text(''.join(result.read_text().splitlines(keepends=True)[1:]))
                    with self.assertRaisesRegex(SystemExit, 'without a result'):
                        parity.merge(suite, outputs)

    def test_portable_merge_rejects_missing_wrong_runtime_and_mixed_artifacts(self):
        with tempfile.TemporaryDirectory() as directory:
            outputs = []
            for index in (1, 2):
                output = Path(directory) / str(index)
                output.mkdir()
                meta = {'suite': 'compiler-wasm', 'pin': 'fixture-pin', 'shard': [index, 2],
                        'total': 2, 'variants': [f'compiler/{index}'], 'partial': False,
                        'portable_identity': fixture_identity('compiler-wasm')}
                (output / 'meta.json').write_text(json.dumps(meta))
                (output / 'results.ndjson').write_text(json.dumps({'id': f'compiler/{index}/error', 'state': 'pass'}))
                outputs.append(output)
            meta_path = outputs[1] / 'meta.json'
            original = json.loads(meta_path.read_text())
            for identity, message in (
                (None, 'missing or invalid'),
                (fixture_identity('compiler-embed'), 'missing or invalid'),
                ({**fixture_identity('compiler-wasm'), 'build_sha256': 'a' * 64}, 'disagree on portable'),
                ({**fixture_identity('compiler-wasm'), 'engine': {**fixture_identity('compiler-wasm')['engine'], 'sha256': 'b' * 64}}, 'disagree on portable'),
            ):
                with self.subTest(identity=identity):
                    meta_path.write_text(json.dumps({**original, 'portable_identity': identity}))
                    with self.assertRaisesRegex(SystemExit, message):
                        parity.merge('compiler-wasm', outputs)

    def test_browser_and_npm_cache_is_exact_and_os_dependencies_are_not_skipped(self):
        self.assertIn('echo "PLAYWRIGHT_BROWSERS_PATH=$RUNNER_TEMP/phase7-browsers" >> "$GITHUB_ENV"', self.build)
        self.assertIn('echo "NPM_CONFIG_CACHE=$RUNNER_TEMP/phase7-browser-npm" >> "$GITHUB_ENV"', self.build)
        self.assertLess(self.build.index('Set runner-local browser cache paths'), self.build.index('id: browser-cache'))
        cache = self.build.split('id: browser-cache', 1)[1].split('      - name:', 1)[0]
        self.assertIn('uses: actions/cache@v4', cache)
        self.assertIn('${{ env.PLAYWRIGHT_BROWSERS_PATH }}', cache)
        self.assertIn('${{ env.NPM_CONFIG_CACHE }}', cache)
        for identity in ('runner.os', 'runner.arch', 'steps.portable-pins.outputs.browser-os', 'steps.portable-pins.outputs.playwright',
                         'tools/phase7/portable/browser/toolchains.json', 'upstream/package.json'):
            self.assertIn(identity, cache)
        self.assertNotIn('restore-keys', cache)
        binary_install = self.build.split('      - name: Download browser binaries on a cache miss', 1)[1].split('      - name:', 1)[0]
        self.assertIn("steps.browser-cache.outputs.cache-hit != 'true'", binary_install)
        self.assertIn('install chromium firefox webkit', binary_install)
        os_install = self.build.split('      - name: Install browser OS dependencies on the runner', 1)[1].split('      - name:', 1)[0]
        self.assertNotIn('cache-hit', os_install)
        self.assertIn('install-deps chromium firefox webkit', os_install)
        self.assertIn('playwright@${{ steps.portable-pins.outputs.playwright }}', self.build)
        self.assertEqual(self.build.count('browser/run.mjs --artifacts'), 1)
        self.assertNotIn('browser/run.mjs', self.shards)


if __name__ == '__main__':
    unittest.main()
