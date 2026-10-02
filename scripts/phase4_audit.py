#!/usr/bin/env python3
"""Phase 4 function audit (docs/PHASE4-plan.md sections 2, 4 and 5, "Function disposition").

The scope is every pinned function (`data/go-functions.tsv`) of the Phase 4
files left after the owner's decisions:

* every `PORTS.toml` entry with `phase = 4` whose kind is `source` or
  `harness` (the 10 `out-of-scope` entries, Windows and other-platform files,
  are not);
* minus the three files decision 4 moves out of Phase 4: `fswatch/kqueue.go`
  (out of scope, ADR 0002), `compiler/projectreferencedtsfakinghost.go`
  (Phase 5) and `pprof/pprof.go` (Phase 7, decision 6). The difference holds
  before and after the ledger regeneration that applies the moves.

Decision 4's `GetDiagnosticsOfAnyProgram` move was not confirmed: Phase 3
ported and marked it, and it stays in `program.go`, a Phase 4 file. The scope
is bound below by count and digest, so a change to the ledger or the pin is
reviewed here too.

Each function gets one status, by `scripts/phase2_audit.py`'s rules as
`scripts/phase3_audit.py` applies them:

* `mapped` -- exactly one `port:` marker under `crates/` or `tools/` names it,
  or the reviewed number of sites for the four multi-site markers below;
* `equivalent` -- folded into another function, an unmarked port, or without a
  caller at the pin while the same fact is computed elsewhere; a reviewed entry
  names the Rust site by file and an anchor text that must occur exactly once
  there, and the build resolves it to `path:line`; `marker_to_add` says that
  the site is a one-to-one port that should carry the marker;
* `later` -- handed to a later phase (Phase 5 project system, auto-import,
  type acquisition and the language server; Phase 6 the API; Phase 7
  profiling), with the reason;
* `pending` -- the checkpoint (X0 to X7) that ports it: a reviewed entry, or
  every unmarked function of a file no checkpoint has ported yet. A marker
  supersedes a reviewed `pending` entry (listed in
  `pending_reviews_superseded`); it makes any other reviewed entry stale.

Problems (the audit is invalid): an unreviewed number of markers for one
function, a marker whose id under a Phase 4 file or package is not in the
inventory, an unmarked function of a ported file (the compiler files, the
diagnostic writer, `incremental`, `nativepath`) with no reviewed entry (`gap`),
a reviewed `equivalent` or `later` entry for a marked function, an
unresolvable anchor, an owner that is not a later phase or a checkpoint.
Markers naming functions of pinned `_test.go` files, which the inventory does
not list, are reported apart (`test_file_markers`). `pending` is open: valid,
but the audit is complete only without it (X7's "none gap"); `check
--complete` names the open functions per owning checkpoint.

    build               # writes data/phase4/x-audit.json; exits 1 on problems
    check [--complete]  # the committed audit is current and valid (and complete)
    summary             # the table by checkpoint group
"""
from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path
import sys
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_audit  # noqa: E402

AUDIT = ROOT / "data/phase4/x-audit.json"
MARKER = phase2_audit.MARKER
PIN = phase2_audit.REVIEWED_PIN
MARKER_ROOTS = ("crates", "tools")
# Decision 4 (and 6): the Phase 4 ledger files that leave the phase.
MOVED_OUT = {
    "tsc/internal/fswatch/kqueue.go": "out of scope: kqueue is not a target (ADR 0002; decision 4)",
    "tsc/internal/compiler/projectreferencedtsfakinghost.go":
        "Phase 5: only UseSourceOfProjectReference reaches it, set only by project/project.go (decision 4)",
    "tsc/internal/pprof/pprof.go": "Phase 7: profiling is deferred (decisions 4 and 6)",
}
PROGRAM = "tsc/internal/compiler/program.go"
# The reviewed denominator: (count, sha256 of the canonical sorted list).
REVIEWED_FILES = (67, "4632d665872d48273b8c36ca29f5c286489967afba2aabadf71e7a0ba6295ded")
REVIEWED_FUNCTIONS = (1007, "1e8820bc2b93a5135a4efdd1fcb2bde046728b96d6862a5065143aaa00d68cee")
STATUSES = ("mapped", "equivalent", "later", "pending", "gap", "duplicate")
OPEN = ("pending", "gap")
LATER_OWNERS = ("Phase 5", "Phase 6", "Phase 7")
CHECKPOINTS = tuple(f"X{number}" for number in range(8))
GROUPS = {
    "X0": "the tsctests harness",
    "X1": "driver, binary and system, compiler remainder, diagnostic writer, help",
    "X2": "incremental programs and .tsbuildinfo",
    "X3": "the build orchestrator",
    "X4": "the native file watcher",
    "X5": "watch mode",
    "X6": "tracing and statistics",
    "X7": "live systems and closure",
}
# Files with Rust ports before Phase 4: every unmarked function needs a
# reviewed disposition. The rest are ported by their checkpoint, and their
# unmarked functions are pending with it.
REVIEWED_PREFIXES = ("tsc/internal/compiler/", "tsc/internal/diagnosticwriter/",
                     "tsc/internal/execute/incremental/", "tsc/internal/nativepath/")


def group_of(go):
    """The checkpoint group of a Phase 4 file (plan section 4)."""
    for prefix, group in (("tsc/internal/execute/tsctests/", "X0"),
                          ("tsc/internal/execute/tsc/statistics.go", "X6"),
                          ("tsc/internal/execute/tsc.go", "X1"), ("tsc/internal/execute/tsc/", "X1"),
                          ("tsc/cmd/tsc/", "X1"), ("tsc/internal/compiler/", "X1"),
                          ("tsc/internal/diagnosticwriter/", "X1"), ("tsc/internal/nativepath/", "X1"),
                          ("tsc/internal/execute/incremental/", "X2"), ("tsc/internal/execute/build/", "X3"),
                          ("tsc/internal/fswatch/", "X4"), ("tsc/internal/execute/watcher.go", "X5"),
                          ("tsc/internal/execute/watchmanager/", "X5"), ("tsc/internal/tracing/", "X6")):
        if go.startswith(prefix):
            return group
    raise ValueError(f"no checkpoint group for {go}")


