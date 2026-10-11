"""Startup parser/scheduler/fake-child controls; no wasm or compiler runs."""
import copy
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from tools.phase7.budget import probe, startup


class StartupControls(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.child = self.root / 'fake.py'
        self.node = probe.identity(sys.executable)

    def identity(self, engine):
        artifact = self.root / engine
        artifact.mkdir(exist_ok=True)
        (artifact / 'build.json').write_bytes(b'{}')
        key = 'TSR_GO_WASM_ARTIFACT_DIR' if engine == 'go' else 'TSR_WASM_ARTIFACT_DIR'
        return {'engine': engine, 'node': self.node, 'node_arguments': [],
                'environment': {key: str(artifact), 'NODE_OPTIONS': None},
                'manifest': probe.identity(artifact / 'build.json'), 'files': []}

    def test_ready_requires_one_exact_positive_engine_response(self):
        ready = {'version': 1, 'kind': 'startup_ready', 'engine': 'go', 'load_processor_ns': '1'}
        self.assertEqual(startup.parse_ready(probe.encoded(ready), 'go'), 1)
        for changed in [dict(ready, engine='rust'), dict(ready, version=True), dict(ready, extra=1),
                        dict(ready, load_processor_ns='0'), dict(ready, load_processor_ns='01'),
                        dict(ready, load_processor_ns=1)]:
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                startup.parse_ready(probe.encoded(changed), 'go')
        for raw in [probe.encoded(ready) * 2, probe.encoded(ready).rstrip(), b'not json\n',
                    b'{"version":1,"version":1,"kind":"startup_ready","engine":"go","load_processor_ns":"1"}\n']:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                startup.parse_ready(raw, 'go')

    def test_javascript_loader_timer_never_invokes_compiler_processor(self):
        node = shutil.which('node')
        self.assertIsNotNone(node, 'Node is required for the loader adapter control')
        code = '''
import assert from 'node:assert/strict';
import {loadReady} from MODULE;
for (const engine of ['go', 'rust']) {
  const processor = {processLine() {throw new Error('compiler operation must not run');}};
  const ticks = [10n, 30n]; let loads = 0;
  const result = await loadReady(engine, '/fixture', async directory => {
    assert.equal(directory, '/fixture'); loads++; return processor;
  }, () => ticks.shift());
  assert.equal(loads, 1); assert.equal(result.processor, processor);
  assert.deepEqual(result.ready, {version:1,kind:'startup_ready',engine,load_processor_ns:'20'});
}
await assert.rejects(loadReady('invalid','/fixture',()=>{throw new Error('must not load');}), /engine/);
await assert.rejects(loadReady('go','/fixture',async()=>({}),()=>1n), /live processor/);
'''.replace('MODULE', json.dumps(startup.CHILD.as_uri()))
        result = subprocess.run([node, '--input-type=module', '--eval', code], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, '')

    def test_fresh_child_ack_boundary_and_raw_logs(self):
        self.child.write_text('import json,sys\nprint(json.dumps(dict(version=1,kind="startup_ready",engine=sys.argv[1],load_processor_ns="1")),flush=True)\nprint("raw diagnostic",file=sys.stderr)\n')
        identity = self.identity('go')
        row = startup.capture(identity, self.root / 'sample', 3, child=self.child)
        self.assertTrue(row['complete'], row)
        self.assertEqual(row['exit'], 0)
        self.assertEqual(row['load_processor_ns'], 1)
        self.assertGreater(row['launch_to_ready_ns'], 1)
        self.assertEqual(row['command'], [self.node['path'], str(self.child), 'go', str(self.root / 'go')])
        self.assertEqual((self.root / 'sample/stderr').read_bytes(), b'raw diagnostic\n')
        self.assertEqual(probe.decode((self.root / 'sample/record.json').read_bytes()), row)

    def test_failed_or_noisy_child_is_not_a_startup_sample(self):
        for name, body in [('failure', 'import sys\nprint("loader failed",file=sys.stderr)\nsys.exit(2)\n'),
                           ('extra', 'print(\'{}\\n{}\')\n')]:
            self.child.write_text(body)
            row = startup.capture(self.identity('rust'), self.root / name, 3, child=self.child)
            self.assertFalse(row['complete'])
            self.assertIn('failure', row)
            self.assertTrue((self.root / name / 'stderr').is_file())

    def test_timeout_kills_process_group_and_retains_partial_stdout(self):
        self.child.write_text('import os,subprocess,sys,time\np=subprocess.Popen([sys.executable,"-c","import time; time.sleep(20)"],stdout=sys.stdout)\nprint(os.getpid(),p.pid,flush=True)\ntime.sleep(20)\n')
        with patch.object(startup.os, 'killpg', wraps=os.killpg) as kill:
            row = startup.capture(self.identity('go'), self.root / 'timeout', .1, child=self.child)
        self.assertFalse(row['complete'])
        self.assertIn('timeout', row['failure'])
        parent_pid, descendant_pid = map(int, (self.root / 'timeout/stdout').read_bytes().split())
        self.assertNotEqual(parent_pid, descendant_pid)
        # Descendants inherit the new process group; kill the group rather than
        # only the process which Popen directly owns. No process-table access.
        kill.assert_called_once_with(parent_pid, startup.signal.SIGKILL)

    def test_warmup_then_seven_serial_alternating_pairs_and_no_thresholds(self):
        identities = {engine: self.identity(engine) for engine in ('go', 'rust')}
        order = []
        def capture(identity, directory, timeout):
            order.append(identity['engine'])
            return {'complete': True, 'exit': 0, 'load_processor_ns': 10 if identity['engine'] == 'go' else 20,
                    'launch_to_ready_ns': 30 if identity['engine'] == 'go' else 40}
        with patch.object(startup, 'capture', side_effect=capture), \
             patch.object(startup.measure, 'unchanged'), \
             patch.object(startup.measure, 'archive_helpers', return_value=[]):
            report = startup.run(identities, self.root / 'capture', 3)
        self.assertTrue(report['complete'], report)
        self.assertEqual(order, ['go', 'rust'] + ['go', 'rust', 'rust', 'go'] * 3 + ['go', 'rust'])
        self.assertEqual(len(report['pairs']), 7)
        self.assertFalse(report['performance_eligible'])
        self.assertIsNone(report['budget_threshold'])
        self.assertEqual(report['summary']['load_processor_ns']['rust_over_go'], 2)
        self.assertNotIn('pass', report['summary']['load_processor_ns'])

    def test_refuses_unpaired_nodes_target_and_output_under_artifacts(self):
        identities = {engine: self.identity(engine) for engine in ('go', 'rust')}
        bad = copy.deepcopy(identities)
        bad['rust']['node_arguments'] = ['--different']
        with self.assertRaisesRegex(ValueError, 'same pinned Node'):
            startup.run(bad, self.root / 'mismatch', 1)
        with self.assertRaisesRegex(ValueError, 'outside target'):
            startup.run(identities, self.root / 'target/new', 1)
        with self.assertRaisesRegex(ValueError, 'immutable artifact'):
            startup.run(identities, self.root / 'go/new', 1)


if __name__ == '__main__':
    unittest.main()
