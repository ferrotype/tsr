"""Bounded scanner/parser targets using existing S05/S06 request contracts.

No oracle builds, corpus freezing, expected-panic qualification or compiler
fallback occurs here. The entire validated ordered stream is compared exactly.
"""
from __future__ import annotations

import copy
import io
import math
from pathlib import Path
import sys
import tempfile
import time

from tools.phase7.fuzz import run as fuzz

sys.path.insert(0, str(fuzz.ROOT / 'scripts'))
from s04_common import strict_json_loads  # noqa: E402
import s05_cases  # noqa: E402
import s05_protocol  # noqa: E402
import s06_protocol  # noqa: E402

MAX_SOURCE = 4096
MAX_OUTPUT = 16 * 1024 * 1024
LIMITS = {'source_bytes': MAX_SOURCE, 'channel_bytes': MAX_OUTPUT, 'mechanism': 'RLIMIT_FSIZE'}


def request_for(target, source):
    if len(source) > MAX_SOURCE:
        raise ValueError(f'protocol source exceeds {MAX_SOURCE} bytes')
    if target == 'scanner':
        return s05_cases.case('r5/scanner', source, target=99)['request']
    if target == 'parser':
        return s06_protocol.parser_request('r5/parser', None, source,
                                           '/s06/fixture.ts', '/s06/fixture.ts')
    raise ValueError('expected named scanner or parser target')


def input_files(target, source):
    return {'main.ts': source,
            'request.ndjson': s05_cases.canonical(request_for(target, source)) + b'\n'}


def validate_input(target, files):
    if set(files) != {'main.ts', 'request.ndjson'}:
        raise ValueError('unexpected protocol target input inventory')
    request = strict_json_loads(files['request.ndjson'])
    # The named targets have one fixed operation/options schedule. Archive
    # replay must not silently reinterpret arbitrary manual scan actions.
    if request != request_for(target, files['main.ts']):
        raise ValueError('archived request differs from the named target/source')
    return request


def validate_diagnostic(value, *, top=False):
    fields = {'code', 'category', 'pos', 'end', 'file_hex', 'key_hex', 'text_hex', 'args_hex', 'related'}
    if top:
        fields |= {'collection', 'index'}
    if type(value) is not dict or set(value) != fields:
        raise ValueError('invalid fuzz diagnostic fields')
    for name in ('code', 'category', 'pos', 'end'):
        if type(value[name]) is not int:
            raise ValueError('invalid fuzz diagnostic integer')
    for name in ('file_hex', 'key_hex', 'text_hex'):
        if name == 'file_hex' and value[name] is None:
            continue
        s06_protocol.hex_bytes(value[name], name)
    if type(value['args_hex']) is not list or type(value['related']) is not list:
        raise ValueError('invalid fuzz diagnostic arrays')
    for arg in value['args_hex']:
        s06_protocol.hex_bytes(arg, 'argument')
    for related in value['related']:
        validate_diagnostic(related)


def arguments_for(target):
    return ["--fuzz-related"] if target == "parser" else []


def validate_output(target, request, data, runtime, *, related=True):
    """Reuse framing/schema validators; keep every raw diagnostic and panic."""
    if not data or not data.endswith(b'\n'):
        raise ValueError('missing or truncated adapter stream')
    state = (s05_protocol.StreamCase(request, 'oracle' if runtime == 'go' else 'rust')
             if target == 'scanner' else s06_protocol.StreamCase(request))
    canonical = bytearray()
    began = ended = library_failure = False
    for line in io.BytesIO(data):
        if len(line) > (s05_protocol.MAX_RECORD if target == 'scanner' else s06_protocol.MAX_RESPONSE_BYTES):
            raise ValueError('oversized adapter frame')
        record = strict_json_loads(line)
        if not isinstance(record, dict):
            raise ValueError('adapter frame must be an object')
        if target == 'scanner':
            if ended:
                raise ValueError('trailing adapter frame')
            if not began:
                state.begin(record)
                began = True
            elif isinstance(record, dict) and record.get('event') == 'end':
                state.end(record)
                ended = True
            else:
                state.observation(record)
                library_failure |= record['status'] == 'panic'
        else:
            validated = record
            if related and record.get('tag') == 'observation' and record.get('kind') == 'diagnostic':
                validate_diagnostic(record['value'], top=True)
                validated = copy.deepcopy(record)
                del validated['value']['related']
                del validated['value']['file_hex']
            state.accept(validated)
            ended = state.ended
            library_failure |= record.get('tag') == 'stage' and record.get('outcome') != 'ok'
        canonical.extend(s05_cases.canonical(record) + b'\n')
    if not ended:
        raise ValueError('incomplete adapter stream')
    return bytes(canonical), library_failure


def compare_case(target, source, binaries, timeout, memory, deadline=math.inf, *, files=None, arguments=None):
    if deadline - time.monotonic() < timeout:
        raise TimeoutError('insufficient budget for a complete paired case')
    files = input_files(target, source) if files is None else files
    request = validate_input(target, files)
    arguments = arguments_for(target) if arguments is None else arguments
    if arguments not in ([], arguments_for(target)):
        raise ValueError('unexpected named protocol arguments')
    observations, artifacts, failures = {}, {}, []
    case_deadline = min(deadline, time.monotonic() + timeout)
    with tempfile.TemporaryDirectory(prefix='phase7-fuzz-') as temporary:
        for runtime in ('go', 'rust'):
            directory = (Path(temporary) / runtime).resolve()
            directory.mkdir()
            for name, data in files.items():
                (directory / name).write_bytes(data)
            remaining = min(timeout / 2, case_deadline - time.monotonic())
            if remaining <= 0:
                raise TimeoutError('run budget exhausted before paired adapter')
            result, stdout, stderr = fuzz.run_child(
                binaries[runtime], directory, remaining, memory, arguments,
                stdin_bytes=files['request.ndjson'], max_output_bytes=MAX_OUTPUT)
            comparable = fuzz.normalize(stdout, directory)
            if result.get('execution_error'):
                failures.append(runtime + '-execution-failure')
            elif result['timeout']:
                failures.append(runtime + '-timeout')
            elif result['exit'] != 0:
                failures.append(runtime + '-crash')
            else:
                try:
                    comparable, library_failure = validate_output(target, request, stdout, runtime, related=bool(arguments))
                    if library_failure:
                        failures.append(runtime + '-library-failure')
                except (ValueError, TypeError, KeyError, UnicodeError) as error:
                    result['protocol_error'] = f'{type(error).__name__}: {error}'
                    failures.append(runtime + '-protocol-failure')
            result.update(stdout_hex=comparable.hex(), stderr_hex=fuzz.normalize(stderr, directory).hex(), emitted={})
            observations[runtime] = result
            artifacts[runtime] = {'stdout': stdout, 'stderr': stderr, 'emitted': {}}
    for field in ('exit', 'stdout_hex', 'stderr_hex'):
        if observations['go'][field] != observations['rust'][field]:
            failures.append(field)
    return fuzz.Pair(observations, artifacts, failures)