# Functions of the unported files that plan section 4 gives to another
# checkpoint than their file's.
OWNER_OVERRIDES = {
    "tsc/internal/execute/tsc.go:tscBuildCompilation": "X3",
    "tsc/internal/execute/tsc.go:performIncrementalCompilation": "X2",
    "tsc/internal/execute/tsc.go:startTracingIfNeeded": "X6",
    "tsc/internal/execute/tsc.go:stopTracing": "X6",
    "tsc/internal/execute/tsc/help.go:PrintBuildHelp": "X3",
    "tsc/internal/execute/tsc/diagnostics.go:CreateBuilderStatusReporter": "X3",
    "tsc/internal/execute/tsc/diagnostics.go:CreateWatchStatusReporter": "X5",
}

# Markers at more than one site, reviewed: the pinned function is ported at
# each of the sites that perform it.
MULTI_SITE = {
    "tsc/internal/compiler/program.go:Program.BindSourceFiles": (
        2, "Files the cache parses bind in cache.rs; content-mapped and supplemental files the loader parses itself "
           "bind in loader.rs."),
    "tsc/internal/compiler/fileloader.go:getModeForUsageLocation": (
        2, "metadata::usage_mode ports it; the checker host's tslib-import mode applies it to the synthetic, "
           "attribute-free import."),
    "tsc/internal/compiler/filesparser.go:parseTask.load": (
        2, "load_worker ports the task's load; its supplemental-file sub-task section carries the second marker."),
    "tsc/internal/compiler/includeprocessor.go:includeProcessor.addProcessingDiagnostic": (
        2, "verify_options adds the two processing diagnostics (not listed in the file list, not under rootDir) "
           "where each arises."),
}

_LOADER = "crates/tsr_compiler/src/loader.rs"
_REASON = "crates/tsr_compiler/src/include_reason.rs"
_HOST = "crates/tsr_compiler/src/checker_host.rs"
_SYMLINKS = "crates/tsr_compiler/src/checker_module_specifiers.rs"
_WRITER = "crates/tsr_compiler/src/diagnostic_writer/mod.rs"
_RESOLVED = "crates/tsr_compiler/src/diagnostic_writer/resolved.rs"
_PRETTY = "crates/tsr_compiler/src/diagnostic_writer/pretty.rs"
_REFERENCES = "crates/tsr_compiler/src/project_references.rs"


def _later(owner, reason):
    return {"disposition": "later", "owner": owner, "reason": reason}


def _pending(owner, reason):
    return {"disposition": "pending", "owner": owner, "reason": reason}


def _equivalent(path, anchor, reason, marker_to_add=False):
    return {"disposition": "equivalent", "rust": (path, anchor), "reason": reason, "marker_to_add": marker_to_add}


