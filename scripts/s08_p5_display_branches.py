#!/usr/bin/env python3
"""Bounded native branch witnesses; diagnostic counters, never acceptance evidence."""
import argparse
from pathlib import Path

from s04 import go_environment, verified_upstream
from s04_common import command
from s08_oracle import ROOT, canonical, digest, strict_json_loads
from s08_p5_display import DRIVER, validate

BRANCHES = ('override_accepted', 'override_rejected', 'retry_attempted', 'retry_succeeded', 'override_other_symbol')
BRIDGE = '''package checker
var s08DisplayBranches [5]int
func S08DisplayBranchReset() { s08DisplayBranches = [5]int{} }
func S08DisplayBranchCounts() [5]int { return s08DisplayBranches }
'''


def replace_once(source, old, new):
    if source.count(old) != 1:
        raise ValueError('native branch instrumentation anchor drift: ' + old)
    return source.replace(old, new)


def capture(request_path, directory):
    directory = directory.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    upstream = verified_upstream()
    env = go_environment()
    package = upstream / 'tsc/internal/checker'
    original = (package / 'nodebuilderimpl.go').read_text()
    instrumented = replace_once(original,
        'return resolved != nil && b.ch.getMergedSymbol(resolved) == b.ch.getMergedSymbol(symbol)',
        '''matches := resolved != nil && b.ch.getMergedSymbol(resolved) == b.ch.getMergedSymbol(symbol)
        if matches { s08DisplayBranches[0]++ } else { s08DisplayBranches[1]++; if resolved != nil { s08DisplayBranches[4]++ } }
        return matches''')
    instrumented = replace_once(instrumented, 'swappedMode := core.ModuleKindESNext',
                                's08DisplayBranches[2]++\n swappedMode := core.ModuleKindESNext')
    instrumented = replace_once(instrumented, 'importModeOverride = swappedMode',
                                's08DisplayBranches[3]++\n importModeOverride = swappedMode')
    driver = replace_once(DRIVER.read_text(), 'programs := []any{}',
                         'programs := []any{}\n branchRows := []any{}')
    driver = replace_once(driver, 'for _, q := range r.Queries {',
                         'for _, q := range r.Queries {\n checker.S08DisplayBranchReset()')
    anchor = 'queries = append(queries, result)'
    if driver.count(anchor) != 2:
        raise ValueError('native display query recording anchor drift')
    driver = driver.replace(anchor, '''branchRows = append(branchRows, map[string]any{
        "program": r.ID, "query": q.ID, "counts": checker.S08DisplayBranchCounts()})
        ''' + anchor)
    driver = replace_once(driver, 'hash := sha256.Sum256(raw)', '''branchBytes, err := json.Marshal(branchRows)
    if err != nil { t.Fatal(err) }
    if err := os.WriteFile(os.Getenv("S08_BRANCHES"), branchBytes, 0600); err != nil { t.Fatal(err) }
    hash := sha256.Sum256(raw)''')
    replacements = {}
    for name, content in [('nodebuilderimpl.go', instrumented),
                          ('codex_s08_display_branches.go', BRIDGE),
                          ('codex_s08_display_branches_test.go', driver)]:
        if name != 'nodebuilderimpl.go' and (package / name).exists():
            raise ValueError('diagnostic overlay would replace an existing bridge')
        path = directory / name
        path.write_text(content)
        replacements[str(package / name)] = str(path)
    (directory / 'overlay.json').write_bytes(canonical({'Replace': replacements}))
    request = strict_json_loads(request_path.read_bytes())
    raw = canonical(request) + b'\n'
    (directory / 'requests.json').write_bytes(raw)
    env.update(S08_REQUESTS=str(directory / 'requests.json'),
               S08_OUTPUT=str(directory / 'observations.json'),
               S08_BRANCHES=str(directory / 'branches.json'))
    sources = {p: digest(p.read_bytes()) for p in [Path(__file__), DRIVER, *map(Path, replacements.values())]}
    stdout = command(['go', 'test', '-trimpath', '-mod=readonly', '-overlay', str(directory / 'overlay.json'),
                      './internal/checker', '-run', '^TestS08P5Display$', '-count=1', '-timeout=5m'],
                     cwd=upstream / 'tsc', env=env)
    (directory / 'go-test.stdout').write_bytes(stdout)
    verified_upstream()
    if any(digest(p.read_bytes()) != expected for p, expected in sources.items()):
        raise ValueError('branch probe sources changed during capture')
    observed = strict_json_loads((directory / 'observations.json').read_bytes())
    validate(request, observed)
    if observed['request_sha256'] != digest(raw):
        raise ValueError('branch probe request fingerprint differs')
    branches = strict_json_loads((directory / 'branches.json').read_bytes())
    identities = [(p['id'], q['id']) for p in request['programs'] for q in p['queries']]
    if [(r['program'], r['query']) for r in branches] != identities:
        raise ValueError('branch probe query schedule differs')
    if any(len(r['counts']) != len(BRANCHES) or any(type(v) is not int or v < 0 for v in r['counts']) for r in branches):
        raise ValueError('invalid native branch counters')
    paths = [Path(__file__), DRIVER, *map(Path, replacements.values()),
             directory / 'requests.json', directory / 'observations.json', directory / 'branches.json']
    provenance = {'pin': strict_json_loads((ROOT / 'data/upstream.json').read_bytes())['pin'],
                  'original_sha256': digest(original.encode()), 'branch_order': BRANCHES,
                  'go': observed['go'], 'goos': observed['goos'], 'goarch': observed['goarch'],
                  'toolchain_local': env['GOTOOLCHAIN'] == 'local',
                  'sha256': {str(p.relative_to(ROOT) if p.is_relative_to(ROOT) else p): digest(p.read_bytes()) for p in paths}}
    (directory / 'provenance.json').write_bytes(canonical(provenance) + b'\n')
    for row in branches:
        print(row['program'], row['query'], dict(zip(BRANCHES, row['counts'], strict=True)))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--requests', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    capture(args.requests, args.output)
