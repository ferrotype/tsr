#!/usr/bin/env python3
"""Write docs/PHASE6-tests.md: the pinned client's test cases by file from a
native run, and every pinned Go test of `internal/api` with its route."""
from __future__ import annotations
import argparse
from collections import Counter
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[3]
TEST = re.compile(r'^func (Test\w+)\(\w+ \*testing\.T\)', re.M)

# Reviewed routes: a Rust test that ports the observation, or the work that
# still owes it. A name here is a claim about assertions, not similarity.
ROUTES = {
    ('api', 'TestDocumentIdentifierUnmarshalJSON'):
        'crates/tsr_api/src/proto/tests.rs pinned_wire_fixtures_hold: the five document-identifier fixtures of the S03 export are the same inputs',
    ('api', 'TestJSONValueToAny'): 'crates/tsr_api/src/session/service_tests.rs client_json_keeps_order_nulls_and_empty_arrays (through parseJsonConfigFileContent)',
    ('api', 'TestStandaloneSessionUsesSnapshotHostWithoutProjectSession'):
        'crates/tsr_api/src/session/tests.rs a_standalone_session_updates_snapshots_over_its_own_project_session and create_program_builds_one_synthetic_project_from_explicit_roots (snapshot ids are not compared: the Rust counter is process-wide)',
    ('api', 'TestSessionTracksAndReleasesAPIRefs'):
        'crates/tsr_api/src/session/tests.rs project_opens_are_idempotent_and_closes_release_only_held_refs (the standalone session; the shared project session case is A3)',
    ('api', 'TestUpdateTemporarySnapshot'):
        'crates/tsr_api/src/session/tests.rs a_temporary_snapshot_overrides_one_file_without_advancing_the_latest (file text instead of semantic diagnostics; the diagnostics methods have their own cases in service_tests.rs)',
    ('api', 'TestUpdateTemporarySnapshotRejectsUnsupportedExtension'):
        'crates/tsr_api/src/session/tests.rs a_temporary_snapshot_overrides_one_file_without_advancing_the_latest (the tail)',
    ('api', 'TestCreateProgram'):
        'crates/tsr_api/src/session/tests.rs create_program_builds_one_synthetic_project_from_explicit_roots and service_tests.rs create_program_returns_the_client_config_diagnostics (semantic diagnostics are the diagnostics methods\' cases)',
    ('api', 'TestCreateProgramFileChangesRequireOldProgram'):
        'crates/tsr_api/src/session/tests.rs create_program_builds_one_synthetic_project_from_explicit_roots (the error text)',
    ('api', 'TestNewDiagnosticResponseIncludesFormattingContext'):
        'crates/tsr_api/src/session/responses_tests.rs diagnostic_response_includes_formatting_context',
    ('api', 'TestNewDiagnosticResponseTruncatesLongFormattingContext'):
        'crates/tsr_api/src/session/responses_tests.rs diagnostic_response_truncates_long_formatting_context',
    ('api', 'TestHandleBatchRequests'): 'crates/tsr_api/src/session/service_tests.rs batch_items_answer_individually',
    ('api', 'TestHandleBatchRequestsRejectsNestedBatch'):
        'crates/tsr_api/src/session/service_tests.rs batch_items_answer_individually (the nested item)',
    ('api', 'TestHandleBatchRequestsRecoversPerRequestPanics'):
        'crates/tsr_api/src/session/service_tests.rs batch_items_report_panics_alone (the type-kind refusal the pin panics on)',
    ('api', 'TestBatchResponseEncodesEmptyResult'): 'crates/tsr_api/src/session/batch.rs empty_results_encode_as_empty_arrays',
    ('api', 'TestHandleBatchRequestsPaginatesResponses'):
        'crates/tsr_api/src/session/service_tests.rs batch_pages_respect_the_byte_limit',
    ('api', 'TestHandleBatchRequestsAllowsOversizedSingleResponse'):
        'crates/tsr_api/src/session/service_tests.rs batch_pages_respect_the_byte_limit (the oversized item)',
    ('api', 'TestHandleBatchRequestsPageLimitIsRequestScoped'):
        'crates/tsr_api/src/session/service_tests.rs batch_pages_respect_the_byte_limit (the limited and unlimited pair)',
    ('api', 'TestHandleBatchRequestsRejectsInvalidContinuationToken'):
        'crates/tsr_api/src/session/service_tests.rs batch_pages_respect_the_byte_limit (the tail)',
    ('api', 'TestCompletionSymbolTypeIsResolvable'):
        'crates/tsr_api/src/session/service_tests.rs completions_carry_resolvable_symbols (the library limited to es5)',
    ('api', 'TestCompletionOnInferredProject'):
        'crates/tsr_api/src/session/service_tests.rs completions_answer_on_an_inferred_project',
    ('api', 'TestCompletionRetriesWithAutoImports'):
        'crates/tsr_api/src/session/service_tests.rs completions_include_module_exports (no retry: the service builds the registry on demand)',
    ('api', 'TestToAPITextEditsUsesOriginalCoordinates'): 'crates/tsr_api/src/session/responses_tests.rs text_edits_use_original_coordinates',
    ('api', 'TestUpdateSnapshotResponseSkipsUnloadedAncestorProject'):
        'crates/tsr_api/src/session/service_tests.rs update_snapshot_reports_loaded_projects_only (the standalone session opens the file through updateSnapshot)',
    ('api', 'TestCreateProgramWithNoRootFiles'): 'crates/tsr_api/src/session/service_tests.rs create_program_accepts_an_empty_root_set',
    ('api', 'TestCreateProgramRemovesAllRootFiles'): 'crates/tsr_api/src/session/service_tests.rs create_program_accepts_an_empty_root_set (the tail)',
    ('api', 'TestCreateProgramPreservesRootFileOrder'):
        'crates/tsr_api/src/session/service_tests.rs create_program_preserves_root_file_order (the pin\'s ProgramUpdateKind is internal to its program reuse)',
    ('api', 'TestCreateProgramReusesProgram'):
        'crates/tsr_api/src/session/service_tests.rs create_program_with_an_old_program_sees_changes_and_new_options (the observable half; the Rust session loads programs afresh, docs/PHASE6-A2.md)',
    ('api', 'TestCreateProgramProjectReferencesAndReuse'):
        'crates/tsr_api/src/session/service_tests.rs create_program_resolves_project_references (the observable half; no program reuse)',
    ('api', 'TestCreateProgramFromConfiguredProgramDoesNotRetainOtherProjects'):
        'crates/tsr_api/src/session/service_tests.rs create_program_from_a_configured_project_drops_the_other_projects',
    ('api', 'TestUpdateTemporarySnapshotAddsUnopenedFile'):
        'crates/tsr_api/src/session/service_tests.rs temporary_snapshots_add_unopened_files_and_derive_from_the_client_base',
    ('api', 'TestUpdateTemporarySnapshotUsesClientSnapshotAsBase'):
        'crates/tsr_api/src/session/service_tests.rs temporary_snapshots_add_unopened_files_and_derive_from_the_client_base (the tail; the later file arrives through the update\'s file changes)',
    ('api/encoder', 'TestEncodeSourceFile'): 'crates/tsr_encoder/src/baseline_tests.rs encode_source_file_matches_the_baseline (the pinned baseline file, byte for byte)',
    ('api/encoder', 'TestEncodeSourceFileWithUnicodeEscapes'): 'crates/tsr_encoder/src/baseline_tests.rs encode_source_file_with_unicode_escapes_matches_the_baseline',
    ('api/encoder', 'TestEncodeContentMapperSourceFileMetadata'): 'crates/tsr_encoder/src/baseline_tests.rs content_mapper_metadata_is_encoded_in_the_extended_data',
    ('api/encoder', 'TestBuildNodeIndexTableMatchesEncode'): 'crates/tsr_encoder/src/baseline_tests.rs the_node_index_table_matches_the_encoding',
}
CHECKPOINTS = {
    'session_batch_test.go': 'A4', 'session_completion_test.go': 'A4', 'session_textedit_test.go': 'A4',
    'session_apistate_test.go': 'A3', 'session_createprogram_test.go': 'A3', 'session_temporary_test.go': 'A3',
    'decoder_test.go': 'equivalent coverage, owner review owed (docs/PHASE6-A5.md): the Rust decoder reads the format the Phase 0 corpus parity froze over 16,120 files; it is exercised by printNode and formatNodeForInsertion over client-encoded trees (tsr_api printing and formatting tests, the jsapi `SnapshotInternalAPI` and `printNode` cases) and every server encoding is decoded by the client suites (`test/encoder.test.ts`, `test/sync/ast.test.ts`)',
}