_P = PROGRAM + ":"
_PHASE5_LS = "Phase 5's language service"
# Reviewed dispositions of the unmarked functions: plan section 2's program.go
# 48 (61 before Phase 3), the 23 other unmarked functions of the compiler files
# that stay (24 before Phase 3), the diagnostic writer's 15, and the functions
# of unported files whose disposition the plan or the sources settle.
REVIEWED = {
    # program.go: the reuse family, watch's program view (X5).
    _P + "Program.ReuseProgram": _pending(
        "X5", "Watch's program reuse (execute/watcher.go:565); C7 handed it to Phase 4 (data/phase2/c7-audit.json) "
              "and X5 ports it with its content-mapped branch."),
    _P + "lazyValue.tryReuse": _pending("X5", "Called only by Program.ReuseProgram (program.go:397-399)."),
    _P + "Program.canReplaceFileInProgram": _pending(
        "X5", "Program.ReuseProgram's admissibility test (program.go:360, 377)."),
    _P + "Program.needsImportHelpersImportSpecifier": _pending(
        "X5", "Program.ReuseProgram's admissibility test (program.go:365, 380)."),
    _P + "equalModuleSpecifiers": _pending("X5", "canReplaceFileInProgram's comparison of imports (program.go:444)."),
    _P + "equalModuleAugmentationNames": _pending(
        "X5", "canReplaceFileInProgram's comparison of module augmentations (program.go:447)."),
    _P + "equalFileReferences": _pending(
        "X5", "canReplaceFileInProgram's comparison of file, type and library references (program.go:449-451)."),
    _P + "equalCheckJSDirectives": _pending(
        "X5", "canReplaceFileInProgram's comparison of @ts-check directives (program.go:452)."),
    _P + "Program.FilesByPath": _pending(
        "X5", "Its command-line callers are the watcher's (execute/watcher.go:295, 515, 541, 557); the API "
              "session's (api/session.go:3839) is Phase 6's. Program::file is GetSourceFileByPath; nothing "
              "publishes the whole map."),
    # program.go: the driver's reports (X1), the harness's program data (X2),
    # the statistics counters (X6).
    _P + "Program.ExplainFiles": _pending("X1", "--explainFiles output (execute/tsc/emit.go:156); plan X1."),
    _P + "Program.GetIncludeReasons": _pending(
        "X2", "Testing only: its one pinned caller is the harness's program baseline (execute/tsctests/sys.go:386); "
              "plan X2 names it. The Rust program keeps the reasons in the crate-private Program.include_reasons."),
    _P + "Program.IsMissingPath": _pending(
        "X2", "Testing only: its one pinned caller is the harness's program baseline (execute/tsctests/sys.go:393); "
              "plan X2 names it. Program::missing_files publishes the names, not this path test."),
    **{_P + f"Program.{name}": _pending(
        "X6", f"Read only by the statistics report (execute/tsc/statistics.go:{line}); plan X6 ports the five "
              "counters.")
       for name, line in (("LineCount", 75), ("IdentifierCount", 76), ("SymbolCount", 77), ("TypeCount", 78),
                          ("InstantiationCount", 79))},
    # program.go: unmarked ports.
    _P + "NewProgram": _equivalent(
        _LOADER, "program.option_verification = crate::verify_compiler_options(&program)?;",
        "C7's recorded disposition (data/phase2/c7-audit.json): Loader::run ends as the pin's constructor does, "
        "with option verification and then the mapper projects' option diagnostics."),
    _P + "Program.Tracing": _equivalent(
        "crates/tsr_compiler/src/checked_program.rs", "pub fn tracing(&self) -> Option<&Arc<dyn TraceSink>> {",
        "Its one pinned caller is the incremental program's emitBuildInfo (execute/incremental/program.go:323), "
        "ported in tsr_incremental through push_emit_trace, which reads this accessor; the trace-file writer of X6 "
        "is one more TraceSink behind it.", True),
    _P + "Program.GetResolvedModules": _equivalent(
        _LOADER, "pub fn resolutions(&self) -> &[Resolution] {",
        "checker.Program's whole-map view of the resolved modules. Its one pinned caller, the checker's "
        "getPackagesMap (checker/utilities.go:1686), is ported as the host's package_bundles_types (marked "
        "Program.GetPackagesMap), which scans Program::resolutions, the program's published resolved modules.",
        True),
    _P + "Program.GetDefaultResolutionModeForFile": _equivalent(
        _HOST, "fn get_default_resolution_mode_for_file(",
        "The checker host's method computes the pin's getDefaultResolutionModeForFile from the file's metadata and "
        "its per-file options (the redirect's options), as the Program method does.", True),
    _P + "Program.GetImportHelpersImportSpecifier": _equivalent(
        _HOST, "fn get_import_helpers_resolution_mode(",
        "checker.Program member. Its one checker reader, resolveHelpersModule (checker/checker.go:29026), needs the "
        "synthetic tslib import only for its reference and resolution mode; the Rust checker resolves the fixed "
        "`tslib` reference with the mode this host method computes (the design is recorded in "
        "crates/tsr_checker/src/host.rs)."),
    _P + "Program.GetGlobalTypingsCacheLocation": _equivalent(
        _HOST, "fn get_global_typings_cache_location(&self) -> Result<tsr_jsstring::JsString, Error> {",
        "It returns ProgramOptions.TypingsLocation, which only the project system sets (project/project.go:455, "
        "from the language server's options); no command-line program has one, and the checker host returns the "
        "empty location. The input arrives with Phase 5's project system.", True),
    _P + "lazyValue.getValue": _equivalent(
        _SYMLINKS, ".get_or_init(|| self.compute_known_symlinks())",
        "Its command-line caller is GetSymlinkCache's once-only computation (program.go:2226); the checker host "
        "keeps the known symlinks in a OnceLock initialized on first use. Its other callers, GetUnresolvedImports "
        "and collectPackageNames, are Phase 5's."),
    _P + "Program.ForEachResolvedModule": _equivalent(
        _SYMLINKS, "for resolution in program.resolutions() {",
        "Passed only to KnownSymlinks.SetSymlinksFromResolutions by GetSymlinkCache (program.go:2231); the Rust "
        "symlink computation iterates the program's resolved modules inline."),
    _P + "Program.ForEachResolvedTypeReferenceDirective": _equivalent(
        _SYMLINKS, "for resolution in program.type_resolutions() {",
        "Passed only to KnownSymlinks.SetSymlinksFromResolutions by GetSymlinkCache (program.go:2231); the Rust "
        "symlink computation iterates the program's type reference resolutions inline."),
    _P + "forEachResolution": _equivalent(
        _SYMLINKS, "for resolution in program.resolutions() {",
        "The iteration shared by the two ForEachResolved methods (program.go:2280, 2284); it is inline at both of "
        "the Rust symlink computation's loops."),
    _P + "Program.ResolveModuleName": _equivalent(
        "crates/tsr_module/src/resolver.rs", "pub fn resolve(",
        "No caller and no interface requirement at the pin (data/phase1/coverage-review.json, "
        "reviewed_unused_compiler_operations): the language service and type acquisition call "
        "module.Resolver.ResolveModuleName directly (ls/sourcedefinition.go:426, project/ata/ata.go:480). The "
        "nil-redirect resolution it forwards to is Resolver::resolve, as Phase 3 recorded for "
        "emitHost.ResolveModuleName."),
    _P + "Program.GetResolvedProjectReferenceFor": _equivalent(
        _REFERENCES, "config_to_project_reference: BTreeMap<JsString, Option<usize>>,",
        "No caller and no interface requirement at the pin (data/phase1/coverage-review.json, "
        "reviewed_unused_compiler_operations); the configToProjectReference lookup it wraps is the mapper's map, "
        "read by range_resolved_reference_worker."),
    # program.go: the project system, auto-import and type acquisition (Phase 5).
    _P + "Program.GetResolvedProjectReferences": _later(
        "Phase 5", "Its pinned callers are the project system (project/projectcollectionbuilder.go:661) and "
                   "auto-import (ls/autoimport/util.go:184)."),
    _P + "Program.RangeResolvedProjectReferenceInChildConfig": _later(
        "Phase 5", "Its one pinned caller is the project system (project/projectcollectionbuilder.go:670)."),
    _P + "Program.UsesUriStyleNodeCoreModules": _later(
        "Phase 5", f"Its one pinned caller is {_PHASE5_LS} (ls/lsutil/utilities.go:88)."),
    _P + "Program.UpdateProgram": _later(
        "Phase 5", "Its one pinned caller is the project system (project/project.go:411); command-line watch reuses "
                   "programs through ReuseProgram."),
    _P + "Program.DuplicateSourceFiles": _later(
        "Phase 5", "Its pinned callers are the project system (project/project.go:426, project/snapshot.go:815)."),
    _P + "Program.GetUnresolvedImports": _later(
        "Phase 5", "Type acquisition's unresolved imports, read by the project system (project/project.go:531, 556)."),
    _P + "Program.extractUnresolvedImports": _later(
        "Phase 5", "GetUnresolvedImports's lazy computation (program.go:522)."),
    _P + "Program.extractUnresolvedImportsFromSourceFile": _later(
        "Phase 5", "extractUnresolvedImports's per-file helper (program.go:529)."),
    _P + "Program.IsGlobalTypingsFile": _later(
        "Phase 5", "Its one pinned caller is auto-import (ls/autoimport/registry.go:1135)."),
    _P + "Program.HasSameFileNames": _later(
        "Phase 5", "Its one pinned caller is the project system (project/project.go:461)."),
    _P + "Program.GetLibFileFromReference": _later(
        "Phase 5", f"Its one pinned caller is {_PHASE5_LS} (ls/utilities.go:1330)."),
    _P + "Program.GetResolvedTypeReferenceDirectiveFromTypeReferenceDirective": _later(
        "Phase 5", f"Its pinned callers are {_PHASE5_LS} (ls/importTracker.go:735, ls/utilities.go:1321)."),
    _P + "Program.getModeForTypeReferenceDirectiveInFile": _later(
        "Phase 5", "Its one caller is GetResolvedTypeReferenceDirectiveFromTypeReferenceDirective (program.go:2102)."),
    _P + "Program.ResolvedPackageNames": _later("Phase 5", "Auto-import (ls/autoimport/util.go:146)."),
    _P + "Program.UnresolvedPackageNames": _later("Phase 5", "Auto-import (ls/autoimport/util.go:147)."),
    _P + "Program.DeepImportPackageNames": _later("Phase 5", "Auto-import (ls/autoimport/registry.go:824)."),
    _P + "Program.collectPackageNames": _later(
        "Phase 5", "The lazy computation behind the three package-name sets of auto-import (program.go:2140-2148)."),
    _P + "Program.IsLibFile": _later("Phase 5", f"Its one pinned caller is {_PHASE5_LS} (ls/symbols.go:616)."),
    _P + "Program.HasTSFile": _later("Phase 5", f"Its one pinned caller is {_PHASE5_LS} (ls/symbols.go:558)."),
    # host.go
    "tsc/internal/compiler/host.go:NewCachedFSCompilerHost": _pending(
        "X1", "The tsc and build commands' cached host (execute/tsc.go:302, 360; execute/build/orchestrator.go:764); "
              "C7 handed it to Phase 4 (data/phase2/c7-audit.json); plan X1."),
    "tsc/internal/compiler/host.go:NewCompilerHost": _equivalent(
        _LOADER, "pub fn load_with_content_mapper_project(",
        "C7's recorded disposition (data/phase2/c7-audit.json): the Rust compiler host carries no mapper project; "
        "the caller hands the project to the program load, where the pin reads it back from the host. Its file "
        "system, current directory and default library path are ProgramOptions fields."),
    "tsc/internal/compiler/host.go:compilerHost.ContentMapperProject": _equivalent(
        _LOADER, "pub fn load_with_content_mapper_project(",
        "C7's recorded disposition (data/phase2/c7-audit.json): the project the pinned host returns is the one "
        "passed to load_with_content_mapper_project."),
    "tsc/internal/compiler/host.go:compilerHost.FS": _equivalent(
        _LOADER, "pub host: Arc<dyn FileSystem>,",
        "The Rust program takes the host's file system as ProgramOptions.host; Program::host (marked Program.Host) "
        "returns it."),
    "tsc/internal/compiler/host.go:compilerHost.DefaultLibraryPath": _equivalent(
        _LOADER, "pub default_library_path: JsString,",
        "The Rust program takes the host's default library path as ProgramOptions.default_library_path."),
    "tsc/internal/compiler/host.go:compilerHost.GetCurrentDirectory": _equivalent(
        _LOADER, "pub current_directory: JsString,",
        "The Rust program takes the host's current directory as ProgramOptions.current_directory."),
    # fileInclude.go
    "tsc/internal/compiler/fileInclude.go:FileIncludeReason.asIndex": _equivalent(
        _REASON, "pub(crate) enum IncludeReasonData {",
        "A type assertion on the reason's data. The Rust reason is an enum whose Root variant carries the index, "
        "destructured by each match arm that reads it (compute_diagnostic, compute_related_info)."),
    "tsc/internal/compiler/fileInclude.go:FileIncludeReason.asLibFileIndex": _equivalent(
        _REASON, "index: Option<usize>,",
        "The checked assertion's (index, ok) is the Lib variant's Option<usize>, matched as Some and None."),
    "tsc/internal/compiler/fileInclude.go:FileIncludeReason.asReferencedFileData": _equivalent(
        _REASON, 'panic!("reason is not a referenced file");',
        "Its one caller, getReferencedLocation, is compute_location, which destructures the four referenced-file "
        "variants and panics otherwise, as the pin's assertion does."),
    "tsc/internal/compiler/fileInclude.go:FileIncludeReason.asAutomaticTypeDirectiveFileData": _equivalent(
        _REASON, "IncludeReasonData::AutomaticType { name, package_id } => {",
        "A type assertion on the reason's data; the AutomaticType variant carries the name and package id, "
        "destructured by the match arms of compute_diagnostic and compute_related_info."),
    # fileloader.go
    "tsc/internal/compiler/fileloader.go:redirectsFile.FileName": _equivalent(
        _REASON, "program.redirect_file_names.get(file_path)",
        "ast.HasFileName on the redirect record. The Rust program keeps each redirect's file name in "
        "Program.redirect_file_names by path, read where the include processor explains a redirect "
        "(includeprocessor.go:133)."),
    "tsc/internal/compiler/fileloader.go:redirectsFile.Path": _equivalent(
        _REASON, "program.redirect_file_names.get(file_path)",
        "ast.HasFileName on the redirect record; the redirect's path is the key of Program.redirect_file_names."),
    "tsc/internal/compiler/fileloader.go:processAllProgramFiles": _equivalent(
        _LOADER, "impl<'a> Loader<'a> {",
        "C7's recorded disposition (data/phase2/c7-audit.json): Loader::new and Loader::run split the pinned "
        "function; the mapper extensions reach the supported-extension set and the resolver as in the pin."),
    # filesparser.go
    "tsc/internal/compiler/filesparser.go:parseTask.FileName": _equivalent(
        _LOADER, "fn load_worker(",
        "ast.HasFileName on the parse task; load_worker (marked parseTask.load) computes the task's normalized "
        "file name, and its path, on entry."),
    "tsc/internal/compiler/filesparser.go:parseTask.Path": _equivalent(
        _LOADER, "fn load_worker(",
        "ast.HasFileName on the parse task; load_worker (marked parseTask.load) computes the task's path on entry."),
    "tsc/internal/compiler/filesparser.go:getParseTaskData": _equivalent(
        _LOADER, "self.depths.insert(key.clone(), depth);",
        "A sync.Pool allocation of the per-path task record (tasks by casing, lowest depth). The Rust loader runs "
        "its tasks in order and keeps that state in per-path maps (depths, child_tasks) in load_worker's "
        "filesParser.start section."),
    "tsc/internal/compiler/filesparser.go:putParseTaskData": _equivalent(
        _LOADER, "self.depths.insert(key.clone(), depth);",
        "Returns a pooled per-path record to the sync.Pool; the Rust loader's per-path maps need no pool."),
    # includeprocessor.go
    "tsc/internal/compiler/includeprocessor.go:updateFileIncludeProcessor": _pending(
        "X5", "Called only by Program.ReuseProgram (program.go:414)."),
    # processingDiagnostic.go
    "tsc/internal/compiler/processingDiagnostic.go:processingDiagnostic.asFileIncludeReason": _equivalent(
        _REASON, "Self::UnknownReference(reason) => {",
        "A type assertion on the diagnostic's data; the Rust processing diagnostic is an enum whose "
        "UnknownReference variant carries the reason, destructured in to_diagnostic."),
    "tsc/internal/compiler/processingDiagnostic.go:processingDiagnostic.asIncludeExplainingDiagnostic": _equivalent(
        _REASON, "Self::ExplainingFileInclude {",
        "A type assertion on the diagnostic's data; the ExplainingFileInclude variant carries the file, reason, "
        "message and arguments, destructured in to_diagnostic."),
    # projectreferencefilemapper.go
    "tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.getResolvedProjectReferences":
        _later("Phase 5", "Its one caller is Program.GetResolvedProjectReferences (program.go:215), Phase 5's."),
    "tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper."
    "rangeResolvedProjectReferenceInChildConfig":
        _later("Phase 5", "Its one caller is Program.RangeResolvedProjectReferenceInChildConfig (program.go:226), "
                          "Phase 5's."),
    "tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.getResolvedReferenceFor":
        _equivalent(_REFERENCES, "config_to_project_reference: BTreeMap<JsString, Option<usize>>,",
                    "Its one caller is the unused Program.GetResolvedProjectReferenceFor (program.go:202; "
                    "data/phase1/coverage-review.json); the configToProjectReference lookup is the mapper's map."),
    # diagnosticwriter.go: the 15 accessors and flatten helpers.
    **{f"tsc/internal/diagnosticwriter/diagnosticwriter.go:ASTDiagnostic.{name}": _equivalent(path, anchor, reason)
       for name, path, anchor, reason in (
           ("Pos", _WRITER, "file.line_and_character(self.resolved_location(d)?.loc.pos())",
            "The resolved start: callers read resolved_location(d).loc.pos() inline, the plain form here and the "
            "pretty form in pretty.rs."),
           ("End", _PRETTY, "let location = self.resolved_location(d)?.loc;",
            "The resolved end: the pretty form passes location.end() to the snippet."),
           ("Len", _PRETTY, "let location = self.resolved_location(d)?.loc;",
            "The pin passes Pos and Len to writeCodeSnippet; the Rust snippet takes the resolved start and end."))},
    **{f"tsc/internal/diagnosticwriter/diagnosticwriter.go:{kind}.{name}": _equivalent(
        path, anchor, f"One File type serves the pin's three FileLike forms (source, original text, renamed), tagged "
                      f"by FileKind; File::{builder} builds this form and {accessor} reads it.", True)
       for kind, builder in (("originalTextFile", "original"), ("renamedFile", "renamed"))
       for name, path, anchor, accessor in (
           ("FileName", _WRITER, "pub fn name(&self) -> &[u8] {", "File::name"),
           ("Text", _WRITER, "pub fn text(&self) -> &[u8] {", "File::text"),
           ("ECMALineMap", _RESOLVED, "pub fn line_map(&self) -> &[i32] {", "File::line_map"))},
    "tsc/internal/diagnosticwriter/diagnosticwriter.go:FlattenDiagnosticMessage": _equivalent(
        _RESOLVED, "pub fn flatten(&self, d: &Diagnostic, new_line: &[u8]) -> Result<Vec<u8>> {",
        "The string form of WriteFlattenedDiagnosticMessage: DiagnosticWriter::flatten (marked "
        "WriteFlattenedDiagnosticMessage) returns the flattened bytes. Its pinned callers are the compiler-test "
        "baseline (testutil/tsbaseline/error_baseline.go:101, 122); mod.rs's flattened is the default-locale form "
        "without file resolution.", True),
    "tsc/internal/diagnosticwriter/diagnosticwriter.go:WriteFlattenedASTDiagnosticMessage": _equivalent(
        _RESOLVED, "pub fn flatten(&self, d: &Diagnostic, new_line: &[u8]) -> Result<Vec<u8>> {",
        "It wraps the AST diagnostic and flattens it; DiagnosticWriter::flatten takes the AST diagnostic itself. "
        "Its one pinned caller is the language server (ls/lsconv/converters.go:628).", True),
    "tsc/internal/diagnosticwriter/diagnosticwriter.go:flattenDiagnosticMessageChain": _equivalent(
        _RESOLVED, "enum Task<'a> {",
        "The recursive chain walk is flatten's explicit stack of chain nodes with their levels, so a deep chain "
        "does not recurse on the native stack."),
    "tsc/internal/diagnosticwriter/diagnosticwriter.go:diagnosticPrefix": _equivalent(
        _WRITER, "pub fn prefix(d: &Diagnostic) -> &[u8] {",
        "The diagnostic's source, or TS when it has none.", True),
    "tsc/internal/diagnosticwriter/diagnosticwriter.go:getCategoryFormat": _equivalent(
        _WRITER, "pub fn color(value: i32) -> Result<&'static [u8]> {",
        "The category's escape: warning yellow, error red, suggestion grey, message blue; an unknown category is "
        "an error where the pin panics.", True),
    "tsc/internal/diagnosticwriter/diagnosticwriter.go:writeWithStyleAndReset": _equivalent(
        _WRITER, "pub fn styled(out: &mut Vec<u8>, bytes: &[u8], style: &[u8], pretty: bool) {",
        "Writes the style, the text and the reset; its pretty flag selects the unstyled writer the pin passes as "
        "FormattedWriter otherwise.", True),
    # execute/tsc/extendedconfigcache.go: an unmarked port in tsr_tsoptions.
    "tsc/internal/execute/tsc/extendedconfigcache.go:ExtendedConfigCache.GetExtendedConfig": _equivalent(
        "crates/tsr_tsoptions/src/extended_config.rs", "pub fn get_extended_config(",
        "The driver's concurrency-safe, permanent extended-config cache: an entry by path, parsed through "
        "ParseExtendedConfig with the cache on a miss and then kept. tsr_tsoptions::ExtendedConfigCache is the same "
        "cache, scoped to one host.", True),
    "tsc/internal/execute/tsc/extendedconfigcache.go:ExtendedConfigCache.loadOrStoreNewLockedEntry": _equivalent(
        "crates/tsr_tsoptions/src/extended_config.rs", "pub fn get_extended_config(",
        "The pin locks a new entry before publishing it, so concurrent requests for one path wait for one parse. "
        "The Rust cache parses outside its map lock and publishes the first entry inserted, which every caller "
        "then receives."),
    "tsc/internal/execute/tsc.go:fmtMain": _pending(
        "X1", "No caller at the pin: CommandLine's `-f` dispatch is commented out (execute/tsc.go:58-59). The "
              "formatting it would run is tsr_format's FormatDocument; X1 records the disposition."),
    # cmd/tsc: --lsp and --api (decision 12).
    "tsc/cmd/tsc/lsp.go:runLSP": _later(
        "Phase 5", "The --lsp entry (cmd/tsc/main.go:24); decision 12: the binary reports the mode unavailable "
                   "until Phase 5 supplies the server."),
    "tsc/cmd/tsc/lsp.go:newParentProcessWatchdog": _later(
        "Phase 5", "The language server's parent-process watchdog (lsp.go:66); decision 12."),
    "tsc/cmd/tsc/lsp.go:startParentProcessWatchdog": _later(
        "Phase 5", "The language server's parent-process watchdog (lsp.go:84, 88); decision 12."),
    "tsc/cmd/tsc/isprocessalive_unix.go:isProcessAlive": _later(
        "Phase 5", "The watchdog's liveness probe (lsp.go:107); decision 12. The X1 test "
                   "TestChildProcessCloseDoesNotWaitForLauncherDescendants also calls it (sys_unix_test.go:66), so "
                   "its Rust port needs a test-side liveness check."),
    "tsc/cmd/tsc/api.go:runAPI": _later(
        "Phase 6", "The --api entry (cmd/tsc/main.go:26); decision 12: the binary reports the mode unavailable "
                   "until Phase 6 supplies the server."),
    "tsc/cmd/tsc/api.go:parseAPIFlags": _later("Phase 6", "runAPI's flag parser (api.go:42); decision 12."),
}


