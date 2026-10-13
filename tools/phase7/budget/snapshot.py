#!/usr/bin/env python3
"""Archive complete filesystem metadata, then the union of discovered raw reads.

Discovery is untimed and scoped. A replay has no live filesystem fallback.
Unprovided known files remain present in metadata and must fail on ReadFile.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import stat
import sys


def b64(value):
    return base64.b64encode(value).decode('ascii')


def unb64(value):
    if not isinstance(value, str):
        raise ValueError('expected a base64 string')
    raw = base64.b64decode(value, validate=True)
    if b64(raw) != value or b'\0' in raw:
        raise ValueError('noncanonical base64 path or NUL')
    return raw


def unique(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f'duplicate JSON key {key}')
        value[key] = item
    return value


def rows(path):
    with path.open('rb') as source:
        for number, line in enumerate(source, 1):
            if not line.endswith(b'\n') or len(line) > 1024 * 1024:
                raise ValueError(f'{path}:{number}: invalid metadata frame')
            yield json.loads(line, object_pairs_hook=unique)


def encode(row):
    return json.dumps(row, separators=(',', ':'), allow_nan=False).encode() + b'\n'


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def inside(path, root):
    return os.path.commonpath((path, root)) == root


def scan(root):
    """Byte-preserving, deterministic traversal; no extension or directory filter."""
    root = os.path.realpath(os.fsencode(root), strict=True)
    if not os.path.isdir(root):
        raise ValueError('snapshot scope must be a directory')
    yield {'version': 1, 'kind': 'header', 'root_b64': b64(root)}, None
    pending = [root]
    while pending:
        path = pending.pop()
        info = os.lstat(path)
        item = {'path_b64': b64(path)}
        identity = {'path_b64': b64(path), 'device': str(info.st_dev),
                    'inode': str(info.st_ino), 'size': str(info.st_size),
                    'mtime_ns': str(info.st_mtime_ns), 'ctime_ns': str(info.st_ctime_ns),
                    'mode': str(info.st_mode)}
        if stat.S_ISLNK(info.st_mode):
            target = os.readlink(path)
            resolved = os.path.realpath(path)
            if not inside(resolved, root):
                # Container-built archives can retain /work/... links that
                # are genuinely missing on this host. Preserve the missing
                # target, never rewrite it to a different readable file.
                try:
                    os.stat(resolved)
                except (FileNotFoundError, NotADirectoryError):
                    identity['external_target_missing'] = True
                else:
                    raise ValueError(f'symlink leaves snapshot scope: {os.fsdecode(path)}')
            item.update(kind='symlink', target_b64=b64(target))
        elif stat.S_ISDIR(info.st_mode):
            item['kind'] = 'directory'
            with os.scandir(path) as children:
                pending.extend(sorted((child.path for child in children), reverse=True))
        elif stat.S_ISREG(info.st_mode):
            item.update(kind='physical_file', raw_size=str(info.st_size))
        else:
            raise ValueError(f'unsupported snapshot entry: {os.fsdecode(path)}')
        yield item, identity


def inventory(root, output):
    output = output.resolve()
    root = Path(root).resolve(strict=True)
    if output.exists() or output.is_relative_to(root) or 'target' in output.parts:
        raise ValueError('inventory output must be new, outside scope and target')
    output.mkdir(parents=True)
    with (output / 'metadata.ndjson').open('xb') as metadata, (output / 'identity.ndjson').open('xb') as identities:
        count = 0
        for item, identity in scan(root):
            metadata.write(encode(item))
            if identity is not None:
                identities.write(encode(identity))
                count += 1
    record = {'version': 1, 'kind': 'inventory', 'entries': count,
              'metadata_sha256': digest(output / 'metadata.ndjson'),
              'identity_sha256': digest(output / 'identity.ndjson')}
    (output / 'inventory.json').write_bytes(encode(record))
    return record


def load_inventory(directory):
    record = json.loads((directory / 'inventory.json').read_bytes(), object_pairs_hook=unique)
    for name in ('metadata', 'identity'):
        if digest(directory / f'{name}.ndjson') != record[f'{name}_sha256']:
            raise ValueError(f'{name} archive differs from inventory')
    entries = iter(rows(directory / 'metadata.ndjson'))
    header = next(entries)
    if header.get('kind') != 'header' or header.get('version') != 1:
        raise ValueError('invalid snapshot header')
    root = unb64(header['root_b64'])
    by_path = {}
    for entry in entries:
        path = unb64(entry['path_b64'])
        if not inside(path, root) or os.path.normpath(path) != path or path in by_path:
            raise ValueError('duplicate, noncanonical or out-of-scope metadata path')
        by_path[path] = entry
    if len(by_path) != record['entries']:
        raise ValueError('metadata inventory count differs')
    return root, record, by_path


def discovered_reads(logs, entries):
    """Require completed runs, consistent repeats, and metadata-backed reads."""
    selected = {}
    for log in logs:
        count, complete = 0, False
        for row in rows(log):
            if complete or row.get('version') != 1:
                raise ValueError(f'invalid discovery sequence: {log}')
            if row.get('kind') == 'complete':
                if row.get('read_events') != str(count):
                    raise ValueError(f'discovery read count differs: {log}')
                complete = True
                continue
            if row.get('kind') != 'read' or not isinstance(row.get('found'), bool):
                raise ValueError(f'incomplete or failed discovery: {log}')
            unb64(row['path_b64'])
            count += 1
            if not row['found']:
                if any(row.get(key) is not None for key in ('canonical_path_b64', 'raw_size', 'sha256')):
                    raise ValueError('missing read carries content')
                continue
            path = unb64(row['canonical_path_b64'])
            metadata = entries.get(path)
            sha = row['sha256']
            if (metadata is None or metadata['kind'] != 'physical_file'
                    or row['raw_size'] != metadata['raw_size']
                    or not isinstance(sha, str) or len(sha) != 64
                    or any(char not in '0123456789abcdef' for char in sha)):
                raise ValueError('discovery read differs from filesystem inventory')
            content = (sha, row['raw_size'])
            if path in selected and selected[path] != content:
                raise ValueError('discovery reads saw changing file content')
            selected[path] = content
        if not complete:
            raise ValueError(f'discovery did not complete: {log}')
    return selected


def verify_unchanged(root, record):
    metadata, identities = hashlib.sha256(), hashlib.sha256()
    for item, identity in scan(root):
        metadata.update(encode(item))
        if identity is not None:
            identities.update(encode(identity))
    if metadata.hexdigest() != record['metadata_sha256'] or identities.hexdigest() != record['identity_sha256']:
        raise ValueError('snapshot scope changed since inventory')


def open_physical(path, root):
    """Walk physical metadata components without following a replaced symlink."""
    if not inside(path, root) or path == root:
        raise ValueError('expected a contained physical file')
    components = os.path.relpath(path, root).split(b'/')
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for component in components[:-1]:
            next_directory = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                     dir_fd=descriptor)
            os.close(descriptor)
            descriptor = next_directory
        result = os.open(components[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                         dir_fd=descriptor)
        if not stat.S_ISREG(os.fstat(result).st_mode):
            os.close(result)
            raise ValueError('physical file was replaced by a special entry')
        return os.fdopen(result, 'rb')
    finally:
        os.close(descriptor)


def freeze(directory, logs, output):
    output = output.resolve()
    root, record, entries = load_inventory(directory)
    if output.exists() or inside(os.fsencode(output), root) or 'target' in output.parts or not logs:
        raise ValueError('content output must be new and outside scope/target; require discovery logs')
    log_digests = [digest(log) for log in logs]
    selected = discovered_reads(logs, entries)
    verify_unchanged(root, record)
    output.mkdir(parents=True)
    blobs = output / 'blobs'
    blobs.mkdir()
    with (output / 'contents.ndjson').open('xb') as contents:
        for path, (sha, size) in sorted(selected.items()):
            temporary = blobs / 'pending'
            with open_physical(path, root) as source, temporary.open('xb') as destination:
                observed, length = hashlib.sha256(), 0
                while chunk := source.read(1024 * 1024):
                    observed.update(chunk)
                    length += len(chunk)
                    destination.write(chunk)
            if observed.hexdigest() != sha or str(length) != size:
                raise ValueError('raw file changed since discovery')
            final = blobs / sha
            if final.exists():
                temporary.unlink()  # Only our redundant temporary copy.
            else:
                temporary.rename(final)
            contents.write(encode({'path_b64': b64(path), 'sha256': sha, 'raw_size': size}))
    verify_unchanged(root, record)
    if log_digests != [digest(log) for log in logs]:
        raise ValueError('discovery log changed during snapshot construction')
    receipt = {'version': 1, 'kind': 'frozen_contents', 'metadata_sha256': record['metadata_sha256'],
               'files': len(selected), 'raw_bytes': sum(int(size) for _, size in selected.values()),
               'contents_sha256': digest(output / 'contents.ndjson'),
               'discovery': [{'path': str(log.resolve()), 'sha256': sha}
                             for log, sha in zip(logs, log_digests)]}
    (output / 'complete.json').write_bytes(encode(receipt))
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    inv = commands.add_parser('inventory')
    inv.add_argument('--root', type=Path, required=True)
    inv.add_argument('--output', type=Path, required=True)
    content = commands.add_parser('freeze')
    content.add_argument('--inventory', type=Path, required=True)
    content.add_argument('--reads', type=Path, action='append', required=True)
    content.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    try:
        result = inventory(args.root, args.output) if args.command == 'inventory' else freeze(args.inventory, args.reads, args.output)
        print(json.dumps(result))
    except (OSError, ValueError, KeyError, StopIteration) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