def go_tests():
    rows = []
    for path in sorted((ROOT / 'upstream/tsc/internal/api').rglob('*_test.go')):
        package = 'api' if path.parent.name == 'api' else f'api/{path.parent.name}'
        text = path.read_text()
        for match in TEST.finditer(text):
            name = match.group(1)
            if name == 'TestMain':
                continue
            route = ROUTES.get((package, name)) or f"WORK ({CHECKPOINTS.get(path.name, 'A5')})"
            line = text.count('\n', 0, match.start()) + 1
            rows.append((package, name, f'{path.relative_to(ROOT)}:{line}', route))
    return rows


def client_cases(results):
    cases = Counter()
    skips = Counter()
    for line in Path(results).read_text().splitlines():
        row = json.loads(line)
        if row['id'] == row.get('parent'):
            continue
        name = row['parent'].split('/', 1)[1]
        cases[name] += 1
        if row['state'] == 'skip':
            skips[name] += 1
    return cases, skips


def markdown(results):
    cases, skips = client_cases(results)
    tests = go_tests()
    work = sum(1 for row in tests if row[3].startswith('WORK'))
    lines = ['# Phase 6 pinned test routing', '',
             'Generated by `python3 tools/phase6/jsapi/inventory.py --results RESULTS --markdown docs/PHASE6-tests.md`',
             'from a native run of the `jsapi` suite and the pinned `internal/api` Go tests. It records',
             'routes, not results: suite rows need the actual native and Rust outcomes before they count,',
             'and a Rust test named here ports the observation it names, not every neighbouring case.', '',
             '## The pinned client suites (`jsapi`)', '',
             'One variant per test file, one row per case; the native run of the same command fixes the',
             'denominator and the native skip set.', '',
             '| File | Cases | Native skips |', '| --- | ---: | ---: |']
    lines.extend(f'| `{name}` | {count} | {skips[name]} |' for name, count in sorted(cases.items()))
    lines += [f'| total | {sum(cases.values())} | {sum(skips.values())} |', '',
              '## Direct Go tests of `internal/api`', '',
              f'{len(tests)} tests; **{work}** still routed to work. `WORK (A<n>)` names the checkpoint that',
              'owes the port or its equivalent-coverage explanation.', '',
              '| Pinned test | Source | Route |', '| --- | --- | --- |']
    lines.extend(f'| `{package}/{name}` | `{source}` | {route} |' for package, name, source, route in tests)
    lines += ['', '## Baselines', '',
              '`testdata/baselines/reference/api/encodeSourceFile.txt` and `encodeSourceFileWithUnicodeEscapes.txt`',
              'are the encoder goldens of `encoder_test.go`; A5 compares them through a Rust encoder test.', '']
    return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--results', required=True, help='results.ndjson of a jsapi run')
    parser.add_argument('--markdown', type=Path, required=True)
    args = parser.parse_args()
    args.markdown.write_text(markdown(args.results))


if __name__ == '__main__':
    main()
