#!/usr/bin/env python3
"""Attribute a warmed LSP operation with samply, outside acceptance timing.

Run the fixed Phase 5 session, repeating one measured request after its first
response. Every repeat must preserve the response (completion uses the same
client ordering as the latency runner). Raw frames retain all repetitions.
This is a diagnostic workload, not a latency or candidate-retention result.
"""
import argparse
import copy
import json
from pathlib import Path
import shutil
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.phase5.latency import capture  # noqa: E402
from tools.phase7.bench import common  # noqa: E402


def profile(binary, fixture, scenario, metric, repeats, output):
    if repeats < 1:
        raise ValueError('repeat count must be positive')
    fixture, binary, output = fixture.resolve(), binary.resolve(), output.resolve()
    capture.validate_scenario(scenario, fixture)
    output.mkdir(parents=True)
    record = {'binary': common.identity(binary), 'fixture_sha256': capture.identity(fixture),
              'scenario': scenario, 'metric': metric, 'repeats': repeats, 'complete': False}
    original = capture.Peer

    class RepeatedPeer(original):
        def measured_request(self, method, params):
            response, elapsed = super().measured_request(method, params)
            if method != capture.METHODS[metric]:
                return response, elapsed

            def comparable(message):
                transcript = {'responses': [{'method': method, 'message': {'result': copy.deepcopy(message['result'])}}]}
                return capture.normalize_completion_order(transcript)

            expected = comparable(response)
            started = time.monotonic_ns()
            for _ in range(repeats):
                other, _ = super().measured_request(method, params)
                if not capture.replay.exact_equal(expected, comparable(other)):
                    raise ValueError(f'{method}: repeated response changed')
            record['repeated_interval_ns'] = time.monotonic_ns() - started
            return response, elapsed

    command = ['samply', 'record', '--save-only', '--unstable-presymbolicate', '--rate', '1000',
               '-o', str(output / 'cpu.json.gz'), str(binary), '--lsp', '--stdio']
    record['command'] = command
    capture.Peer = RepeatedPeer
    try:
        with tempfile.TemporaryDirectory(prefix='tsr-r2-profile-') as temporary:
            cwd = Path(temporary).resolve() / 'project'
            shutil.copytree(fixture, cwd, symlinks=True)
            _, transcript = capture.run_runtime(command, cwd, scenario, output / 'session')
            (output / 'transcript.json').write_text(json.dumps(transcript, indent=2) + '\n')
        record['complete'] = True
    finally:
        capture.Peer = original
        (output / 'profile.json').write_text(json.dumps(record, indent=2) + '\n')
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--fixture', type=Path, required=True)
    parser.add_argument('--scenario', type=Path, required=True)
    parser.add_argument('--metric', choices=capture.METHODS, required=True)
    parser.add_argument('--repeats', type=int, default=1000)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    print(profile(args.binary, args.fixture, json.loads(args.scenario.read_text()),
                  args.metric, args.repeats, args.output))


if __name__ == '__main__':
    main()