def inventory(root=ROOT):
    """Pinned functions in inventory order: (id, file)."""
    with (root / "data/go-functions.tsv").open(newline="") as handle:
        rows = list(csv.reader(handle, delimiter="\t"))
    if rows[0] != ["# upstream " + PIN]:
        raise ValueError("function inventory pin differs from the reviewed Phase 4 scope")
    header = rows[1]
    file_index, id_index = header.index("file"), header.index("id")
    return [(row[id_index], row[file_index]) for row in rows[2:] if len(row) > id_index]


def ledger_entries(root=ROOT):
    ledger = tomllib.loads((root / "PORTS.toml").read_text())
    if ledger["pin"] != PIN:
        raise ValueError("ledger pin differs from the reviewed Phase 4 scope")
    return ledger["file"]


def scope_files(root=ROOT, entries=None):
    """The Phase 4 files after the decisions: ledger phase 4, source or harness, not moved out."""
    entries = ledger_entries(root) if entries is None else entries
    return sorted(entry["go"] for entry in entries
                  if entry.get("phase") == 4 and entry.get("kind") in ("source", "harness")
                  and entry["go"] not in MOVED_OUT)


def ledger_numbers(entries, functions, marked):
    """The ledger's Phase 4 files, before and after the decisions (the plan counted before)."""
    phase4 = [entry for entry in entries if entry.get("phase") == 4]
    before = [entry for entry in phase4 if entry.get("kind") in ("source", "harness")]
    after = [entry for entry in before if entry["go"] not in MOVED_OUT]
    by_file = {}
    for identity, go in functions:
        by_file.setdefault(go, []).append(identity)

    def tally(rows):
        ids = [identity for entry in rows for identity in by_file.get(entry["go"], [])]
        return {"files": len(rows), "source": sum(entry["kind"] == "source" for entry in rows),
                "harness": sum(entry["kind"] == "harness" for entry in rows),
                "lines": sum(entry.get("loc", 0) for entry in rows), "functions": len(ids),
                "marked": sum(identity in marked for identity in ids)}
    return {"ledger_phase4_files": len(phase4),
            "ledger_out_of_scope": sum(entry.get("kind") == "out-of-scope" for entry in phase4),
            "before_decisions": tally(before), "after_decisions": tally(after)}


