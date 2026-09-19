#!/usr/bin/env python3
"""Compare archived bind-phase CPU ranks; never launch or rebuild a compiler."""
import argparse
from collections import Counter, defaultdict
import gzip
import hashlib
import json
import re
from pathlib import Path

import analyze_pprof as go
import analyze_xctrace as rust

ROOT = Path(__file__).resolve().parents[3]
GO_ARCHIVE = ROOT / 'tools/s07/cpu-profile/results/2026-09-08'
RUST_ARCHIVE = ROOT / 'tools/s07/performance-experiments/results/2026-09-09-post-text-cpu'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def rank(counter, denominator):
    return [{'function': name, 'cpu_ns': value, 'percent_bind_cpu': 100 * value / denominator}
            for name, value in sorted(counter.items(), key=lambda item: (-item[1], item[0]))]


def phase_for_rust(names, worker):
    parse = any(n.startswith('tsr_parser::orchestration::parse_source_file_with_counters') for n in names)
    bind = any(n.startswith('tsr_binder::bind_parsed_file') for n in names)
    if parse and bind:
        raise ValueError('ambiguous parse/bind ancestry')
    if not worker:
        return 'non_worker'
    return 'parse' if parse else 'bind' if bind else 'worker_unassigned'


def summarize(samples):
    """Each sample contributes once to self, and once per distinct inclusive name."""
    total = sum(weight for weight, _, _ in samples)
    own, inclusive, chains, callers = Counter(), Counter(), Counter(), defaultdict(Counter)
    for weight, displayed, _ in samples:
        own[displayed[0] if displayed else '<missing stack>'] += weight
        for name in set(displayed):
            inclusive[name] += weight
        chains[tuple(displayed[:8])] += weight
        if displayed:
            callers[displayed[0]][displayed[1] if len(displayed) > 1 else '<no caller>'] += weight
    assert sum(own.values()) == total
    return {'cpu_ns': total, 'self': rank(own, total), 'inclusive': rank(inclusive, total),
            'self_callers': {name: dict(values.most_common()) for name, values in callers.items()},
            'top_leaf_chains': [{'cpu_ns': weight, 'frames': list(stack)}
                                for stack, weight in chains.most_common(40)]}


# Selectors are operation envelopes, not promises of removable CPU. Physical
# names supplement display names only for inclusive group membership, never self.
SUSPECTS = {
    'stack_guard': lambda n: n.startswith(('stacker::', '<stacker::', 'psm::')) or n.startswith('tsr_binder::recursion::guarded'),
    'contextual_identifier': lambda n: n == '<tsr_binder::state::Binder>::check_contextual_identifier',
    'source_metadata': lambda n: n in {'<tsr_ast::storage::AstView>::source_file', '<tsr_ast::storage::AstView>::file_info'},
    'binding_writes': lambda n: n.startswith(('<tsr_ast::bind_result::BindBuilder>::set_binding_field', '<tsr_ast::compact::binding::BindingWrite>')),
    'hashing': lambda n: 'Hasher' in n or ('RandomState' in n and 'hash_one' in n),
    'name_pool': lambda n: n.startswith('<tsr_ast::symbol_tables::NamePool>'),
    'payload_materialization': lambda n: n == '<tsr_ast::node_read::NodeRead>::data' or (n.startswith('<tsr_ast::') and n.endswith('>::to_owned')),
    'ast_reads': lambda n: n in {'<tsr_ast::storage::AstView>::node', '<tsr_ast::bind_result::BindBuilder>::node', '<tsr_ast::compact::CompactContext>::decode_node'},
    'node_text': lambda n: n.startswith(('<tsr_ast::storage::AstView>::node_text', '<tsr_ast::compact::text::TextPool>')),
    'is_identifier_name': lambda n: n == 'tsr_ast::binder_helpers::is_identifier_name',
    'bind_validation': lambda n: n == '<tsr_ast::bind_result::BindBuilder>::validate',
}


def suspect_groups(samples):
    own, inclusive, leafs = Counter(), Counter(), defaultdict(Counter)
    for weight, displayed, names in samples:
        for group, predicate in SUSPECTS.items():
            if displayed and predicate(displayed[0]):
                own[group] += weight
            if any(predicate(n) for n in names):
                inclusive[group] += weight
                leafs[group][displayed[0] if displayed else '<missing stack>'] += weight
    return {name: {'displayed_self_ns': own[name], 'inclusive_ns': inclusive[name],
                   'leaves': dict(leafs[name].most_common())} for name in SUSPECTS}


