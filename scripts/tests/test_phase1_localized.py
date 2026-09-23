"""Localized config integration requires all native envelopes and untouched Rust results."""
import copy
import contextlib
import io
import json
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import phase1_capture as capture
import phase1_integration as integration
import phase1_localized as localized


def fixture():
    requests = localized.schedule()
    raw, rendered = [], []
    for request in requests['requests']:
        row = {'case': request['case'], 'operation': request['operation'], 'result': 'observed',
               'observation': {'options': [], 'errors': ['localized message']}}
        raw.append(row)
        text = request['case'] + '\nlocalized message\n'
        rendered.append({**row, 'observation': {'baseline': request['baseline'], 'typed': row['observation'],
                                              'rendered': text, 'rendered_sha256': capture.digest(text.encode())}})
    request_sha = capture.digest(capture.request_bytes(requests))
    bridge_sha = capture.digest(capture.request_bytes({**requests, 'observations': raw}))
    return {'pin': capture.pin(), 'requests_sha256': request_sha,
            'raw': {'observations': raw}, 'rust': {'observations': rendered, 'request_sha256': bridge_sha},
            'native': {'observations': copy.deepcopy(rendered), 'request_sha256': request_sha}}


class LocalizedTests(unittest.TestCase):
    def test_integration_command_output_replays_the_exact_renderer_request(self):
        observed = fixture()
        leaf = {"id": "localized-config-diagnostics", "locale": "de-DE", "code": 5023,
                "leaf_localized": 'Unbekannte Compileroption "notAnOption".',
                "writer_localized": 'Unbekannte Compileroption "notAnOption".'}
        output = io.StringIO()
        with patch.object(sys, "argv", ["phase1_integration.py", "observe-localized-config"]), \
                patch.object(integration.subprocess, "run", return_value=subprocess.CompletedProcess(
                    args=[], returncode=0, stdout=json.dumps(leaf).encode())), \
                patch.object(localized, "observe", return_value={"observation": observed}), \
                contextlib.redirect_stdout(output):
            self.assertEqual(integration.main(), 0)
        recorded = json.loads(output.getvalue())
        replayed = integration.localized_integration_result(
            ROOT, recorded["leaf"]["observation"], recorded["envelopes"]["observation"])
        self.assertEqual(recorded, replayed)
        self.assertEqual(replayed["status"], "match")

    def test_sorted_raw_renderer_request_is_rejected(self):
        # Sorting named result fields is harmless to result comparison but is
        # not the byte sequence the Go renderer's request digest authenticates.
        observed = json.loads(json.dumps(fixture(), sort_keys=True))
        with self.assertRaisesRegex(ValueError, "renderer observed a different Rust request"):
            localized.compare(observed)

    def test_whole_native_envelope_is_compared(self):
        observed = fixture()
        self.assertEqual(localized.compare(observed)['status'], 'match')
        observed['native']['observations'][0]['observation']['rendered'] += 'drift'
        self.assertEqual(localized.compare(observed)['rows'][0]['result'], 'different')

    def test_partial_reordered_and_duplicate_schedules_fail(self):
        for mutate in (lambda rows: rows.pop(), lambda rows: rows.reverse(), lambda rows: rows.append(rows[0])):
            observed = fixture()
            mutate(observed['native']['observations'])
            with self.assertRaises(ValueError):
                localized.compare(observed)

    def test_renderer_cannot_substitute_the_typed_rust_result(self):
        observed = fixture()
        observed['rust']['observations'][0]['observation']['typed'] = {'wrong': True}
        with self.assertRaisesRegex(ValueError, 'typed Rust result'):
            localized.compare(observed)

    def test_missing_native_execution_or_changed_inputs_fail(self):
        for mutation in ('pin', 'requests_sha256', 'unavailable'):
            observed = fixture()
            if mutation == 'unavailable':
                observed['native']['observations'][0].update(result='native_unavailable', reason='unsupported')
            else:
                observed[mutation] = '0' * 64
            with self.assertRaises(ValueError):
                localized.compare(observed)

    def test_child_request_binding_is_replayed(self):
        for side in ('native', 'rust'):
            observed = fixture()
            observed[side]['request_sha256'] = '0' * 64
            with self.assertRaisesRegex(ValueError, 'different'):
                localized.compare(observed)