def scope(root=ROOT, functions=None):
    """The scope's files and function ids (inventory order)."""
    files = scope_files(root)
    functions = inventory(root) if functions is None else functions
    known = {identity for identity, _ in functions}
    chosen = set(files)
    ids = [identity for identity, go in functions if go in chosen]
    return files, ids, known


def binding(values):
    return (len(values), digest(canonical(sorted(values))))


def marker_sites(root=ROOT):
    """Every marker under crates/ and tools/ with all of its sites, `path:line`."""
    sites = {}
    for base in MARKER_ROOTS:
        for path in sorted((root / base).rglob("*.rs")):
            if "target" in path.relative_to(root).parts:
                continue
            for number, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
                match = MARKER.match(line)
                if match and ":" in match.group(1):
                    sites.setdefault(match.group(1), []).append(f"{path.relative_to(root).as_posix()}:{number}")
    return sites


def resolve_site(site, root=ROOT):
    """A reviewed (path, anchor) site as `path:line`, or None when it does not resolve uniquely."""
    path, anchor = site
    target = root / path
    if not path.startswith(tuple(f"{base}/" for base in MARKER_ROOTS)) or target.suffix != ".rs" \
            or not target.is_file():
        return None
    lines = [number for number, line in enumerate(target.read_text().splitlines(), 1) if anchor in line]
    return f"{path}:{lines[0]}" if len(lines) == 1 else None