def compare(prefix):
    provenance = {}
    gm_raw = (GO_ARCHIVE / 'manifest.json').read_bytes()
    gm = json.loads(gm_raw)
    ga = {item['path']: item for item in gm['artifacts']}
    provenance[str((GO_ARCHIVE / 'manifest.json').relative_to(ROOT))] = sha(gm_raw)

    def go_input(name):
        data = (GO_ARCHIVE / name).read_bytes(); entry = ga[name]
        assert len(data) == entry['stored_bytes'] and sha(data) == entry['stored_sha256']
        content = gzip.decompress(data) if name.endswith('.gz') else data
        assert len(content) == entry['content_bytes'] and sha(content) == entry['content_sha256']
        provenance[str((GO_ARCHIVE / name).relative_to(ROOT))] = sha(data)
        return content

    rm_raw = (RUST_ARCHIVE / 'archive.json').read_bytes(); rm = json.loads(rm_raw)
    provenance[str((RUST_ARCHIVE / 'archive.json').relative_to(ROOT))] = sha(rm_raw)

    def rust_input(suffix):
        path = Path(str(prefix) + suffix)
        data = path.read_bytes(); name = str(path.relative_to(ROOT)); entry = rm['members'][name]
        assert len(data) == entry['bytes'] and sha(data) == entry['sha256']
        provenance[name] = sha(data)
        return data

    work = json.loads(rust_input('.stdout'))
    for name, value in gm['expected_work'].items():
        assert type(work[name]) is type(value) and work[name] == value
    assert work['workers'] == 1 and work['allocated_bytes'] is None
    summary = json.loads(rust_input('.summary.json')); rust_input('.time-profile.xml')
    prior = json.loads(rust_input('-audit.json'))
    xml, _ = rust.xml_document(Path(str(prefix) + '.time-profile.xml'))
    refs = rust.References(xml); frames = {frame['id']: frame for frame in summary['frames']}
    phases, samples, worker_self = Counter(), [], Counter()
    for row in xml.findall('node/row'):
        _, thread, _, _, state, weight, tag = row
        if refs.resolve(state).text != 'Running':
            continue
        ns = int(refs.resolve(weight).text)
        backtrace = refs.resolve(tag).find('backtrace')
        stack = [] if backtrace is None else [refs.resolve(f).get('id') for f in refs.resolve(backtrace)]
        displayed = [frames[i]['name'] for i in stack]
        names = {n for i in stack for n in rust.frame_names(frames[i])}
        phase = phase_for_rust(names, refs.resolve(thread).get('fmt', '').startswith('ts-parser '))
        phases[phase] += ns
        if phase != 'non_worker' and displayed:
            worker_self[displayed[0]] += ns
        if phase == 'bind': samples.append((ns, displayed, names))
    assert sum(phases.values()) == summary['cpu_weight_ns']
    assert phases['bind'] == prior['phase_ns']['bind']
    assert dict(worker_self) == prior['worker_displayed_self_ns']
    rust_result = {**summarize(samples), 'phase_cpu_ns': dict(phases), 'groups': suspect_groups(samples)}
    go_summary = json.loads(go_input('go/summary.json.gz'))
    go_runs, pooled = [], []
    for index in range(3):
        old = next(p for p in go_summary['profiles'] if p['workers'] == 1 and p['index'] == index)
        profile = go_input(f'go/go-1-{index}.pprof')
        assert sha(profile) == old['profile_sha256']
        parsed = go.parse_raw(go_input(f'go/exports/go-1-{index}.pprof.raw.txt.gz').decode())
        assert parsed['period_ns'] == 10_000_000
        full = go.summarize(parsed['samples'])
        assert full['phase_cpu_ns'] == old['phase_cpu_ns'] and full['phase_category_cpu_ns'] == old['phase_category_cpu_ns']
        bind = [s for s in parsed['samples'] if s['phase'] == 'bind']
        selected = [(s['cpu_ns'], [n for n, _ in s['frames']], {n for n, _ in s['frames']}) for s in bind]
        result = summarize(selected); pooled.extend(selected)
        # Check reconstructed flat weights against the archived native pprof
        # display as well as the raw sample parser. Its percentages intentionally
        # differ: the display divides by the whole profile, this report by bind.
        native_top = go_input(f'go/exports/go-1-{index}.pprof.bind-top.txt').decode()
        flat = {row['function']: row['cpu_ns'] for row in result['self']}
        checked_rows = 0
        for line in native_top.splitlines():
            match = re.fullmatch(r'\s*(0|\d+ms)\s+\S+%\s+\S+%\s+(?:0|\d+ms)\s+\S+%\s+(.+?)(?: \(inline\))?', line)
            if match:
                assert flat.get(match[2], 0) == int(match[1].removesuffix('ms')) * 1_000_000
                checked_rows += 1
        assert checked_rows == 30
        result['native_pprof_flat_rows_checked'] = checked_rows
        assert result['cpu_ns'] == full['phase_cpu_ns']['bind']
        go_runs.append({'index': index, **result, 'bind_categories_ns': full['phase_category_cpu_ns']['bind'],
                        'whole_profile_phases_ns': full['phase_cpu_ns']})
    for path in (Path(__file__), ROOT/'tools/s07/cpu-profile/analyze_pprof.py', ROOT/'tools/s07/cpu-profile/analyze_xctrace.py'):
        provenance[str(path.relative_to(ROOT))] = sha(path.read_bytes())
    return {'schema': 1, 'diagnostic_only': True, 'work': work, 'provenance': provenance,
            'rust': rust_result, 'go': {'runs': go_runs, 'pooled_three_runs': summarize(pooled)},
            'limits': ['No new compiler process or timing screen.',
                       'Rust one current normal capture at 1ms; Go three historical pipeline captures at 10ms.',
                       'Rust bind ancestry includes validation/publication; Go phase labels retain systemstack work.',
                       'Unlabeled Go GC is preserved separately, not charged to bind or discarded from the full profile.',
                       'Percentages use each selected bind denominator; pooled Go weights cover three runs, never one.',
                       'Self means displayed Rust leaf / innermost Go inline frame; hidden inlining limits correspondence.',
                       'Inclusive groups overlap. No call counts, instruction latency or speedup follows from weights.',
                       go_summary['madvise_contract']]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rust-prefix', type=Path, default=ROOT/'target/s07-bis/post-text-cpu/benchmark')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = compare(args.rust_prefix.resolve())
    if args.output.exists(): raise ValueError('Refuse to overwrite an existing comparison')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True)+'\n')
    print(json.dumps({'rust_bind_ns': result['rust']['cpu_ns'],
                      'go_bind_ns': [r['cpu_ns'] for r in result['go']['runs']],
                      'output': str(args.output)}))


if __name__ == '__main__':
    main()
