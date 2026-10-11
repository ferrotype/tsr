#!/usr/bin/env python3
"""Bounded native consumer correctness probe; these are not acceptance timings.

A child retains compiler roots through checkpoint, then produces observations.
The recorded command intervals include pipe transport and are diagnostic only.
The process rusage peak includes preparation and observation serialization.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import itertools
import json
import math
import os
from pathlib import Path
import queue
import signal
import subprocess
import sys
import threading
import time

MAX_FRAME = 256 * 1024 * 1024
MAX_OBSERVATION_ROWS = 1_000_000


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'duplicate field {key}')
        result[key] = value
    return result


def finite_float(value):
    number = float(value)
    if not math.isfinite(number):
        raise ValueError(f'invalid number {value}')
    return number


def decode(raw):
    return json.loads(raw, object_pairs_hook=unique,
                      parse_float=finite_float,
                      parse_constant=lambda value: (_ for _ in ()).throw(ValueError(f'invalid number {value}')))


def encoded(value):
    # Preserve maps such as paths: the protocol must not sort nested keys.
    return json.dumps(value, separators=(',', ':'), allow_nan=False).encode() + b'\n'


def identity(path):
    path = Path(path).resolve(strict=True)
    with path.open('rb') as source:
        sha = hashlib.file_digest(source, 'sha256').hexdigest()
    return {'path': str(path), 'bytes': path.stat().st_size, 'sha256': sha}


def write(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + '\n')


def resident(pid):
    return int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(pid)],
                                      text=True, timeout=10).strip()) * 1024


class Child:
    def __init__(self, binary, directory, timeout, archive_responses=False):
        self.directory, self.timeout = directory, timeout
        self.log = (directory / 'stderr').open('xb')
        self.process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.log, start_new_session=True)
        self.responses = queue.Queue()
        self.commands = []
        self.reaped = False
        self.response_archive = gzip.open(directory / 'responses.ndjson.gz', 'xb', compresslevel=1) if archive_responses else None
        self.observation_digest = None

        def reader():
            try:
                while True:
                    row = self.process.stdout.readline(MAX_FRAME + 1)
                    if not row:
                        self.responses.put(EOFError('consumer ended before a response'))
                        return
                    if len(row) > MAX_FRAME or not row.endswith(b'\n'):
                        self.responses.put(ValueError('unterminated or oversized response'))
                        return
                    self.responses.put(row)
            except Exception as error:
                self.responses.put(error)

        self.reader = threading.Thread(target=reader, daemon=True)
        self.reader.start()

    def exchange(self, request, expected):
        raw = encoded(request)
        if len(raw) > MAX_FRAME:
            raise ValueError('request exceeds the tiny-probe transport limit')
        index = len(self.commands)
        entry = {'command': request['command'], 'request_sha256': hashlib.sha256(raw).hexdigest()}
        self.commands.append(entry)
        start = time.monotonic_ns()
        # Writing a large prepare frame can block too; it shares this command's deadline.
        sent = queue.Queue()

        def writer():
            try:
                self.process.stdin.write(raw)
                self.process.stdin.flush()
                sent.put(None)
            except Exception as error:
                sent.put(error)

        threading.Thread(target=writer, daemon=True).start()
        try:
            error = sent.get(timeout=self.timeout)
            if error is not None:
                raise error
            remaining = self.timeout - (time.monotonic_ns() - start) / 1e9
            if remaining <= 0:
                raise queue.Empty
            response = self.responses.get(timeout=remaining)
        except queue.Empty as error:
            raise TimeoutError(f'{request["command"]} exceeded {self.timeout}s') from error
        entry['round_trip_ns'] = time.monotonic_ns() - start
        if isinstance(response, Exception):
            raise response
        if self.response_archive is None:
            (self.directory / f'{index:02d}-{request["command"]}.json').write_bytes(response)
        else:
            self.response_archive.write(response)
        if self.observation_digest is not None:
            self.observation_digest.update(response)
        value = decode(response)
        if (not isinstance(value, dict) or set(value) != {'version', 'id', 'kind', 'value'} or type(value['version']) is not int
                or value['version'] != request['version'] or value['id'] != request['id'] or value['kind'] != expected):
            raise ValueError(f'{request["command"]} returned an unexpected response: {value}')
        if expected in ('preparing', 'metadata', 'content_started', 'content_chunk',
                        'content_verified', 'prepared', 'executed', 'released') and value['value'] is not None:
            raise ValueError(f'{request["command"]} acknowledgement must have a null value')
        if expected == 'observation':
            self.observation_record = {
                'file': 'responses.ndjson.gz' if self.response_archive is not None else f'{index:02d}-{request["command"]}.json',
                'row': index if self.response_archive is not None else 0,
                'response_sha256': hashlib.sha256(response).hexdigest(),
                'response_bytes': len(response),
            }
        return value['value']

    def finish(self):
        self.process.stdin.close()
        # A separate thread allows a bounded wait4 without Popen.poll reaping first.
        finished = queue.Queue()
        threading.Thread(target=lambda: finished.put(os.wait4(self.process.pid, 0)), daemon=True).start()
        try:
            _, status, usage = finished.get(timeout=self.timeout)
        except queue.Empty as error:
            os.killpg(self.process.pid, signal.SIGKILL)
            _, status, usage = finished.get(timeout=10)
            self.reaped = True
            self.process.returncode = os.waitstatus_to_exitcode(status)
            raise TimeoutError('consumer did not exit after EOF') from error
        self.reaped = True
        self.process.returncode = os.waitstatus_to_exitcode(status)
        self.reader.join(timeout=5)
        extra = self.responses.get(timeout=5)
        if not isinstance(extra, EOFError):
            raise ValueError('consumer wrote an extra response after release')
        if self.process.returncode:
            raise ValueError(f'consumer exited {self.process.returncode}')
        return {'exit': self.process.returncode,
                'entire_process_peak_rss_bytes': usage.ru_maxrss * (1024 if sys.platform.startswith('linux') else 1),
                'entire_process_user_seconds': usage.ru_utime,
                'entire_process_system_seconds': usage.ru_stime}

    def close(self):
        # Reap an already-exited leader before signalling its group. Darwin
        # can report EPERM for a group whose only member is a zombie.
        if not self.reaped and self.process.poll() is not None:
            self.reaped = True
        # These consumers do not spawn subprocesses. An exited leader needs
        # no group signal; Darwin can refuse one against its former group.
        if not self.reaped:
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError:
                # The leader may exit between poll and killpg. Otherwise use
                # the direct child handle, retaining an actual failure to kill.
                if self.process.poll() is None:
                    self.process.kill()
        if not self.reaped:
            self.process.wait(timeout=10)
        self.reader.join(timeout=5)
        for stream in (self.process.stdin, self.process.stdout, self.log, self.response_archive):
            if stream is None:
                continue
            try:
                stream.close()
            except BrokenPipeError:
                pass


def fields(value, names, label):
    if not isinstance(value, dict) or set(value) != set(names.split()):
        raise ValueError(f'{label} has missing, extra, or invalid fields')


def integer(value, low, high, label):
    if type(value) is not int or not low <= value <= high:
        raise ValueError(f'{label} must be an integer in [{low},{high}]')


def decimal(value, label, signed=False, positive=False):
    if not isinstance(value, str):
        raise ValueError(f'{label} must be a canonical decimal string')
    digits = value[1:] if signed and value.startswith('-') else value
    if not digits.isascii() or not digits.isdecimal():
        raise ValueError(f'{label} must be a canonical decimal string')
    number = int(value)
    low, high = (-(1 << 63), (1 << 63) - 1) if signed else (0, (1 << 64) - 1)
    if str(number) != value or not low <= number <= high or positive and number <= 0:
        raise ValueError(f'{label} must be a {"positive " if positive else ""}canonical decimal count')


def byte_string(value, label, nonempty=False):
    # Only the host's validated paged reader creates bytes; JSON decoding
    # cannot manufacture this representation or bypass per-byte validation.
    if isinstance(value, bytes) and (value or not nonempty):
        return
    if not isinstance(value, list) or nonempty and not value:
        raise ValueError(f'{label} must be {"a nonempty" if nonempty else "an"} integer byte array')
    for item in value:
        integer(item, 0, 255, label)


def array(value, label):
    if not isinstance(value, list):
        raise ValueError(f'{label} must be an array')


def validate_checkpoint(value, backend):
    peak_fields = 'process_peak_rss_before_observation_bytes process_peak_rss_authority'
    has_peak = isinstance(value, dict) and any(key in value for key in peak_fields.split())
    if backend in ('go', 'rust') and has_peak:
        if value.get('process_peak_rss_authority') != 'getrusage(RUSAGE_SELF).ru_maxrss':
            raise ValueError('unknown native process peak RSS authority')
        decimal(value.get('process_peak_rss_before_observation_bytes'), 'native process peak RSS', positive=True)
    if backend == 'go':
        names = 'heap_alloc heap_inuse heap_sys stack_inuse stack_sys sys total_alloc mallocs frees num_gc'
        if has_peak:
            names += ' ' + peak_fields
        fields(value, names, 'Go checkpoint')
        for name in names.split():
            if name != 'process_peak_rss_authority':
                decimal(value[name], f'Go checkpoint {name}')
    elif backend == 'rust':
        if not isinstance(value, dict) or value.get('allocation_instrumented') not in ('true', 'false'):
            raise ValueError('Rust checkpoint must declare allocation instrumentation')
        names = 'authority allocation_instrumented tracked_owners tracked_allocations'
        if has_peak:
            names += ' ' + peak_fields
        if 'allocator' in value:
            names += ' allocator'
            if value['allocator'] != 'mimalloc-0.1.48':
                raise ValueError('Rust checkpoint returned an unknown allocator')
        metrics = 'live_requested_bytes total_requested_bytes peak_requested_bytes allocation_events deallocation_events'
        if value['allocation_instrumented'] == 'true':
            names += ' ' + metrics
        fields(value, names, 'Rust checkpoint')
        if value['authority'] != 'rust_storage_owners_not_heap_bytes':
            raise ValueError('Rust checkpoint returned an unknown metric authority')
        for name in names.split():
            if name not in ('authority', 'allocation_instrumented', 'allocator', 'process_peak_rss_authority'):
                decimal(value[name], f'Rust checkpoint {name}')
    elif backend in ('go-wasm', 'rust-wasm'):
        names = 'authority linear_memory_bytes process_rss_bytes v8_heap_used_bytes external_bytes array_buffer_bytes'
        expected = 'go-wasm-node' if backend == 'go-wasm' else 'production-wasm-node'
        if backend == 'go-wasm':
            names += ' go_runtime'
        if isinstance(value, dict) and ('process_peak_rss_bytes' in value or 'gc_policy' in value):
            names += ' process_peak_rss_bytes gc_policy'
            if value.get('gc_policy') != 'v8_after_prepare_and_at_checkpoint':
                raise ValueError('unknown Node collection policy')
            decimal(value.get('process_peak_rss_bytes'), 'Node process peak RSS', positive=True)
        fields(value, names, 'wasm checkpoint')
        if value['authority'] != expected:
            raise ValueError('wasm checkpoint returned the wrong authority')
        for name in names.split():
            if name not in ('authority', 'go_runtime', 'gc_policy'):
                decimal(value[name], name, positive=name in ('linear_memory_bytes', 'process_rss_bytes'))
        if backend == 'go-wasm':
            validate_checkpoint(value['go_runtime'], 'go')
    else:
        raise ValueError(f'unknown checkpoint backend {backend}')


def validate_observation(observation, operation=None):
    fields(observation, 'outcome roots options_json sources diagnostics checkers types nodes identifiers writes emitted_files emit_skipped emit_diagnostics', 'observation')
    if observation['outcome'] != 'compiled':
        raise ValueError('config failure is not a completed compilation')
    if observation['checkers'] != '1':
        raise ValueError('consumer must observe exactly one checker')
    for key in ('types', 'nodes'):
        decimal(observation[key], key, positive=True)
    decimal(observation['identifiers'], 'identifiers')
    for key in ('roots', 'sources', 'diagnostics', 'writes', 'emitted_files', 'emit_diagnostics'):
        array(observation[key], key)
    if not observation['sources'] or not observation['roots']:
        raise ValueError('empty source/root observation cannot pass')
    for root in observation['roots']:
        byte_string(root, 'root', nonempty=True)
    for source in observation['sources']:
        fields(source, 'name path text default_library node_count identifier_count', 'source')
        for key in ('name', 'path', 'text'):
            byte_string(source[key], f'source {key}', nonempty=key != 'text')
        if type(source['default_library']) is not bool:
            raise ValueError('source default_library must be a boolean')
        for key in ('node_count', 'identifier_count'):
            decimal(source[key], f'source {key}')
    byte_string(observation['options_json'], 'options_json', nonempty=True)
    options = decode(bytes(observation['options_json']))
    if not isinstance(options, dict):
        raise ValueError('options_json must encode an object')

    def diagnostic(value):
        fields(value, 'file pos end code category message_key message_text message_args localized chain related source unnecessary deprecated skipped_on_no_emit repopulate', 'diagnostic')
        if value['file'] is not None:
            integer(value['file'], 0, len(observation['sources']) - 1, 'diagnostic file index')
        for key in ('pos', 'end'):
            decimal(value[key], f'diagnostic {key}', signed=True)
        integer(value['code'], -(1 << 31), (1 << 31) - 1, 'diagnostic code')
        integer(value['category'], 0, 3, 'diagnostic category')
        for key in ('message_key', 'message_text', 'localized', 'source'):
            byte_string(value[key], f'diagnostic {key}')
        array(value['message_args'], 'diagnostic message_args')
        for argument in value['message_args']:
            byte_string(argument, 'diagnostic message argument')
        for key in ('unnecessary', 'deprecated', 'skipped_on_no_emit'):
            if type(value[key]) is not bool:
                raise ValueError(f'diagnostic {key} must be a boolean')
        for key in ('chain', 'related'):
            array(value[key], f'diagnostic {key}')
            for child in value[key]:
                diagnostic(child)
        repopulate = value['repopulate']
        if repopulate is not None:
            fields(repopulate, 'kind module_reference mode package_name', 'diagnostic repopulate')
            for key in ('kind', 'mode'):
                integer(repopulate[key], -(1 << 31), (1 << 31) - 1, f'repopulate {key}')
            for key in ('module_reference', 'package_name'):
                byte_string(repopulate[key], f'repopulate {key}')

    for key in ('diagnostics', 'emit_diagnostics'):
        for value in observation[key]:
            diagnostic(value)
    for value in observation['writes']:
        fields(value, 'name text', 'write')
        byte_string(value['name'], 'write name', nonempty=True)
        byte_string(value['text'], 'write text')
    for name in observation['emitted_files']:
        byte_string(name, 'emitted file name', nonempty=True)
    if observation['emit_skipped'] is not None and type(observation['emit_skipped']) is not bool:
        raise ValueError('emit_skipped must be a boolean or null')
    if operation == 'check' and (observation['emit_skipped'] is not None or observation['writes'] or observation['emitted_files'] or observation['emit_diagnostics']):
        raise ValueError('check-only operation returned emit work')
    if operation == 'check_emit' and type(observation['emit_skipped']) is not bool:
        raise ValueError('check_emit operation omitted its emit result')


def comparison(go, rust):
    """Observations retain exact source, diagnostic and output bytes.

    Type creation is implementation-specific and remains a separate count.
    All other work fields must match. Options object field order is immaterial,
    except the ordered paths map whose iteration affects resolution.
    """
    def option_value(value, ordered=False):
        # Python equates True with 1. Preserve JSON kinds before comparing
        # options so a boolean/numeric serialization error cannot pass.
        if isinstance(value, dict):
            items = value.items() if ordered else sorted(value.items())
            return ('object', tuple((key, option_value(item)) for key, item in items))
        if isinstance(value, list):
            return ('array', tuple(option_value(item) for item in value))
        if type(value) is bool:
            return ('boolean', value)
        if value is None:
            return ('null',)
        return ('number' if isinstance(value, (int, float)) else 'string', value)

    def normalized(value):
        value = dict(value)
        value.pop('types', None)
        # Paged observation compacts text after validating every wire byte.
        # Historical whole-record captures remain readable without copying
        # millions of Python integer references for current captures.
        for key in ('sources', 'writes'):
            value[key] = [dict(row, text=bytes(row['text'])) for row in value[key]]
        options = decode(bytes(value.pop('options_json')))
        value['options'] = tuple((key, option_value(item, ordered=key == 'paths'))
                                 for key, item in sorted(options.items()))
        return value
    go, rust = normalized(go), normalized(rust)
    return [key for key in sorted(go.keys() | rust.keys()) if go.get(key) != rust.get(key)]


def read_observation(directory):
    """Read and authenticate the saved response without duplicating its bytes."""
    record = decode((directory / 'record.json').read_bytes())
    if not record.get('complete'):
        raise ValueError('cannot compare an incomplete consumer capture')
    if 'observation' in record:
        # Historical tiny prototypes stored both the raw response and this copy.
        return record['observation']
    pointer = record['observation_record']
    if pointer.get('format') == 'rows-v2':
        return read_observation_rows(directory, record, pointer)
    filename = pointer['file']
    if Path(filename).name != filename:
        raise ValueError('observation response must belong to its capture directory')
    integer(pointer['row'], 0, len(record['commands']) - 1, 'observation response row')
    with response_file(directory / filename) as source:
        for _ in range(pointer['row'] + 1):
            raw = source.readline(MAX_FRAME + 1)
            if not raw.endswith(b'\n') or len(raw) > MAX_FRAME:
                raise ValueError('invalid archived response frame')
    if len(raw) != pointer['response_bytes'] or hashlib.sha256(raw).hexdigest() != pointer['response_sha256']:
        raise ValueError('archived observation response changed')
    response = decode(raw)
    if response.get('kind') != 'observation':
        raise ValueError('archive pointer does not identify an observation')
    return response['value']


def response_file(path):
    return gzip.open(path, 'rb') if path.suffix == '.gz' else path.open('rb')


def observation_header(value):
    """Validate the shape/counts before requesting any row from the child."""
    fields(value, 'outcome roots options_json diagnostics checkers types nodes identifiers emitted_files emit_skipped emit_diagnostics source_count write_count', 'observation header')
    counts = []
    for name in ('source_count', 'write_count'):
        decimal(value[name], name, positive=name == 'source_count')
        count = int(value[name])
        if count > MAX_OBSERVATION_ROWS:
            raise ValueError(f'{name} exceeds the observation row limit')
        counts.append(count)
    result = {key: item for key, item in value.items() if key not in ('source_count', 'write_count')}
    result.update(sources=[], writes=[])
    return result, counts


def compact_observation_row(value, kind):
    if kind == 'source':
        fields(value, 'name path text default_library node_count identifier_count', 'source')
        byte_string(value['path'], 'source path', nonempty=True)
        if type(value['default_library']) is not bool:
            raise ValueError('source default_library must be a boolean')
        for name in ('node_count', 'identifier_count'):
            decimal(value[name], name)
    else:
        fields(value, 'name text', 'write')
    byte_string(value['name'], f'{kind} name', nonempty=True)
    byte_string(value['text'], f'{kind} text')
    return dict(value, text=bytes(value['text']))


def capture_observation_rows(child, identifier):
    first = len(child.commands)
    child.observation_digest = hashlib.sha256()
    header = child.exchange({'version': 2, 'command': 'observe_begin', 'id': identifier}, 'observation_begin')
    value, counts = observation_header(header)
    for kind, count in zip(('source', 'write'), counts):
        for index in range(count):
            row = child.exchange({'version': 2, 'command': 'observe_' + kind,
                                  'id': identifier, 'index': str(index)}, 'observation_' + kind)
            value[kind + 's'].append(compact_observation_row(row, kind))
    child.observation_record = {'format': 'rows-v2', 'file': 'responses.ndjson.gz', 'row': first,
                                'count': len(child.commands) - first,
                                'responses_sha256': child.observation_digest.hexdigest()}
    child.observation_digest = None
    return value


def read_observation_rows(directory, record, pointer):
    if pointer['file'] not in ('responses.ndjson', 'responses.ndjson.gz'):
        raise ValueError('paged observation must belong to its response archive')
    integer(pointer['row'], 0, len(record['commands']) - 1, 'observation response row')
    integer(pointer['count'], 1, len(record['commands']) - pointer['row'], 'observation response count')
    digest = hashlib.sha256()
    with response_file(directory / pointer['file']) as source:
        for _ in range(pointer['row']):
            raw = source.readline(MAX_FRAME + 1)
            if not raw.endswith(b'\n') or len(raw) > MAX_FRAME:
                raise ValueError('invalid archived response frame')

        def read(kind):
            raw = source.readline(MAX_FRAME + 1)
            if not raw.endswith(b'\n') or len(raw) > MAX_FRAME:
                raise ValueError('invalid archived response frame')
            digest.update(raw)
            response = decode(raw)
            fields(response, 'version id kind value', 'observation response')
            if type(response['version']) is not int or response['version'] != 2 or response['id'] != '0' or response['kind'] != kind:
                raise ValueError('unexpected archived observation response')
            return response['value']

        value, counts = observation_header(read('observation_begin'))
        if 1 + sum(counts) != pointer['count']:
            raise ValueError('archived observation row counts differ')
        for kind, count in zip(('source', 'write'), counts):
            for _ in range(count):
                value[kind + 's'].append(compact_observation_row(read('observation_' + kind), kind))
    if digest.hexdigest() != pointer['responses_sha256']:
        raise ValueError('archived observation responses changed')
    validate_observation(value)
    return value


def capture(binary, input_value, output, timeout, backend='go', *, preparation=None, protocol_version=1):
    output.mkdir()
    record = {'schema': 1, 'performance_acceptance': False, 'complete': False,
              'binary': identity(binary), 'timeout_seconds_per_command': timeout,
              'scope': 'correctness probe; intervals include transport; peak includes prepare and observation'}
    write(output / 'record.json', record)
    child = Child(binary, output, timeout, archive_responses=protocol_version == 2)
    try:
        if preparation is None:
            preparation = [({'version': protocol_version, 'command': 'prepare', 'id': '0',
                             'input': input_value}, 'prepared')]
        commands = (({'version': protocol_version, 'command': command, 'id': '0'}, expected)
                    for command, expected in [('execute', 'executed'), ('checkpoint', 'checkpoint'),
                                              ('observe', 'observation'), ('release', 'released')])
        for request, expected in itertools.chain(preparation, commands):
            command = request['command']
            value = capture_observation_rows(child, '0') if command == 'observe' and protocol_version == 2 else child.exchange(request, expected)
            if command == 'checkpoint':
                validate_checkpoint(value, backend)
                record['checkpoint'] = value
                record['retained_process_rss_before_observation'] = resident(child.process.pid)
            elif command == 'observe':
                validate_observation(value, input_value.get('operation'))
                record['observation'] = value
        record.update(child.finish())
        record['complete'] = True
    except Exception as error:
        record['error'] = str(error)
        raise
    finally:
        record['commands'] = child.commands
        try:
            child.close()
        except Exception as error:
            record['complete'] = False
            record['cleanup_error'] = str(error)
            raise
        finally:
            persisted = dict(record)
            if 'observation' in persisted:
                # Source byte arrays are already in the raw response. A second
                # indented copy can multiply a large capture's disk footprint.
                persisted.pop('observation')
                persisted['observation_record'] = child.observation_record
            write(output / 'record.json', persisted)
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--go', type=Path, required=True)
    parser.add_argument('--rust', type=Path, required=True)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--timeout', type=float, default=180)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() or 'target' in output.parts or args.timeout <= 0:
        parser.error('require a new output outside target and a positive timeout')
    raw = args.input.read_bytes()
    if len(raw) > MAX_FRAME:
        parser.error('input exceeds tiny-probe transport limit')
    inputs = decode(raw)
    output.mkdir(parents=True)
    (output / 'input.json').write_bytes(raw)
    record = {'schema': 1, 'performance_acceptance': False, 'complete': False,
              'input_sha256': hashlib.sha256(raw).hexdigest()}
    try:
        results = {name: capture(binary.resolve(strict=True), inputs, output / name, args.timeout, backend=name)
                   for name, binary in [('go', args.go), ('rust', args.rust)]}
        differences = comparison(results['go']['observation'], results['rust']['observation'])
        record.update(complete=True, differences=differences,
                      types={name: value['observation']['types'] for name, value in results.items()})
        write(output / 'comparison.json', record)
        return int(bool(differences))
    except Exception as error:
        record['error'] = str(error)
        write(output / 'comparison.json', record)
        print(error, file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
