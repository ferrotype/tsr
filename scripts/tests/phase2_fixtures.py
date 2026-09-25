"""Small native/Rust C0 recordings for tests; never acceptance evidence."""
import copy
import gzip
import json
from pathlib import Path

import s08_p4 as p4
from s08_oracle import digest

FIXTURE = Path(__file__).parent / 'fixtures/phase2/corpus.json.gz'
FIXTURE_SHA256 = '23e1061648f441de7278fb70fb87d8add4f5d1ffda7b42741823b0fea2dc5bd3'
CONTROL = 'compiler/2dArrays.ts#configuration=0'
PANIC = 'compiler/sliceTupleTypeOutOfBounds.ts#configuration=0'
MAPPER = 'compiler/contentMapperInvalidExtension.ts#configuration=0'
S08_CONTROLS = (
    CONTROL,
    'compiler/ClassDeclaration14.ts#configuration=0',
    'compiler/accessorDeclarationEmitVisibilityErrors.ts#configuration=0',
    'compiler/unusedLocalsAndParameters.ts#configuration=0',
    'conformance/salsa/moduleExportAlias.ts#configuration=0',
)


def load():
    raw = FIXTURE.read_bytes()
    assert digest(raw) == FIXTURE_SHA256, 'C0 regression fixture changed'
    document = json.loads(gzip.decompress(raw))
    for record in document['records'].values():
        assert record['envelope']['request_sha256'] == digest(p4.canonical(record['request']) + b'\n')
        for name, value in record['artifacts'].items():
            assert record['envelope']['artifacts'][name] == digest(bytes.fromhex(value))
    return document


def build_capture(directory, ids, *, selection=None):
    """Rebind authentic rows to a test-only miniature capture, without a compiler.

    The executable is inert data and sources deliberately contain a fixture-only
    entry: this capture cannot pass the real producer's source check.
    """
    document = load()
    records = document['records']
    requests = [records[vid]['request'] for vid in ids]
    (directory / 'source-snapshot').mkdir(exist_ok=True)
    source = b'Test fixture only. Not a compiler source snapshot.\n'
    (directory / 'source-snapshot/fixture.txt').write_bytes(source)
    binary = b'Test fixture only. Not an executable.\n'
    (directory / 'executable').write_bytes(binary)
    selected = selection if selection is not None else {'sample': False, 'cases': list(ids), 'limit': None}
    metadata = {'version': 1, 'requests_sha256': digest(p4.canonical(requests) + b'\n'),
                'timeout_seconds': 60, 'selection': selected,
                'partial': selected != {'sample': False, 'cases': [], 'limit': None},
                'inventory_sha256': digest((p4.ROOT / 'data/phase2/inventory.json').read_bytes()),
                'native': {'directory': '/fixture-only', 'report_sha256': '0' * 64,
                           'observation_sha256': document['provenance']['native_observation_sha256']},
                'build': {'binary_sha256': digest(binary), 'sources': {'fixture.txt': digest(source)}}}
    p4.atomic(directory / 'capture.json', metadata)
    p4.atomic(directory / 'requests.json', requests)
    (directory / 'cases').mkdir()
    for i, vid in enumerate(ids):
        record = records[vid]
        target = directory / 'cases' / f'{i:05d}'
        target.mkdir()
        for name, raw in record['artifacts'].items():
            (target / name).write_bytes(bytes.fromhex(raw))
        envelope = copy.deepcopy(record['envelope'])
        envelope['capture_sha256'] = digest(p4.canonical(metadata) + b'\n')
        p4.atomic(target / 'result.json', envelope)
    return metadata