def phase4_prefixes(files):
    """Marker ids under these prefixes belong to Phase 4: its packages, and its compiler files by name."""
    directories = {go.rsplit("/", 1)[0] + "/" for go in files if not go.startswith("tsc/internal/compiler/")}
    return tuple(sorted(directories)) + tuple(f"{go}:" for go in files if go.startswith("tsc/internal/compiler/"))


def default_owner(identity, go):
    return OWNER_OVERRIDES.get(identity, group_of(go))


def build_document(root=ROOT, markers=None, reviewed=None, functions=None, multi_site=None):
    """The audit document; its `problems` list is empty when it is valid."""
    reviewed = REVIEWED if reviewed is None else reviewed
    multi_site = MULTI_SITE if multi_site is None else multi_site
    markers = marker_sites(root) if markers is None else markers
    functions = inventory(root) if functions is None else functions
    entries_ledger = ledger_entries(root)
    files, ids, known = scope(root, functions)
    problems = []
    if binding(files) != REVIEWED_FILES:
        problems.append(f"the scope's files differ from the reviewed {REVIEWED_FILES[0]}")
    if binding(ids) != REVIEWED_FUNCTIONS:
        problems.append(f"the scope's functions differ from the reviewed {REVIEWED_FUNCTIONS[0]}")
    in_scope = set(ids)
    superseded = []
    for identity in sorted(set(reviewed) - in_scope):
        problems.append(f"{identity}: reviewed, but not a Phase 4 function")
    for identity in sorted(set(OWNER_OVERRIDES) - in_scope):
        problems.append(f"{identity}: an owner override, but not a Phase 4 function")
    entries = {}
    for identity in ids:
        go = identity.split(":", 1)[0]
        sites = sorted(markers.get(identity, []))
        entry = {"group": group_of(go)}
        review = reviewed.get(identity)
        expected = multi_site.get(identity, (1, None))[0]
        if len(sites) > 1 and len(sites) != expected:
            entry |= {"status": "duplicate", "rust": sites}
            problems.append(f"{identity}: {len(sites)} port markers ({', '.join(sites)})")
        elif sites:
            entry |= {"status": "mapped", "rust": sites}
            if len(sites) > 1:
                entry["reason"] = multi_site[identity][1]
            if review and review.get("disposition") == "pending":
                # The port a pending disposition waited for has landed.
                superseded.append(identity)
            elif review:
                problems.append(f"{identity}: a port marker names it, so its reviewed disposition is stale")
        elif review:
            kind = review.get("disposition")
            reason = review.get("reason")
            if not isinstance(reason, str) or not reason.strip():
                problems.append(f"{identity}: {kind} needs a reason")
            if kind == "equivalent":
                site = resolve_site(review.get("rust") or ("", ""), root)
                if site is None:
                    problems.append(f"{identity}: equivalent site {review.get('rust')} does not resolve to one line")
                entry |= {"status": "equivalent", "rust": [site] if site else [], "reason": reason,
                          "marker_to_add": bool(review.get("marker_to_add"))}
            elif kind == "later":
                if review.get("owner") not in LATER_OWNERS:
                    problems.append(f"{identity}: later needs an owner among {', '.join(LATER_OWNERS)}")
                entry |= {"status": "later", "owner": review.get("owner"), "reason": reason}
            elif kind == "pending":
                if review.get("owner") not in CHECKPOINTS:
                    problems.append(f"{identity}: pending needs the checkpoint that ports it")
                entry |= {"status": "pending", "owner": review.get("owner"), "reason": reason}
            else:
                problems.append(f"{identity}: unknown reviewed disposition {kind!r}")
                entry |= {"status": "gap", "reason": reason}
        elif go.startswith(REVIEWED_PREFIXES):
            entry |= {"status": "gap", "reason": "no marker and no reviewed disposition"}
            problems.append(f"{identity}: no port marker and no reviewed disposition")
        else:
            entry |= {"status": "pending", "owner": default_owner(identity, go), "reason": "not yet ported"}
        entries[identity] = entry
    for identity, (count, _) in sorted(multi_site.items()):
        found = len(markers.get(identity, []))
        if identity not in in_scope:
            problems.append(f"{identity}: reviewed as multi-site, but not a Phase 4 function")
        elif found != count and found <= 1:
            # More sites than reviewed are reported above as a duplicate.
            problems.append(f"{identity}: reviewed as {count} marker sites, {found} found")
    prefixes = phase4_prefixes(files)
    unknown, test_markers = {}, {}
    for identity, sites in markers.items():
        if identity in known or not identity.startswith(prefixes):
            continue
        go = identity.split(":", 1)[0]
        (test_markers if go.endswith("_test.go") else unknown)[identity] = sorted(sites)
    for identity, sites in sorted(unknown.items()):
        problems.append(f"{identity}: a port marker names no pinned function ({', '.join(sites)})")
    by_file = {}
    for identity, entry in entries.items():
        go = identity.split(":", 1)[0]
        row = by_file.setdefault(go, {"group": entry["group"], "functions": {}})
        row["functions"][identity] = {key: value for key, value in entry.items() if key != "group"}
    for go in files:
        by_file.setdefault(go, {"group": group_of(go), "functions": {}})
    for row in by_file.values():
        row["counts"] = tally(entry["status"] for entry in row["functions"].values())
    groups = {group: {"title": GROUPS[group],
                      "files": sorted(go for go, row in by_file.items() if row["group"] == group),
                      "counts": tally(entry["status"] for entry in entries.values() if entry["group"] == group)}
              for group in GROUPS}
    totals = tally(entry["status"] for entry in entries.values())
    pending_by_checkpoint = {checkpoint: sum(1 for entry in entries.values()
                                             if entry["status"] == "pending" and entry["owner"] == checkpoint)
                             for checkpoint in CHECKPOINTS}
    later_by_owner = {owner: sum(1 for entry in entries.values()
                                 if entry["status"] == "later" and entry["owner"] == owner) for owner in LATER_OWNERS}
    return {
        "version": 1,
        "pin": PIN,
        "scope": {
            "description": ("Every pinned function of the PORTS.toml phase-4 files of kind source or harness, minus "
                            "the three files decision 4 moves out (docs/PHASE4-plan.md sections 1, 2 and 8)."),
            "files": len(files), "files_sha256": binding(files)[1],
            "functions": len(ids), "functions_sha256": binding(ids)[1],
            "moved_out": dict(sorted(MOVED_OUT.items())),
            "marker_roots": list(MARKER_ROOTS),
            "ledger": ledger_numbers(entries_ledger, functions, set(markers)),
        },
        "groups": groups,
        "totals": totals,
        "pending_by_checkpoint": pending_by_checkpoint,
        "later_by_owner": later_by_owner,
        "markers_to_add": sorted(identity for identity, entry in entries.items()
                                 if entry["status"] == "equivalent" and entry.get("marker_to_add")),
        "pending_reviews_superseded": superseded,
        "files": dict(sorted(by_file.items())),
        "unknown_markers": dict(sorted(unknown.items())),
        "test_file_markers": dict(sorted(test_markers.items())),
        "problems": problems,
        "complete": not problems and not any(totals[kind] for kind in OPEN),
    }


