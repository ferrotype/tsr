#!/usr/bin/env python3
"""E7/E8 replay-only producers. Missing captures never launch long work."""
import argparse
import json
import sys

from s10_corpus import ROOT, verify as corpus_verify
from s10_lifetime import verify as lifetime_verify
from s10_measure import verify as measure_verify


def producer(experiment):
    captures = [('parser-measurement', measure_verify), ('portable', measure_verify), ('wasm-corpus', corpus_verify)] if experiment == 'e7' else [
        ('node-measurement', measure_verify), ('lifetime', lifetime_verify), ('rust-corpus', corpus_verify)]
    metrics = {}
    reports = {}
    for name, replay in captures:
        path = ROOT / 'target/s10' / name
        if not (path / 'capture.json').exists():
            print('S10 capture unavailable: ' + str(path), file=sys.stderr)
            continue
        report = replay(path)
        reports[name] = report
        metrics.update(report['metrics'])
        print(name + ' capture_sha256=' + report['capture_sha256'], file=sys.stderr)
    if experiment == 'e7':
        # The small public-API test alone cannot certify the full checker host.
        corpus = reports.get('wasm-corpus')
        if corpus is None or corpus['partial']:
            metrics.pop('portable_host', None)
        elif 'portable_host' in metrics:
            metrics['portable_host'] = metrics['portable_host'] and not any(
                counts.get('failed', 0) for counts in corpus['counts']['acceptance'].values())
    return {'metrics': metrics}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('experiment', choices=['e7', 'e8'])
    print(json.dumps(producer(parser.parse_args().experiment)))
