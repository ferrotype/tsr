#!/usr/bin/env python3
"""Source routing audit; compiled Go lists, when supplied, validate the roster."""
from __future__ import annotations
import argparse
from collections import Counter
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[3]
TEST = re.compile(r'^func (Test\w+)\(\w+ \*testing\.T\)\s*\{', re.M)

# Reviewed assertions, not name similarity or protocol-family coverage.
DIRECT_AUDIT = {
    ('project', 'TestCheckerPoolDiagnosticsRouting'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:164 checks slot 0 eviction, but does not compare acquired diagnostics identity with the slot-0 identity',
    ('project', 'TestCheckerPoolQueryRouting'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:103 checks an idle query index 1, but not acquired identity exclusion from diagnostics slot 0',
    ('project', 'TestCheckerPoolRequestAffinity'):
        'Exact observation assignment: crates/tsr_project/src/scheduler/tests.rs:73 (requests_reuse_across_nested_calls_and_releases_but_not_categories): held nested request and cross-release identity equality; source audit only, execution not certified here',
    ('project', 'TestCheckerPoolIdleCleanup'):
        'Assigned observation (source audit, not executed): crates/tsr_project/src/scheduler/tests.rs:164 observes released diagnostics slot disposal and fresh query identity after its configured idle interval; independent per-category observations cover idle eviction without requiring identical timeout values',
    ('project', 'TestCheckerPoolFileAssociationCleanup'):
        'WORK: no test asserts file association exists before idle disposal and is removed after its configured deadline',
    ('project', 'TestCheckerPoolMinCheckers'):
        'WORK: no scheduler test supplies MaxCheckers=1 and asserts normalized maximum and slot count are both 2',
    ('project', 'TestCheckerPoolDefaultIdleTimeout'):
        'WORK: setup always supplies 10 seconds; no zero-timeout input and 30-second default assertion',
    ('project', 'TestCheckerPoolQueryContention'):
        'Assigned observation (source audit, not executed): crates/tsr_project/src/scheduler/tests.rs:230 observes a distinct request blocked on the only held query slot and successful acquisition after release; prior request affinity adds coverage without weakening the contention invariant',
    ('project', 'TestCheckerPoolDiagnosticsContention'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:230 blocks a query, not a second diagnostics request; native blocked diagnostics, independent query and subsequent diagnostics unblock sequence is missing',
    ('project', 'TestCheckerPoolCanceledCheckerDisposal'):
        'Exact observation assignment: crates/tsr_project/src/scheduler/tests.rs:267 (canceled_checkers_and_their_associations_are_disposed_in_all_categories): canceled diagnostic operation marks query checker canceled, release replaces identity with a usable checker; Rust also tests diagnostics/API categories; source audit only, execution not certified here',
    ('project', 'TestCheckerPoolRequestAssociationCleanupOnDisposal'):
        'Exact observation assignment: crates/tsr_project/src/scheduler/tests.rs:267: cancellable named request association is empty after canceled-checker release; native one association is represented by empty Rust request map; source audit only, execution not certified here',
    ('project', 'TestCheckerPoolRequestAssociationCleanupOnContextDone'):
        'Exact observation assignment: crates/tsr_project/src/scheduler/tests.rs:73: request association persists after release and is empty synchronously after request-context cancellation; source audit only, execution not certified here',
    ('project', 'TestCheckerPoolDiagnosticsRecreatedAfterIdleDisposal'):
        'Exact observation assignment: crates/tsr_project/src/scheduler/tests.rs:164: released diagnostics slot becomes uninitialized at its idle deadline and next acquisition has a fresh identity; source audit only, execution not certified here',
    ('project', 'TestCheckerPoolCrossReleaseAffinityWithContention'):
        'Exact observation assignment: crates/tsr_project/src/scheduler/tests.rs:230: A releases the only query slot, B holds it, A reacquisition reaches contention and returns A identity after B release; source audit only, execution not certified here',
    ('project', 'TestCheckerPoolLifetimeMismatchIgnoresAssociation'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:73 tests temporary-to-diagnostics while temporary remains held; native diagnostics-to-temporary after diagnostics release and explicit slot-0 exclusion are missing',
    ('project', 'TestCheckerPoolNoRequestID'):
        'WORK: no test performs two released successful acquisitions with an empty request ID (unscoped_reentry tests held operation failure instead)',
    ('project', 'TestCheckerPoolDiagnosticsCrossReleaseAffinity'):
        'WORK: no test reacquires diagnostics with the same cancellable request before timeout and asserts slot-0 identity equality',
    ('project', 'TestCheckerPoolDiscardKeepsIdleCheckers'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:198 discards with query held; native both idle at discard, stopped cleanup timer and immediate identity assertions are missing',
    ('project', 'TestCheckerPoolDiscardHeldCheckerSurvivesRelease'):
        'Assigned observation (source audit, not executed): crates/tsr_project/src/scheduler/tests.rs:198 holds query across discard, releases it and advances beyond idle timeout; same unique checker identity on reacquisition demonstrates uninterrupted survival through discard and release',
    ('project', 'TestCheckerPoolDiscardStillFunctional'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:198 reuses preexisting checkers; native discard-before-first-acquisition creates a fresh query, checks index and reuses it under a different request ID',
    ('project', 'TestCheckerPoolDiagnosticsCheckerStableIdentity'):
        'WORK: no before-timeout diagnostics identity equality under two different request IDs',
    ('project', 'TestCheckerPoolDiagnosticsCheckerSurvivesDiscard'):
        'Exact observation assignment: crates/tsr_project/src/scheduler/tests.rs:198: released diagnostics identity survives discard and delayed reacquisition; Rust additionally invokes a late cleanup callback and advances 100 seconds; source audit only, execution not certified here',
    ('project', 'TestCheckerPoolDiagnosticsCheckerIndependentFromQuery'):
        'Exact observation assignment: crates/tsr_project/src/scheduler/tests.rs:73 and 230: simultaneously held diagnostics and temporary checkers have distinct identities; source audit only, execution not certified here',
    ('project', 'TestCheckerPoolAPICheckerStableIdentity'):
        'Assigned observation (source audit, not executed): crates/tsr_project/src/scheduler/tests.rs:164 releases API checker, advances beyond the idle timeout and reacquires the same unique identity; this covers API identity preservation across release and idle time',
    ('project', 'TestCheckerPoolAPICheckerSurvivesDiscard'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:198 never acquires API; no persistent-checker identity and reacquisition assertions after discard',
    ('project', 'TestCheckerPoolAllThreeIndependent'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:164 holds all categories but never asserts all three pairwise-distinct identities',
    ('project', 'TestCheckerPoolFileAffinity'):
        'Exact observation assignment: crates/tsr_project/src/scheduler/tests.rs:103 (queries_prefer_file_affinity_and_then_an_existing_idle_checker): same file under different named requests returns the same released checker; source audit only, execution not certified here',
    ('project', 'TestCheckerPoolMultipleConcurrentQueryCheckers'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:103 holds only two query checkers; native three pairwise-distinct checkers, diagnostics-slot exclusion and blocked fourth/unblock sequence are missing',
    ('project', 'TestCheckerPoolDoubleReleaseSafe'):
        'WORK: Rust consuming Drop cannot express native repeated release callback; explicit representation correspondence and post-double-release successful acquisition remain to document',
    ('project', 'TestCheckerPoolDefaultMaxCheckers'):
        'WORK: no zero-MaxCheckers input asserting maximum 4, four slots and query capacity 3',
    ('project', 'TestCheckerPoolStaggeredIdleCleanup'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:344 proves staggered per-slot expiry at configured deadlines, but does not assert both slots remain initialized after second release and before either deadline (native observes that at t=6); differing timeout values and unused slot capacity are not gaps',
    ('project', 'TestCheckerPoolDiscardIdempotent'):
        'Assigned observation (source audit, not executed): crates/tsr_project/src/scheduler/tests.rs:198 calls discard twice and successfully reacquires existing diagnostics/query identities after release and elapsed time; stable unique identities demonstrate preservation and pool usability',
    ('project', 'TestCheckerPoolGetGlobalDiagnosticsEmpty'):
        'Exact observation assignment: crates/tsr_project/src/scheduler/tests.rs:296 (globals_accumulate_once_and_survive_disposal): global diagnostic collection is empty before any checker acquisition; both fixtures use noLib; source audit only, execution not certified here',
    ('project', 'TestCheckerPoolTakeNewGlobalDiagnostics'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:296 checks reset/stable globals after acquisition, but native executes query diagnostics twice on the same file under distinct requests before checking flag stability',
    ('project', 'TestCheckerPoolAPICheckerDisposedOnCancel'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:267 covers canceled API replacement and a usable next operation; independent immediate persistent-slot absence assertion is missing',
    ('project', 'TestCheckerPoolNonCancelableContextNoAffinity'):
        'Exact observation assignment: crates/tsr_project/src/scheduler/tests.rs:73: background context with nonempty ignored-name succeeds and does not grow request-association map; source audit only, execution not certified here',
    ('project', 'TestCheckerPoolCleanupAfterDiscardIsNoop'):
        'WORK: crates/tsr_project/src/scheduler/tests.rs:198 simulates a late cleanup callback and preserves checker identities, but does not assert cleanup timer remains absent (no re-arm)',
    ('lsp', 'TestDynamicQueueFIFO'):
        'Exact observation assignment: crates/tsr_lsp/src/dynamic_queue.rs:105 '
        '(fifo_and_canceled_operations_preserve_remaining_items); same ordered 0..1000 put/get assertions; execution not certified here',
    ('lsp', 'TestDynamicQueueGetCancellation'):
        'WORK: crates/tsr_lsp/src/dynamic_queue.rs:105 observes pre-canceled get error; '
        'native additionally asserts returned zero value (Rust Result has no accompanying value); explicit representation correspondence remains to document',
    ('lsp', 'TestDynamicQueuePutCancellationWhileStateUnavailable'):
        'WORK: crates/tsr_lsp/src/dynamic_queue.rs:121 tests cancellation of a full bounded writer; '
        'native holds the internal queue state unavailable, then releases it and verifies a subsequent put/get of 2; that exact observation is missing',
    ('lsp/lspwatcher', 'TestRootFromGlob'):
        'Exact observation assignment: crates/tsr_lsp/src/watcher/tests.rs:215 '
        '(roots_from_pinned_globs); all four pinned inputs and expected roots match; execution not certified here',
    ('lsp/lspwatcher', 'TestWatcher_CreateChangeDelete'):
        'WORK: crates/tsr_lsp/src/watcher/tests.rs:348 observes real-backend missing-directory promotion; '
        'native existing-root changed/deleted notifications and unregistration are not asserted by that test',
    ('lsp/lspwatcher', 'TestWatcher_RealBackend_MissingThenCreate'):
        'WORK: crates/tsr_lsp/src/watcher/tests.rs:348 is related real-backend coverage, '
        'but returns without observation when fast recursive backend is absent; compare all native event/path expectations before exact assignment',
}


def collect(root: Path = ROOT) -> list[dict]:
    references: dict[str, list[str]] = {}
    for path in sorted((root / 'crates').rglob('*.rs')):
        text = path.read_text()
        # A name in a source comment is evidence of an observation port, not
        # certification that every native subcase is implemented or passing.
        for number, line in enumerate(text.splitlines(), 1):
            if '//' not in line:
                continue
            for name in re.findall(r'\bTest[A-Z]\w*', line.split('//', 1)[1]):
                references.setdefault(name, []).append(f'{path.relative_to(root)}:{number}')
    rows = []
    internal = root / 'upstream/tsc/internal'
    for area in ('project', 'lsp', 'ls', 'fourslash/tests'):
        for path in sorted((internal / area).rglob('*_test.go')):
            text = path.read_text()
            matches = list(TEST.finditer(text))
            for i, match in enumerate(matches):
                name = match.group(1)
                body = text[match.end():matches[i + 1].start() if i + 1 < len(matches) else len(text)]
                package = str(path.parent.relative_to(internal))
                source = f'{path.relative_to(root)}:{text.count(chr(10), 0, match.start()) + 1}'
                if area == 'fourslash/tests':
                    route = f'fourslash/{name}'
                elif package == 'lsp' and (
                    'lsptestutil.NewLSPClient(' in body or path.name in {
                        'server_completion_test.go', 'server_contentmapper_test.go',
                        'server_progress_test.go', 'server_projectinfo_test.go',
                        'server_projectreference_updates_test.go', 'server_semantictokens_test.go'}):
                    route = f'lsp/{name}' if name != 'TestReplay' else 'fixture-dependent replay; no default corpus route'
                elif package == 'project/ata':
                    route = {
                        'TestValidatePackageName': 'Rust observation port: tsr_project::ata::tests::pinned_package_name_validation',
                        'TestInstallNpmPackages': 'Rust mock-executor port: tsr_project::ata::tests::pinned_npm_long_command_runs_both_batches_even_after_failure; real npm integration is opt-in',
                        'TestDiscoverTypings': 'Rust observation family: crates/tsr_project/src/ata/tests.rs (pinned_discovery_* and manifest/cache cases); native skipped local @types deduplication remains excluded',
                        'TestATA': 'Rust observation family: crates/tsr_project/src/session/ata/tests.rs (pinned_* acquisition cases); no claim of all native subcases',
                    }[name]
                elif (package, name) in DIRECT_AUDIT:
                    route = DIRECT_AUDIT[(package, name)]
                elif name in references:
                    route = 'Rust source references: ' + ', '.join(sorted(set(references[name])))
                else:
                    route = 'WORK: exact native-test-to-Rust-test assignment unverified'
                rows.append(dict(package=package, name=name, source=source, route=route,
                                 skip_call='t.Skip(' in body or 't.Skipf(' in body,
                                 first_statement_skip=bool(re.match(r'\s*t\.Skip\(', body)),
                                 state_baseline='@stateBaseline: true' in body,
                                 mapper=path.name.startswith('contentMapper'),
                                 tsc_prebuild='@tsc:' in body))
    return rows


def verify_lists(rows: list[dict], specs: list[str]) -> None:
    for spec in specs:
        package, filename = spec.split('=', 1)
        if package not in {row['package'] for row in rows}:
            raise ValueError(f'unknown package: {package}')
        actual = {line.strip() for line in Path(filename).read_text().splitlines()
                  if re.fullmatch(r'Test\w+', line.strip()) and line.strip() != 'TestMain'}
        expected = {row['name'] for row in rows if row['package'] == package}
        if actual != expected:
            raise ValueError(f'{package}: source-only={sorted(expected - actual)}, compiled-only={sorted(actual - expected)}')


def markdown(rows: list[dict], verified: list[str]) -> str:
    counts = Counter(row['package'] for row in rows)
    work_counts = Counter(row['package'] for row in rows if row['route'].startswith('WORK:'))
    fs = [row for row in rows if row['package'] == 'fourslash/tests']
    lines = ['# Phase 5 pinned test routing inventory', '',
             'Generated by `python3 tools/phase5/harness/inventory.py --markdown docs/PHASE5-tests.md`.', '',
             'This is a source audit of the pinned Go top-level test functions, excluding `TestMain`.',
             'It records routes and source references, not execution results. Suite routes require the',
             'compiled harness list and actual Go/Rust results before they confer acceptance credit.',
             'Rust comment references identify ported observations; they do not certify all subcases.',
             'The CLI JSON output contains every fourslash id; the table below groups that family.',
             'WORK rows require an exact port assignment or implementation, even where family coverage',
             'exists in the development checks. Runtime skip events determine the executable denominator.', '',
             'Validate a compiled roster with `--compiled-list PACKAGE=FILE`, repeated per package.',
             'FILE is the captured output of that package test binary with `-test.list ^Test`.',
             'Build tags, platform constraints and generated tests can change runtime membership.',
             'For host-filtered binaries use `runner.py list SUITE --prepared DIR`: its preparation',
             'records `go list -json` TestGoFiles/XTestGoFiles and GOOS/GOARCH, then validates',
             'the compiled list against exactly those included sources. The inventory CLI',
             '`--compiled-list` check compares the whole source roster and detects omissions.',
             '',
             'The native-host compiled fourslash list observed during L7 bring-up contains',
             '**4,534 tests**, excluding 13 functions in `*_js_test.go` files. Go interprets',
             'that filename suffix as GOOS=js. These are platform exclusions, not runtime skips;',
             'the 4,547-function source roster remains visible in the CLI JSON inventory.',
             'The acceptance denominator N is not measured by this list; it requires Go outcomes.', '',
             'Compiled rosters checked in this generation: ' + (', '.join(verified) if verified else '**none**') + '.', '',
             '| Package | Source test functions |', '| --- | ---: |']
    lines.extend(f'| `{package}` | {count} |' for package, count in sorted(counts.items()))
    lines += ['', f'Fourslash source facts: {len(fs)} functions; '
              f'{sum(r["first_statement_skip"] for r in fs)} first-statement skip calls; '
              f'{sum(r["state_baseline"] for r in fs)} state-baseline functions; '
              f'{sum(r["mapper"] for r in fs)} contentMapper-file functions; '
              f'{sum(r["tsc_prebuild"] for r in fs)} prebuild functions.', '',
              'A skip call in the source region up to the next test declaration is flagged below;',
              'helpers in that region may contain it. This is not an observed skip.',
              'Compiler-option-driven skips reside in shared harness code and are not inferred here.', '',
              'Development routes: `tools/phase5/project/check.py` compares original Go snapshot-writer',
              'bytes; `tools/phase5/lsp/check.py` carries selected native client tests; the other scripts',
              'under `tools/phase5/lsp/` compare bounded synthetic scenarios. Their documented past',
              'results are not rerun results of this inventory or full pinned corpus acceptance.', '',
              'L6 already added the project content-mapper observation ports referenced below, ATA',
              'discovery/validation/mock-install/session coverage, and the carried',
              '`TestSetContentMapperContributionsBeforeDidOpen` client check. Exact residuals',
              'remain runtime evidence work. The L7 planning reference predates those ports.',
              'Its custom API-session',
             'wire claim also conflicts with the L6 record, which assigns that handshake to Phase 6.', '',
              'Bounded direct-test audit (2026-10-06): exact assignments below compare assertions,',
              'not Rust test names. The queue FIFO and four glob-root cases match their native',
              'observations. The checkerpool audit assigns matching lifecycle observations and names',
              'missing independent slot, timer, category, cancellation and capacity assertions.',
              'Configured timeout or unused capacity differences alone do not create WORK.',
              'Assigned observations are source comparisons, not execution evidence.',
              'Cancellation and real watcher routes retain missing',
              'observations. These assignments do not claim tests were run in this audit.',
              'Remaining WORK rows are the concrete L7/L8 assignment backlog; source-reference',
              'rows still require subcase review and runtime evidence before acceptance credit.', '',
              f'Unassigned or partial direct-test observations: **{sum(work_counts.values())}**.',
              'Each WORK row below identifies its native test and source; the counts do not',
              'include source-reference rows whose exact subcase review is still pending.', '',
              '| Package | WORK rows |', '| --- | ---: |']
    lines.extend(f'| `{package}` | {count} |' for package, count in sorted(work_counts.items()))
    lines += ['', '| Pinned test | Source | Route / evidence | Skip call in source region |',
              '| --- | --- | --- | --- |']
    lines.append('| `fourslash/tests/Test*` (4,547 functions) | `upstream/tsc/internal/fourslash/tests/*_test.go` | `fourslash/<TestName>`; enumerate all ids with the CLI JSON output | runtime authority |')
    lines.extend(f'| `{r["package"]}/{r["name"]}` | `{r["source"]}` | {r["route"]} | {"yes" if r["skip_call"] else ""} |' for r in rows if r['package'] != 'fourslash/tests')
    return '\n'.join(lines) + '\n'


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--markdown', type=Path)
    parser.add_argument('--compiled-list', action='append', default=[])
    args = parser.parse_args()
    rows = collect()
    verify_lists(rows, args.compiled_list)
    if args.markdown:
        args.markdown.write_text(markdown(rows, [s.split('=', 1)[0] for s in args.compiled_list]))
    else:
        print(json.dumps(rows, indent=2))


if __name__ == '__main__':
    main()