def tally(statuses):
    counts = {kind: 0 for kind in STATUSES} | {"total": 0}
    for status in statuses:
        counts[status] += 1
        counts["total"] += 1
    return counts


def render(document):
    return json.dumps(document, indent=1, sort_keys=True) + "\n"


def table(document):
    columns = ("total", "mapped", "equivalent", "later", "pending", "gap", "duplicate")
    lines = ["| Group | " + " | ".join(columns) + " |", "| --- |" + " ---: |" * len(columns)]
    for group, row in document["groups"].items():
        lines.append(f"| {group} {row['title']} | " + " | ".join(str(row["counts"][c]) for c in columns) + " |")
    lines.append("| all | " + " | ".join(str(document["totals"][c]) for c in columns) + " |")
    lines.append("")
    lines.append("pending by owning checkpoint: " + ", ".join(
        f"{checkpoint} {count}" for checkpoint, count in document["pending_by_checkpoint"].items() if count))
    return "\n".join(lines)


def open_summary(document):
    """The open dispositions: pending per owning checkpoint, then gaps."""
    parts = [f"{count} pending {checkpoint}" for checkpoint, count in document["pending_by_checkpoint"].items()
             if count]
    if document["totals"]["gap"]:
        parts.append(f"{document['totals']['gap']} gap")
    return ", ".join(parts)


def check(*, complete=False, root=ROOT, path=None):
    """Problems with the committed audit: stale, invalid, or (with `complete`) open."""
    document = build_document(root)
    path = AUDIT if path is None else path
    found = list(document["problems"])
    if not path.is_file() or path.read_text() != render(document):
        found.append(f"{path.relative_to(root) if path.is_relative_to(root) else path} differs from the rebuilt audit")
    if complete and not document["complete"]:
        found.append("the audit is not complete: " + open_summary(document))
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("build", "check", "summary"))
    parser.add_argument("--complete", action="store_true", help="check: also require no pending and no gap function")
    args = parser.parse_args()
    if args.command == "build":
        document = build_document()
        AUDIT.parent.mkdir(parents=True, exist_ok=True)
        AUDIT.write_text(render(document))
        print(table(document))
        for problem in document["problems"]:
            print("problem: " + problem, file=sys.stderr)
        if document["problems"]:
            raise SystemExit(1)
    elif args.command == "check":
        found = check(complete=args.complete)
        for problem in found:
            print("problem: " + problem, file=sys.stderr)
        if found:
            raise SystemExit(1)
        print("phase4 audit current" + (" and complete" if args.complete else ""))
    else:
        print(table(build_document()))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase4 audit failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
