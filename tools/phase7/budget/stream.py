#!/usr/bin/env python3
"""Replay archived metadata and discovered contents with bounded preparation.

This is a correctness capture. Transport intervals and whole-process peaks
are diagnostic measurements, not Phase 7 acceptance timings or RSS budgets.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import os
from pathlib import Path
import stat
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import probe
import snapshot

MAX_FRAME = 2 * 1024 * 1024
MAX_METADATA = 1024 * 1024
CHUNK = 1024 * 1024


def open_blob(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise ValueError('frozen blob is not a regular file')
        return os.fdopen(descriptor, 'rb')
    except BaseException:
        os.close(descriptor)
        raise


def source_path(value, root, label):
    probe.byte_string(value, label, nonempty=True)
    path = bytes(value)
    if (b'\0' in path or not os.path.isabs(path) or os.path.normpath(path) != path
            or not snapshot.inside(path, root)):
        raise ValueError(f'{label} must be a canonical path inside the archived scope')


class Archive:
    def __init__(self, inventory, contents, input_path):
        self.inventory, self.contents = inventory, contents
        root, record, self.entries = snapshot.load_inventory(inventory)
        self.metadata_sha = record['metadata_sha256']
        self.contents_sha = snapshot.digest(contents / 'contents.ndjson')
        self.receipt_sha = snapshot.digest(contents / 'complete.json')
        receipt = probe.decode((contents / 'complete.json').read_bytes())
        if (receipt.get('version') != 1 or receipt.get('kind') != 'frozen_contents'
                or receipt.get('metadata_sha256') != self.metadata_sha
                or receipt.get('contents_sha256') != self.contents_sha):
            raise ValueError('frozen contents do not match their metadata and receipt')
        self.input_bytes = input_path.read_bytes()
        self.input = probe.decode(self.input_bytes)
        if self.input.get('entries') != []:
            raise ValueError('streamed input must have an empty entries array')
        source_path(self.input['current_directory'], root, 'current_directory')
        source_path(self.input['configuration']['config_path'], root, 'config_path')
        if self.input.get('operation') not in ('check', 'check_emit'):
            raise ValueError('unsupported operation')
        self.files = sum(row['kind'] == 'physical_file' for row in self.entries.values())
        self.content_rows = []
        seen = set()
        total = 0
        verified = set()
        for row in snapshot.rows(contents / 'contents.ndjson'):
            probe.fields(row, 'path_b64 raw_size sha256', 'frozen content')
            path = snapshot.unb64(row['path_b64'])
            entry = self.entries.get(path)
            probe.decimal(row['raw_size'], 'content size')
            sha = row['sha256']
            if (path in seen or entry is None or entry['kind'] != 'physical_file'
                    or entry['raw_size'] != row['raw_size'] or not isinstance(sha, str)
                    or len(sha) != 64 or any(c not in '0123456789abcdef' for c in sha)):
                raise ValueError('invalid, duplicate or non-metadata content identity')
            seen.add(path)
            blob = contents / 'blobs' / sha
            with open_blob(blob) as source:
                if os.fstat(source.fileno()).st_size != int(row['raw_size']):
                    raise ValueError('frozen blob has the wrong size')
                if sha not in verified and hashlib.file_digest(source, 'sha256').hexdigest() != sha:
                    raise ValueError('frozen blob content digest differs')
            verified.add(sha)
            total += int(row['raw_size'])
            self.content_rows.append(row)
        if (type(receipt.get('files')) is not int or receipt['files'] != len(self.content_rows)
                or type(receipt.get('raw_bytes')) is not int or receipt['raw_bytes'] != total):
            raise ValueError('frozen content counts differ from their receipt')
        self.identity = {'metadata': str(inventory.resolve()), 'metadata_sha256': self.metadata_sha,
                         'contents': str(contents.resolve()), 'contents_sha256': self.contents_sha,
                         'receipt_sha256': self.receipt_sha,
                         'input_sha256': hashlib.sha256(self.input_bytes).hexdigest(),
                         'entries': len(self.entries), 'physical_files': self.files,
                         'provided_files': len(self.content_rows), 'provided_raw_bytes': total}

    def unchanged(self):
        for path, expected in [(self.inventory / 'metadata.ndjson', self.metadata_sha),
                               (self.contents / 'contents.ndjson', self.contents_sha),
                               (self.contents / 'complete.json', self.receipt_sha)]:
            if snapshot.digest(path) != expected:
                raise ValueError('snapshot archive changed during replay')

    def preparation(self):
        def command(name, expected, **fields):
            value = dict(version=2, command=name, id='0', **fields)
            if len(probe.encoded(value)) > MAX_FRAME:
                raise ValueError('stream preparation exceeds its frame limit')
            return value, expected

        self.unchanged()
        yield command('prepare_begin', 'preparing', input=self.input,
                      expected_entries=str(len(self.entries)), expected_files=str(self.files),
                      expected_contents=str(len(self.content_rows)))
        batch, size = [], 2
        for row in self.entries.values():
            length = len(probe.encoded(row)) - 1
            if length + 2 > MAX_METADATA:
                raise ValueError('single metadata entry exceeds frame budget')
            if size + length + bool(batch) > MAX_METADATA:
                yield command('prepare_metadata', 'metadata', entries=batch)
                batch, size = [], 2
            size += length + bool(batch)
            batch.append(row)
        if batch:
            yield command('prepare_metadata', 'metadata', entries=batch)
        for row in self.content_rows:
            yield command('content_begin', 'content_started', **row)
            offset, digest = 0, hashlib.sha256()
            with open_blob(self.contents / 'blobs' / row['sha256']) as source:
                while chunk := source.read(CHUNK):
                    yield command('content_chunk', 'content_chunk', offset=str(offset),
                                  bytes_b64=base64.b64encode(chunk).decode('ascii'))
                    digest.update(chunk)
                    offset += len(chunk)
            if str(offset) != row['raw_size'] or digest.hexdigest() != row['sha256']:
                raise ValueError('frozen blob changed during streaming')
            yield command('content_end', 'content_verified')
        self.unchanged()
        yield command('prepare_end', 'prepared')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--go', type=Path, required=True)
    parser.add_argument('--rust', type=Path, required=True)
    parser.add_argument('--inventory', type=Path, required=True)
    parser.add_argument('--contents', type=Path, required=True)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--timeout', type=float, default=300)
    parser.add_argument('--runtime', choices=['native', 'wasm'], default='native')
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() or 'target' in output.parts or args.timeout <= 0:
        parser.error('require new output outside target and a positive command timeout')
    archive = Archive(args.inventory, args.contents, args.input)
    output.mkdir(parents=True)
    (output / 'input.json').write_bytes(archive.input_bytes)
    record = {'schema': 1, 'complete': False, 'performance_acceptance': False,
              'snapshot': archive.identity, 'protocol_version': 2, 'runtime': args.runtime}
    try:
        results = {name: probe.capture(binary.resolve(strict=True), archive.input, output / name,
                                       args.timeout, backend=name if args.runtime == 'native' else name + '-wasm',
                                       preparation=archive.preparation(),
                                       protocol_version=2)
                   for name, binary in [('go', args.go), ('rust', args.rust)]}
        archive.unchanged()
        differences = probe.comparison(results['go']['observation'], results['rust']['observation'])
        record.update(complete=True, differences=differences,
                      types={name: result['observation']['types'] for name, result in results.items()})
        probe.write(output / 'comparison.json', record)
        return int(bool(differences))
    except Exception as error:
        record['error'] = str(error)
        probe.write(output / 'comparison.json', record)
        print(error, file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
