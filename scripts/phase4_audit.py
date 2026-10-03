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
  there, and the build records the file (a line number would stale the audit
  on every edit above it); `marker_to_add` says that the site is a
  one-to-one port that should carry the marker;
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
    _P + "Program.GetIncludeReasons": _equivalent(
        _LOADER, "pub fn include_reason_paths(&self) -> impl Iterator<Item = &JsString> {",
        "Its only pinned consumer, tsctests/sys.go's program-include baseline, tests membership and iterates "
        "the map's keys without reading a reason value. The borrowed key iterator exposes exactly that view; "
        "the private map and reason records remain owned by Program."),
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
    "tsc/internal/compiler/host.go:NewCachedFSCompilerHost": _equivalent(
        "crates/tsr_execute/src/compile.rs", "let cached = Arc::new(tsr_vfs::cached::CachedFs::new(sys.fs()));",
        "The ordinary and incremental command paths wrap the System filesystem in CachedFs and pass that "
        "same instance to the incremental host and compiler load. The build host and each full watch cycle "
        "construct their own CachedFs too; current directory, default library, tracing and mapper project are "
        "passed through the Rust host/load interfaces instead of one Go compilerHost struct."),
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
        _REASON, "let location = reason.reference_location(program)?;",
        "A type assertion on the diagnostic's data; the Rust processing diagnostic is an enum whose "
        "UnknownReference variant carries the reason, destructured in to_diagnostic."),
    "tsc/internal/compiler/processingDiagnostic.go:processingDiagnostic.asIncludeExplainingDiagnostic": _equivalent(
        _REASON, "} => program.explain_file_include_with_reason(",
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
    "tsc/internal/execute/tsc.go:fmtMain": _equivalent(
        "crates/tsr_execute/src/command.rs", "pub fn command_line(",
        "No caller at the pin: its sole prospective -f dispatch is commented out in execute/tsc.go:58-59. "
        "Rust preserves that command surface: -f goes through ordinary option validation, not formatting. "
        "The existing tsr_format::format_document is the formatting algorithm, but no unreachable file-I/O "
        "wrapper is added or claimed as an executable command."),
    # execute/tsctests/readablebuildinfo.go: decoders with no caller (X0).
    **{f"tsc/internal/execute/tsctests/readablebuildinfo.go:{name}.UnmarshalJSON": _equivalent(
        "tools/phase4/tsctests/src/readablebuildinfo.rs", f"impl Encode for {rust} {{",
        "No caller at the pin: the readable build info is only marshalled (toReadableBuildInfo's json.MarshalIndent, "
        "readablebuildinfo.go:242). The unexported type is used only in readablebuildinfo.go, and the package's "
        "json.Unmarshal calls (fs.go:40, 62) decode incremental.BuildInfo, never a readable form. The harness "
        "renders the shape encode-only; the site is its marked MarshalJSON port.")
       for name, rust in (("readableBuildInfoDiagnosticsOfFile", "ReadableBuildInfoDiagnosticsOfFile"),
                          ("readableBuildInfoSemanticDiagnostic", "ReadableBuildInfoSemanticDiagnostic"),
                          ("readableBuildInfoFilePendingEmit", "ReadableBuildInfoFilePendingEmit<'_>"),
                          ("readableBuildInfoResolvedRoot", "ReadableBuildInfoResolvedRoot"))},
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


# X1/X2/X3 closure review, 2026-10-02. These sites were compared with the
# pinned implementations and callers; folded operations are not extra coverage
# claims. The source parse-cache operations now carry markers on the shared
# declaration/JSON cache and its project-local caller.
REVIEWED.update({
    'tsc/internal/execute/build/buildtask.go:BuildTask.canUpdateJsDtsOutputTimestamps': _equivalent(
        'crates/tsr_build/src/task.rs', 'let mut files = if !config.options.no_emit.is_true() && !config.options.is_incremental()',
        'The timestamp eligibility predicate is folded into update_timestamps; watch cache retention applies '
        'the same predicate. '
    ),
    'tsc/internal/execute/build/buildtask.go:BuildTask.cleanProjectOutput': _equivalent(
        'crates/tsr_build/src/task.rs', 'for file in outputs {',
        'Output deletion/input collision avoidance/dry-run recording/error diagnostics are folded into clean '
        'output iteration. '
    ),
    'tsc/internal/execute/build/buildtask.go:BuildTask.getContentMapperProject': _equivalent(
        'crates/tsr_build/src/task.rs', 'fn project(&self, o: &Orchestrator)',
        'OnceLock initializes at most one project from the session host, config name, mapper list and '
        'options; no host, unresolved config or empty mapper list yields None. The retained project_error is '
        'read by both compilation and up-to-date checking and is updated by watch refresh/identity failures. '
    ),
    'tsc/internal/execute/build/buildtask.go:BuildTask.hasConflictingBuildInfo': _equivalent(
        'crates/tsr_build/src/task.rs', 'let conflicting = upstream_state',
        'Build-info path collision check is inlined in upstream stale-status handling. '
    ),
    'tsc/internal/execute/build/buildtask.go:BuildTask.onBuildInfoEmit': _equivalent(
        'crates/tsr_build/src/task.rs', 'let dts_time = if incremental.has_changed_dts_file()',
        'write_file callback updates BuildInfoEntry under task state mutex, preserving previous dts_time '
        'unless declarations changed. '
    ),
    'tsc/internal/execute/build/buildtask.go:BuildTask.refreshContentMapperProject': _equivalent(
        'crates/tsr_build/src/watch.rs', '*lock(&task.project_error) = project.refresh().err().map(Arc::new);',
        'The refresh call and persisted refresh error are folded into dynamic mapper dependency event '
        'handling. '
    ),
    'tsc/internal/execute/build/buildtask.go:BuildTask.report': _equivalent(
        'crates/tsr_build/src/lib.rs', 'let report = std::panic::catch_unwind',
        'Each worker waits for the previous graph-order report, writes its buffer and invokes OnProgram, then '
        'releases report_done. After workers join, graph-order reduction aggregates diagnostics, maximal exit '
        'status, statistics, delete paths and build/pseudo counters; retained program owners keep diagnostic '
        'sources alive through the summary. '
    ),
    'tsc/internal/execute/build/buildtask.go:BuildTask.resetConfig': _equivalent(
        'crates/tsr_build/src/watch.rs', 'if normalized.contains_key(&self.path(task.config.as_bytes()))',
        'A config-path change marks the task dirty; extended-config and mapper-manifest changes use the same '
        'flag. Graph reconstruction bypasses dirty retained tasks, reparses the config and replaces its Arc, '
        "implementing deletion of the pin's separate resolvedReferences entry. "
    ),
    'tsc/internal/execute/build/buildtask.go:BuildTask.unblockDownstream': _equivalent(
        'crates/tsr_build/src/lib.rs', 'task.pending.store(false, Ordering::Release);',
        'Worker completion clears pending and initial_cycle then signals done, including error/panic paths. '
    ),
    'tsc/internal/execute/build/buildtask.go:BuildTask.updateWatch': _equivalent(
        'crates/tsr_build/src/watch.rs', 'fn update_watch(&self)',
        'Watch update transfers retained eligible output timestamps from previous mtime cache for every task. '
    ),
    'tsc/internal/execute/build/buildtask.go:BuildTask.waitOnUpstream': _equivalent(
        'crates/tsr_build/src/lib.rs', 'for (upstream, _) in &self.tasks[index].upstream',
        'The worker waits upstream task completion before build/clean dispatch. '
    ),
    'tsc/internal/execute/build/buildtask.go:BuildTask.writeFile': _equivalent(
        'crates/tsr_build/src/task.rs', 'let write_file = |file: &[u8], text: &[u8], data:',
        'The shared emit callback performs actual write, build-info entry update and watch-only output '
        'timestamp caching on successful writes. '
    ),
    'tsc/internal/execute/build/compilerHost.go:compilerHost.GetContentMappedSourceFiles': _equivalent(
        'crates/tsr_compiler/src/loader.rs', 'fn content_mapped_source_files(',
        'Build passes its project to the shared loader host operation: absent project returns '
        'ProjectUnavailable before reading; unreadable file returns no source; transform-and-parse errors '
        'propagate; supplemental filename collisions are checked against the filesystem. The file-loader '
        'wrapper separately owns failure-budget diagnostics. '
    ),
    'tsc/internal/execute/build/compilerHost.go:compilerHost.GetResolvedProjectReference': _equivalent(
        'crates/tsr_build/src/graph.rs', 'impl tsr_compiler::ResolvedProjectReferenceProvider for Orchestrator',
        'Compiler load services return the exact shared Arc config retained by the build graph; no second '
        'parse or identity substitution. '
    ),
    'tsc/internal/execute/build/compilerHost.go:compilerHost.Trace': _equivalent(
        'crates/tsr_build/src/task.rs', 'tsr_tsc::report_resolution_trace(&program, &trace);',
        'Shared loader buffers actual resolver trace; build flushes it with its per-task '
        'writer/locale/testing trace reporter before diagnostics. '
    ),
    'tsc/internal/execute/build/host.go:host.ContentMapperProject': _equivalent(
        'crates/tsr_build/src/host.rs', 'pub(crate) struct BuildHost {',
        'Base BuildHost implements only incremental Host, so invalid mapper-project calls on the base host '
        'are excluded by the Rust trait boundary; per-project CompilerHost exposes mapper project. '
    ),
    'tsc/internal/execute/build/host.go:host.DefaultLibraryPath': _equivalent(
        'crates/tsr_build/src/host.rs', 'pub library: JsString,',
        'Base host stores actual system library path; CompilerHost and loader read the field directly. '
    ),
    'tsc/internal/execute/build/host.go:host.GetContentMappedSourceFiles': _equivalent(
        'crates/tsr_build/src/host.rs', 'impl tsr_incremental::Host for BuildHost {',
        'Base host has no mapper parse method in its trait; only project-specific load services can invoke '
        'mapped loading, excluding the native unreachable-project wrapper. '
    ),
    'tsc/internal/execute/build/host.go:host.GetCurrentDirectory': _equivalent(
        'crates/tsr_build/src/host.rs', 'pub cwd: JsString,',
        'Base host stores actual current directory; CompilerHost and loader read the field directly. '
    ),
    'tsc/internal/execute/build/host.go:host.GetResolvedProjectReference': _equivalent(
        'crates/tsr_build/src/graph.rs', 'let result = cache.read_config_file(',
        'Graph creation parses each normalized config path once through shared extended-config entries and '
        'wrapped command-line options, records elapsed config time, and retains absent as well as present '
        'results on its task. Project loads receive that exact Arc through ResolvedProjectReferenceProvider. '
    ),
    'tsc/internal/execute/build/host.go:host.Trace': _equivalent(
        'crates/tsr_build/src/host.rs', 'impl tsr_incremental::Host for BuildHost {',
        'Base-host trace call is excluded by trait boundary; tracing is installed only for project '
        'compilation. '
    ),
    'tsc/internal/execute/build/host.go:host.loadOrStoreMTime': _equivalent(
        'crates/tsr_build/src/host.rs', 'pub fn m_time(&self',
        'mtime misses use actual stat and load_or_store; old-cache transfer is folded into update_watch '
        'before source status evaluation; native store=false branch has no caller at pin. '
    ),
    'tsc/internal/execute/build/host.go:host.storeMTimeFromOldCache': _equivalent(
        'crates/tsr_build/src/watch.rs', 'if let Some(time) = previous.get(&path)',
        'Transfers eligible existing output timestamp from previous cache into new cycle cache. '
    ),
    'tsc/internal/execute/build/orchestrator.go:Orchestrator.Watch': _equivalent(
        'crates/tsr_build/src/watch.rs', 'pub fn start(ctx: &Context, options: Options)',
        'Build watch setup is folded into free start: initial build, backend/debug setup, locked watch/cache '
        'reconciliation, native run loop only outside testing, retained watcher. '
    ),
    'tsc/internal/execute/build/orchestrator.go:Orchestrator.addWatchDir': _equivalent(
        'crates/tsr_build/src/watch.rs', 'let add = |desired: &mut DirWatchSet, directory: &[u8]|',
        'The local add closure checks coverage and CanWatchDirectory before insertion; package-directory '
        'ancestry applies the same predicate. Configured Phase 4 mappers have no ContributionID; filtering '
        'inferred-project contributions is a Phase 5 precondition when that representation is added. '
    ),
    'tsc/internal/execute/build/orchestrator.go:Orchestrator.buildOrCleanProject': _equivalent(
        'crates/tsr_build/src/lib.rs', 'if options.clean.is_true() {',
        'Worker initializes TaskResult via build/clean functions and performs dispatch and ordered report in '
        'same worker scope. '
    ),
    'tsc/internal/execute/build/orchestrator.go:Orchestrator.createBuildTasks': _equivalent(
        'crates/tsr_build/src/graph.rs', 'while !pending.is_empty() {',
        'Graph batches parse newly discovered config paths once, reuse clean task Arc identities, reset '
        'adjacency while constructing Node records, carry prior build-info into dirty replacements and close '
        'discarded mapper projects. Root traversal later supplies stable postorder reporting independently of '
        'parsing completion order. '
    ),
    'tsc/internal/execute/build/orchestrator.go:Orchestrator.createBuilderStatusReporter': _equivalent(
        'crates/tsr_build/src/lib.rs', 'fn status_report(',
        'Builder status reporter factory plus immediate diagnostic reporting are folded together; caller '
        'explicitly supplies system or task writer. '
    ),
    'tsc/internal/execute/build/orchestrator.go:Orchestrator.getTask': _equivalent(
        'crates/tsr_build/src/lib.rs', 'fn index(&self, config: &[u8])',
        'Graph path lookup returns stable task index, with same missing-task panic, rather than pointer. '
    ),
    'tsc/internal/execute/build/orchestrator.go:Orchestrator.getWriter': _equivalent(
        'crates/tsr_build/src/task.rs', 'pub output: Arc<Buffer>,',
        'TaskResult owns its output buffer; each reporter caller explicitly selects task buffer or system '
        'writer, eliminating nullable-task writer dispatch. '
    ),
    'tsc/internal/execute/build/orchestrator.go:Orchestrator.rangeTask': _equivalent(
        'crates/tsr_build/src/lib.rs', 'let current = AtomicUsize::new(0);',
        'build_or_clean embeds bounded atomic task queue with singleThreaded/builders/default4 policies. '
        'Other task loops run sequentially because they only update retained graph/cache state. Scheduler '
        'limit tests cover 1/2/4. '
    ),
    'tsc/internal/execute/build/orchestrator.go:Orchestrator.resetCaches': _equivalent(
        'crates/tsr_build/src/lib.rs', 'pub fn reset_caches(&self)',
        'reset_caches clears filesystem and shared source entries and resets config durations. '
        'Extended-config entries are scoped to graph construction and already dropped; resolved project '
        'configs deliberately remain on retained tasks, matching the pin. '
    ),
    'tsc/internal/execute/build/orchestrator.go:orchestratorResult.report': _equivalent(
        'crates/tsr_build/src/lib.rs', 'fn report_summary(',
        'Summary dispatch is split into report_summary, dry-delete list rendering, report_statistics, all '
        'called after graph-order aggregation. '
    ),
    'tsc/internal/execute/build/parseCache.go:parseCache.delete': _equivalent(
        'crates/tsr_build/src/graph.rs', 'previous.filter(|task| !task.dirty.load(Ordering::Acquire))',
        'Dirty configs bypass retained graph task cache; no distinct resolved-reference parseCache remains. '
    ),
    'tsc/internal/execute/build/parseCache.go:parseCache.reset': _equivalent(
        'crates/tsr_build/src/lib.rs', 'pub fn reset_caches(&self)',
        'The source cache is cleared by reset_caches at watch-cycle boundaries; the extended-config parse '
        'cache is lexical to graph construction, so its entries drop when graph creation completes. Resolved '
        'project configs remain retained on graph tasks until invalidated. '
    ),
    'tsc/internal/execute/build/parseCache.go:parseCache.store': _equivalent(
        'crates/tsr_build/src/watch.rs', '.replace_config(config);',
        'Only pin caller replaces reloaded resolved configuration; Rust stores new Arc config on graph task '
        'after root-file reload and retires the lazily prepared project-reference view. '
    ),
    'tsc/internal/execute/build/uptodatestatus.go:upToDateStatus.inputOutputFileAndTime': _equivalent(
        'crates/tsr_build/src/status.rs', 'pub has_times: bool,',
        'Flattened Status carries times and presence bit; callers inspect bit instead of downcasting Go any. '
    ),
    'tsc/internal/execute/build/uptodatestatus.go:upToDateStatus.inputOutputName': _equivalent(
        'crates/tsr_build/src/status.rs', 'pub output: JsString,',
        'Flattened Status stores input/output fields directly; no runtime any downcast needed. '
    ),
    'tsc/internal/execute/build/uptodatestatus.go:upToDateStatus.oldestOutputFileName': _equivalent(
        'crates/tsr_build/src/status.rs', 'pub output: JsString,',
        'The pin accepts only UpToDate or pseudo-build states and extracts output from one of three payload '
        'shapes. update_downstream matches those states before reading the flattened output field; each '
        'corresponding constructor stores the same oldest-output filename there. There is no public '
        'downcasting accessor that could be called with another state. '
    ),
    'tsc/internal/execute/build/uptodatestatus.go:upToDateStatus.upstreamErrors': _equivalent(
        'crates/tsr_build/src/status.rs', 'pub ref_has_upstream_errors: bool,',
        'Flattened upstream status carries reference in input and the upstream-error flag directly. '
    ),
    'tsc/internal/execute/tsc.go:getContentMapperProject': _equivalent(
        'crates/tsr_execute/src/compile.rs', '    let project = mapper_host',
        'Folded into compilation setup: no host or empty mapper list yields None; otherwise host.project '
        'receives config name, complete mapper list and compiler options. MapperSession closes the retained '
        'project before closing its session host on normal, error and unwind exits. '
    ),
    'tsc/internal/execute/tsc.go:getTraceFromSys': _equivalent(
        'crates/tsr_execute/src/compile.rs', '        &tsc::get_trace_with_writer_from_sys(',
        'The wrapper only forwards sys.Writer, locale and testing to GetTraceWithWriterFromSys. The combined '
        'normal/incremental compilation path invokes that function with those three arguments directly, then '
        'flushes the actual resolver events through it. '
    ),
    'tsc/internal/execute/tsc/compile.go:NewContentMapperHost': _equivalent(
        'crates/tsr_execute/src/compile.rs', '    let mapper_host = config.options.run_external_code.is_true().then(|| {',
        'Compilation setup creates a session host only for RunExternalCode, passing the context, System '
        'spawner, parsed option locale and environment-controlled serialized stderr logger. Build-session and '
        'compiler-watch setup apply the same gate and arguments and retain their host across projects/cycles. '
    ),
})


_MACOS = "crates/tsr_fswatch/src/macos.rs"
_MACOS_NATIVE = "crates/tsr_fswatch/src/macos/native.rs"
# Reviewed X4 adaptations: typed framework bindings and ownership replace the
# pin's Go runtime/assembly bridge. Every anchor is a live operation, not a
# comment claiming an implementation exists.
REVIEWED.update({
    "tsc/internal/fswatch/fsevents_darwin.go:init#1": _equivalent(
        "crates/tsr_fswatch/src/watcher.rs", "Kind::Fsevents => crate::macos::new(),",
        "The target-specific factory is an explicit backend match arm instead of a Go init mutation. "
        "FsEventsBackend::sequence supplies FSEventsGetCurrentEventId through the Backend trait."),
    "tsc/internal/fswatch/fsevents_darwin.go:fsEventsBackend.start": _equivalent(
        _MACOS, "Ok(Arc::new(FsEventsBackend::default()))",
        "Go start only signals readiness; this backend has no event-loop startup. The synchronous Rust factory "
        "returns the ready backend, and native streams are started by add_many before that operation returns."),
    "tsc/internal/fswatch/fsevents_darwin.go:checkWatcher": _equivalent(
        _MACOS, "let metadata = std::fs::metadata(os_path(&watch.physical_dir))?;",
        "Folded into add_many's pre-mutation validation of every physical directory: follow the root symlink, "
        "propagate stat errors and reject non-directories with ENOTDIR. Common watcher registration supplies "
        "the logical request context."),
    "tsc/internal/fswatch/fsevents_darwin.go:fsEventsBackend.startStreams": _equivalent(
        _MACOS, "let streams = match start_streams(&state.active_watches(), Stream::new) {",
        "Go's method only forwards watches and b.startStream. Both Rust subscription and removal call the "
        "ported start_streams with Stream::new directly."),
    "tsc/internal/fswatch/fsevents_darwin.go:stopFSEventsStreams": _equivalent(
        _MACOS, "state.streams.clear();",
        "Vec<Stream> owns each native stream. Clearing or replacing the vector runs Stream::drop for every "
        "element, including partially constructed chunk sets on error; Drop stops, invalidates, drains and "
        "releases each stream once."),
    "tsc/internal/fswatch/fsevents_darwin.go:fsEventsBackend.subscribe": _equivalent(
        "crates/tsr_fswatch/src/watcher.rs", "if let Err(error) = backend.add_many(&added) {",
        "The one-watch wrapper only delegates to subscribeMany. Common Rust registration sends both single "
        "and batched requests through Backend::add_many, whose FSEvents implementation is the marked port."),
    **{f"tsc/internal/fswatch/fsevents_darwin_ffi.go:{name}": _equivalent(
        _MACOS_NATIVE, anchor, reason) for name, anchor, reason in (
        ("syscall_syscall6", "unsafe extern \"C-unwind\" fn callback(",
         "Go's runtime ABI trampoline is unnecessary with the typed objc2 framework imports and Rust C ABI "
         "callback. The compiler supplies native argument passing, including FSEventStreamCreate's float "
         "latency; callback catches panics and aborts before one can cross the framework boundary."),
        ("cfRelease", "fn cf_string(bytes: &[u8]) -> Option<CFRetained<CFString>> {",
         "CFRetained owns CFString/CFMutableString/CFArray creation results and releases them on every return "
         "path. Callback CFArray/CFString values are borrowed only until callback return, requiring no retain."),
        ("cfArrayCreate", "            CFArray::new(",
         "Typed CFArray::new calls the same framework constructor with null element callbacks; the owned "
         "CFString vector stays alive through FSEventStreamCreate, which copies the paths."),
        ("cfArrayGetValueAtIndex", "cf_string_to_nfc(unsafe { paths.get_unchecked(index as isize) })",
         "The typed CFArray accessor performs the same indexed lookup after the callback checks the array "
         "length. The immutable borrowed CFString is normalized before callback return."),
        ("cfStringCreateMutableCopy", "if let Some(normalized) = CFMutableString::new_copy(None, 0, Some(value)) {",
         "The binding calls CFStringCreateMutableCopy with the same allocator/capacity and owns the result; "
         "null preserves the pin's fallback to the original string."),
        ("cfStringNormalize", "CFMutableString::normalize(Some(&normalized), CFStringNormalizationForm::C);",
         "The typed binding calls CFStringNormalize with canonical composition, on an exclusively owned copy."),
        ("cfStringGetLength", "CFString::maximum_size_for_encoding(value.length(), UTF8)",
         "The typed CFString length method calls CFStringGetLength for the UTF-8 output-buffer calculation."),
        ("cfStringGetMaximumSizeForEncoding", "CFString::maximum_size_for_encoding(value.length(), UTF8)",
         "The typed binding computes the same maximum UTF-8 byte capacity; checked_add reserves the trailing NUL."),
        ("cfStringGetCString", "if !unsafe { value.c_string(bytes.as_mut_ptr().cast(), size, UTF8) } {",
         "The binding calls CFStringGetCString into the owned buffer with its exact capacity and UTF-8 encoding; "
         "false returns empty, and success trims the first NUL as in the pin."),
        ("isASCII", "    if path.is_ascii() {",
         "The byte-slice ASCII predicate performs the same all-bytes-below-0x80 test before any CF allocation."),
        ("cfStringNormalizedToGo", "if let Some(normalized) = CFMutableString::new_copy(None, 0, Some(value)) {",
         "Folded into cf_string_to_nfc: make an owned mutable copy, normalize to C, convert to UTF-8, release "
         "the copy and fall back to the original contents if allocation or conversion fails."),
        ("dispatchQueueCreate", "let queue = DispatchQueue::new(\"typescript.fswatch.fsevents.stream\", None);",
         "dispatch2 creates the same named per-stream serial queue and returns an owning DispatchRetained."),
        ("dispatchRelease", "    queue: DispatchRetained<DispatchQueue>,",
         "DispatchRetained releases the queue when Stream fields drop, after Stop/Invalidate, the synchronous "
         "queue barrier and FSEventStreamRelease. No copied integer queue handle survives that owner."),
        ("dispatchSync", "self.queue.exec_sync(|| {});",
         "dispatch2's synchronous no-op on the serial queue is the same teardown barrier as dispatch_sync_f."),
        ("fsEventStreamCreate", "            FSEventStreamCreate(",
         "The typed framework call supplies the native C callback, stable boxed context, path array, SinceNow, "
         "0.001 latency and UseCFTypes|FileEvents directly, replacing the arch-specific argument trampoline."),
        ("fsEventStreamSetDispatchQueue", "FSEventStreamSetDispatchQueue(stream, Some(&result.queue));",
         "Direct typed framework call before start; the owning Stream retains the queue through teardown."),
        ("fsEventStreamStart", "if !unsafe { FSEventStreamStart(stream) } {",
         "Direct typed framework call with the same false-result error; a failed start still invalidates/releases."),
        ("fsEventStreamFlushSync", "FSEventStreamFlushSync(stream);",
         "Direct typed framework call after successful start, before returning the subscription."),
        ("fsEventStreamStop", "FSEventStreamStop(self.stream);",
         "Direct typed framework call in the unique owner's destructor, before invalidation and queue draining."),
        ("fsEventStreamInvalidate", "FSEventStreamInvalidate(self.stream);",
         "Direct typed framework call during teardown, before waiting for the serial callback queue."),
        ("fsEventStreamRelease", "FSEventStreamRelease(self.stream);",
         "Direct typed framework call after callbacks finish and before callback context/queue fields drop."),
        ("libcFree", "std::slice::from_raw_parts(flags.as_ptr(), count),",
         "Only Go's assembly payload copies need libcFree. Rust borrows native flags/IDs/path values for the "
         "callback duration and processes them synchronously; its owned path Vec values use Rust drop."),
        ("fsEventsCallbackPayload.close", "process_events(&context.watches, events, |path| {",
         "There is no retained/copied assembly payload: the callback borrows framework arrays and classifies "
         "before returning. Local Rust path buffers drop normally; native arrays remain framework-owned."),
        ("newStreamCallback", "let mut context = Box::new(CallbackContext { watches });",
         "A stable boxed immutable watch snapshot and owned per-stream serial queue replace the Go pinner, "
         "assembly write pipe and event-loop goroutine. FSEvents invokes the typed C callback directly."),
        ("streamCallback.waitDispatchQueue", "self.queue.exec_sync(|| {});",
         "The synchronous serial-queue barrier waits for callback classification as well as callback return, "
         "since Rust does not hand a payload to a second Go worker."),
        ("streamCallback.close", "impl Drop for Stream {",
         "Stream's unique owner stops and invalidates the stream, waits for all classification on its serial "
         "queue, releases the stream, then drops queue/context. No pipe or event-loop goroutine remains to close."),
        ("streamCallback.eventLoop", "process_events(&context.watches, events, |path| {",
         "Classification runs directly in the serial C callback; there is no Go ABI handoff pipe. The shared "
         "debouncer still delivers user callbacks outside this queue, preserving callback self-close safety."),
    )},
})


# X4 native lifecycle and platform abstraction review, 2026-10-02.
# Rust typed fields/ownership fold the listed Go operations at these checked sites.
REVIEWED.update({
    'tsc/internal/fswatch/canonicalize_darwin.go:canonicalizePath': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'return crate::macos::canonicalize(path);',
        'The macOS arm delegates to the reviewed CoreFoundation NFC normalization, preserving the native '
        'canonicalizePath rule.'),
    'tsc/internal/fswatch/canonicalize_other.go:canonicalizePath': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'pub(crate) fn canonicalize(path: &[u8]) -> Vec<u8> {',
        'The non-macOS arm returns the path bytes unchanged; allocating the returned byte vector replaces Go '
        'string value ownership.'),
    'tsc/internal/fswatch/watcher.go:WithIgnore': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'pub ignore: Option<Ignore>,',
        'The typed WatchOptions.ignore field carries the caller filter directly; registration copies it into each '
        'logical callback. There is no Go WatchOption interface object or apply call.'),
    'tsc/internal/fswatch/watcher.go:ignoreOption.applyWatchOption': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'pub ignore: Option<Ignore>,',
        'The typed WatchOptions.ignore field carries the caller filter directly; registration copies it into each '
        'logical callback. There is no Go WatchOption interface object or apply call.'),
    'tsc/internal/fswatch/watcher.go:WithRecursive': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'pub recursive: bool,',
        'The typed WatchOptions.recursive field represents applying WithRecursive directly. Registration and '
        'filtering read this flag, so no interface constructor/apply helper is needed.'),
    'tsc/internal/fswatch/watcher.go:recursiveOption.applyWatchOption': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'pub recursive: bool,',
        'The typed WatchOptions.recursive field represents applying WithRecursive directly. Registration and '
        'filtering read this flag, so no interface constructor/apply helper is needed.'),
    'tsc/internal/fswatch/watcher.go:watcher.unexported': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'pub struct Watcher(Arc<Owner>);',
        'Go seals its Watcher interface with this no-op method. Rust exposes a concrete Watcher with private '
        'fields; external implementations/construction are impossible without the method.'),
    'tsc/internal/fswatch/watcher.go:fallbackWatcher.unexported': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'pub struct Watcher(Arc<Owner>);',
        'Go seals its Watcher interface with this no-op method. Rust exposes a concrete Watcher with private '
        'fields; external implementations/construction are impossible without the method.'),
    'tsc/internal/fswatch/watcher.go:watch.unexported': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'pub struct Watch {',
        'Go seals its Watch interface with a no-op method; Rust uses the concrete Watch with private owner, '
        'directory and id fields.'),
    'tsc/internal/fswatch/watcher.go:watcher.getImpl': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'if state.backend.is_none() {',
        'Backend creation happens once under the owner state lock. Successful synchronous descriptor/worker '
        'creation publishes the backend, while failure leaves the slot empty for retry; no Go started-channel '
        'handshake is required.'),
    'tsc/internal/fswatch/watcher.go:watcher.canShareRecursiveDirWatches': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'let covering = if self.0.kind == Kind::Fsevents {',
        'The only eligible backend is FSEvents, exactly the pin. The test is inlined into registration rather than '
        'duplicated in a predicate method.'),
    'tsc/internal/fswatch/watcher.go:watcher.findCoveringRecursiveWatchLocked': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', '.max_by_key(|watch| watch.dir.len())',
        'Registration selects the deepest recursive watcher covering both the logical and physical paths under the '
        'owner mutex.'),
    'tsc/internal/fswatch/watcher.go:watcher.findConsolidationDirLocked': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'if count >= 10 {',
        'Registration walks ancestor directories, checks the physical hierarchy and consolidates at the native '
        'threshold of ten existing/new roots.'),
    'tsc/internal/fswatch/watcher.go:watcher.keyForDirWatch': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', '.find(|watch| watch.dir == root && watch.recursive == recursive)',
        "Rust compares (directory, recursive) directly instead of constructing Go's string key with a NUL- "
        'recursive suffix; the same pair identifies the shared watch.'),
    'tsc/internal/fswatch/watcher.go:watcher.getOrCreateDirWatch': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'let watch = Arc::new(DirWatch {',
        'Registration performs covering/consolidation lookup, tuple-key lookup and new directory state allocation '
        'under one owner lock.'),
    'tsc/internal/fswatch/watcher.go:newDirWatch': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'debounce.add(&watch);',
        'Registration initializes paths, events, callback state and the weak debounce link in the adjacent '
        'DirWatch literal, then registers the callback exactly once.'),
    'tsc/internal/fswatch/watcher.go:watcher.removeDirWatch': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'state.dirs.retain(|watch| !Arc::ptr_eq(watch, &self.dir));',
        'Final logical-watch close removes the exact Arc identity from the owner table after backend removal; Rust '
        'need not reconstruct and recheck a string map key.'),
    'tsc/internal/fswatch/watcher.go:dirWatch.unref': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'if self.dir.unwatch(self.id) {',
        'Close performs the native last-callback check, backend removal and owner removal together. Retained Watch '
        'values preserve ownership without Go explicit unref bookkeeping.'),
    'tsc/internal/fswatch/watcher.go:validateWatchDirectory': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'if !metadata.is_dir() {',
        'Batch registration validates that each canonicalized directory exists and is a directory before applying '
        'any subscription; callback nil is unrepresentable in the Rust API.'),
    'tsc/internal/fswatch/watcher.go:callback.mapEvent': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'event.path = rebase_path(&physical, &cb.physical, &cb.dir);',
        "The per-callback event loop rebases physical paths to the subscriber's logical path before filtering; no "
        'detached callback method is needed.'),
    'tsc/internal/fswatch/watcher.go:callback.eventPhysicalPath': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'let physical = self.physical_path(&event.path);',
        'Callback delivery obtains the backend watch root through its enclosing DirWatch and maps from that '
        'logical root to its physical root. Go stores those same two roots in each callback.'),
    'tsc/internal/fswatch/watcher.go:fallbackWatcher.Name': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'Kind::Fanotify => "fanotify",',
        'The fallback is represented by Kind::Fanotify in the same Watcher, so the primary name is already the '
        'common name result.'),
    'tsc/internal/fswatch/watcher.go:fallbackWatcher.Available': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'Kind::Fanotify => crate::linux::fanotify_available(),',
        'The fallback wrapper shares the common Watcher implementation and reports primary fanotify availability, '
        'not secondary availability.'),
    'tsc/internal/fswatch/watcher.go:fallbackWatcher.HasFastRecursiveBackend': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'matches!(self.0.kind, Kind::Fsevents | Kind::Windows)',
        'The fallback uses the common Kind::Fanotify discriminator and therefore returns false, as its primary '
        'backend does.'),
    'tsc/internal/fswatch/watcher.go:fallbackWatcher.WatchDirectory': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'pub fn watch_directory(',
        'The single-directory wrapper builds a one-row request and enters the same WatchDirectories fallback route '
        'as native.'),
    'tsc/internal/fswatch/watcher.go:fallbackWatcher.WatchFile': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'file_callback(path, callback),',
        'The file wrapper attaches the same file filter to the parent-directory request; the fallback dispatch '
        'occurs inside that request and returns the secondary subscription when needed.'),
    'tsc/internal/fswatch/watcher.go:dirWatch.destroyDebounce': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'state.dirs.retain(|watch| !Arc::ptr_eq(watch, &self.dir));',
        'Unwatch first empties logical callbacks. The debounce registry holds only Weak directory references; '
        'later triggers cannot deliver a removed callback, and the final owner shuts down the debounce worker. '
        'Dead weak entries are removed on delivery instead of explicit map deregistration.'),
    'tsc/internal/fswatch/watcher.go:watcherBase.watchAdd': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'backend.add_many(&added)',
        'Only watchAddMany is called on supported native backends; the one-element helper has no pinned caller. '
        'Rust dispatches all additions through the batch Backend contract.'),
    'tsc/internal/fswatch/watcher.go:watcherBase.watchAddMany': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'if let Err(error) = backend.add_many(&added) {',
        'Registration supplies unique newly allocated directory identities to backend batch-add. Linux removes '
        'previously added subscriptions on failure; FSEvents validates/builds replacement streams atomically. '
        'Owner rollback removes each logical callback and newly added root.'),
    'tsc/internal/fswatch/watcher.go:watcherBase.watchRemove': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'let _ = backend.remove(&self.dir);',
        'Final callback close removes the native subscription while holding the owner state lock; the public Close '
        'result deliberately ignores teardown errors, as the pin does.'),
    'tsc/internal/fswatch/watcher.go:watcherBase.shutdown': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'fn shutdown(&self);',
        'The native base has a no-op shutdown default. Rust requires each sealed Backend to supply shutdown, '
        'implemented by the Linux wake/join and macOS stream teardown paths.'),
    'tsc/internal/fswatch/watcher.go:watcherBase.init': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'state: Mutex::new(State::default()),',
        'Owner and backend constructors initialize subscription tables and synchronization directly; Rust Backend '
        'trait dispatch replaces the self-reference of Go watcherBase.'),
    'tsc/internal/fswatch/watcher.go:watcherBase.notifyStarted': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'state.backend = Some(self.backend()?);',
        'Backend construction opens required descriptors before returning; the owner publishes only after '
        'successful construction. This synchronous return is the start barrier rather than a goroutine started '
        'channel.'),
    'tsc/internal/fswatch/watcher.go:watcherBase.run': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'fn backend(&self) -> Result<Arc<dyn Backend>, Error> {',
        'The factory returns Result only after initializing the native backend, with the Linux read worker spawned '
        'inside its constructor and FSEvents streams established on subscribe. Rust Result carries startup refusal '
        'without a started channel.'),
    'tsc/internal/fswatch/watcher.go:watcherBase.handleStartError': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'state.backend = Some(self.backend()?);',
        'Synchronous startup errors propagate without publication, and registration has not installed callbacks '
        'yet. Later Linux worker errors notify existing watches in linux.rs; native C FSEvents callbacks have '
        'their own unwind boundary.'),
    'tsc/internal/fswatch/watcher.go:watcherBase.handleWatcherError': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'pub(crate) fn notify_error(&self, error: Error) {',
        'The only pinned caller is windowsSubscription.fatal (windows.go), outside Phase 4 targets. Supported '
        'Linux terminal errors already deliver through DirWatch.notify_error; no Windows async-fatal adapter is '
        'required.'),
    'tsc/internal/fswatch/watcher.go:dirWatchError.Error': _equivalent(
        'crates/tsr_fswatch/src/lib.rs', 'Self::DirectoryWatch { source, .. } => return write!(f, "{source}"),',
        'The structured directory error delegates Display to its wrapped cause exactly as native Error().'),
    'tsc/internal/fswatch/watcher.go:dirWatchError.Unwrap': _equivalent(
        'crates/tsr_fswatch/src/lib.rs', 'impl std::error::Error for Error {',
        'Error::source exposes the DirectoryWatch boxed cause; errno and tagged-error predicates recursively '
        'traverse the same structured wrappers.'),
    'tsc/internal/fswatch/debounce.go:debounce.loop': _equivalent(
        'crates/tsr_fswatch/src/debounce.rs', 'loop {',
        'The worker loops over wait, coalescing and callback delivery; it additionally exits when the owned '
        'backend is retired.'),
    'tsc/internal/fswatch/debounce.go:debounce.notifyIfReady': _equivalent(
        'crates/tsr_fswatch/src/debounce.rs', 'if last.is_some_and(|last| last.elapsed() <= Duration::from_millis(500)) {',
        'The last-delivery timestamp decides between immediate delivery after the 500ms maximum and a 50ms '
        'coalescing wait, matching the pin.'),
    'tsc/internal/fswatch/debounce.go:debounce.coalesceWait': _equivalent(
        'crates/tsr_fswatch/src/debounce.rs', '.wait_timeout(state, Duration::from_millis(50))',
        'The condvar timed wait uses a generation counter to distinguish a new trigger from timeout; another '
        'trigger restarts the loop without firing.'),
    'tsc/internal/fswatch/debounce.go:debounce.fireCallbacks': _equivalent(
        'crates/tsr_fswatch/src/debounce.rs', 'for watch in watches {',
        'The worker upgrades the directory snapshot under lock, resets the latch, drops the lock and invokes '
        'callbacks, recording delivery time. Panic isolation does not retain the lock.'),
    'tsc/internal/fswatch/debounce.go:debounce.waitChLocked': _equivalent(
        'crates/tsr_fswatch/src/debounce.rs', 'while !state.stop && !state.signalled {',
        'The predicate is initialized with the State, replacing the lazy closed/open wait channel; condvar wait is '
        'always guarded by that predicate.'),
    'tsc/internal/fswatch/debounce.go:debounce.latchWait': _equivalent(
        'crates/tsr_fswatch/src/debounce.rs', 'while !state.stop && !state.signalled {',
        'The condition variable waits until the persistent signalled predicate is set; it cannot lose a trigger '
        'between predicate read and sleep.'),
    'tsc/internal/fswatch/debounce.go:debounce.triggerChLocked': _equivalent(
        'crates/tsr_fswatch/src/debounce.rs', 'generation: u64,',
        'The generation counter replaces the renewed trigger channel identity; trigger increments it and the timed '
        'waiter detects changes.'),
    'tsc/internal/fswatch/debounce.go:debounce.latchReset': _equivalent(
        'crates/tsr_fswatch/src/debounce.rs', 'state.signalled = false;',
        'The worker resets the persistent latch under its mutex before delivering the callback snapshot.'),
    'tsc/internal/fswatch/debounce.go:debounce.remove': _equivalent(
        'crates/tsr_fswatch/src/debounce.rs', 'state.watches.retain(|w| w.strong_count() != 0);',
        'Go explicitly removes callback-map entries. Rust stores only Weak directory references and prunes expired '
        'registrations at delivery; unwatch clears live callbacks, and final close stops the worker.'),
    'tsc/internal/fswatch/event.go:eventList.createLocked': _equivalent(
        'crates/tsr_fswatch/src/event.rs', '0 => entry.created = sequence,',
        'The create branch of record sets createdSeq, including the preceding rapid delete/recreate reset, under '
        'the single EventList mutex.'),
    'tsc/internal/fswatch/event.go:eventList.updateLocked': _equivalent(
        'crates/tsr_fswatch/src/event.rs', '1 => entry.updated = sequence,',
        'The update branch of record changes updatedSeq under the same mutex.'),
    'tsc/internal/fswatch/event.go:eventList.removeLocked': _equivalent(
        'crates/tsr_fswatch/src/event.rs', '_ => entry.deleted = sequence,',
        'The remove branch of record changes deletedSeq under the same mutex.'),
    'tsc/internal/fswatch/event.go:eventList.nextSeqLocked': _equivalent(
        'crates/tsr_fswatch/src/event.rs', 'state.sequence = state.sequence.wrapping_add(1);',
        "A missing explicit event sequence increments the u64 sequence with the pin's wrapping semantics while "
        'holding the mutex.'),
    'tsc/internal/fswatch/event.go:eventList.advanceSeqLocked': _equivalent(
        'crates/tsr_fswatch/src/event.rs', 'state.sequence = state.sequence.max(sequence);',
        'An explicit native sequence advances the stored cutoff only when larger, inlined in record.'),
    'tsc/internal/fswatch/event.go:eventList.getOrCreate': _equivalent(
        'crates/tsr_fswatch/src/event.rs', 'let entry = state.entries.entry(path.to_vec()).or_default();',
        'HashMap entry lookup inserts the zeroed sequence record only when absent, directly in the locked record '
        'path.'),
    'tsc/internal/fswatch/event.go:eventEntry.isDeleted': _equivalent(
        'crates/tsr_fswatch/src/event.rs', '0 if entry.deleted > entry.created && entry.deleted > entry.updated => {',
        'The predicate is inlined at its only production caller, the rapid-recreate arm of record.'),
    'tsc/internal/fswatch/event.go:eventList.size': _equivalent(
        'crates/tsr_fswatch/src/event.rs', '!s.entries.is_empty() || s.error.is_some()',
        'Production consumers only ask whether size is positive; has_pending folds that and hasError into one '
        'locked predicate. The event-list unit case separately asserts the raw entry count.'),
    'tsc/internal/fswatch/event.go:eventList.hasError': _equivalent(
        'crates/tsr_fswatch/src/event.rs', '!s.entries.is_empty() || s.error.is_some()',
        'Production callers combine nonempty events with a latched error; Rust performs both reads atomically in '
        'has_pending.'),
    'tsc/internal/fswatch/event.go:eventList.getError': _equivalent(
        'crates/tsr_fswatch/src/event.rs', 'assert_eq!(lock(&e.0).error, Some(Error::Message("first".into())));',
        'No production caller at the pin; native tests use getError as a non-consuming observation. The Rust unit '
        'checks read the same private error slot without adding a public/test-only accessor.'),
    'tsc/internal/fswatch/event.go:eventList.snapshotSinceLocked': _equivalent(
        'crates/tsr_fswatch/src/event.rs', 'entry.kind_since(*start).map(|kind| PendingEvent {',
        'The drain loop builds each callback snapshot by sequence with unchanged kind/root/path fields under the '
        'EventList mutex.'),
    'tsc/internal/fswatch/event.go:eventList.snapshotLocked': _equivalent(
        'crates/tsr_fswatch/src/event.rs', 'fn drain(events: &EventList) -> (Vec<Event>, Option<Error>) {',
        'The startSeq=0 specialization is the test drain helper; production requests all subscriber cutoff '
        'snapshots through drain_for_sequences.'),
    'tsc/internal/fswatch/event.go:eventList.drain': _equivalent(
        'crates/tsr_fswatch/src/event.rs', '(output, state.error.take())',
        'Drain-for-sequences combines event extraction and first-error take atomically and clears entries; '
        'production uses an empty sequence slice to discard events without callbacks and a cutoff per live '
        'callback.'),
    'tsc/internal/fswatch/event.go:eventList.getEvents': _equivalent(
        'crates/tsr_fswatch/src/event.rs', 'fn drain(events: &EventList) -> (Vec<Event>, Option<Error>) {',
        'The pin has only test callers for non-consuming getEvents. Rust tests observe the same event reduction '
        'through the production drain; production never exposes a snapshot separate from its atomic clear.'),
    'tsc/internal/fswatch/inotify_linux.go:init#1': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'fn backend(&self) -> Result<Arc<dyn Backend>, Error> {',
        'Compile-time platform branches dispatch the named native backend; fanotify availability probes required '
        'kernel flags before selection instead of installing a Go package factory.'),
    'tsc/internal/fswatch/inotify_linux.go:inotifyBackend.start': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn new(mode: Mode, no_rename: bool) -> Result<Arc<Self>, Error> {',
        'The shared constructor opens the mode-specific descriptor and wake pipe, then spawns the poll/read '
        'worker. Owned descriptors clean up every partial failure; construction returning is the start barrier.'),
    'tsc/internal/fswatch/inotify_linux.go:inotifyBackend.closeFDs': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'state.fd.take();',
        'Worker exit retires the shared native descriptor before notifying failures; later subscriptions fail. '
        'OwnedFd releases the read descriptor and wake reader, while shutdown takes the writer once and joins '
        'unless invoked from that worker itself.'),
    'tsc/internal/fswatch/inotify_linux.go:inotifyBackend.shutdown': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn shutdown(&self) {',
        'The common backend writes a wake byte once, then joins unless called on its own worker. Both mode '
        'variants use this same shutdown operation.'),
    'tsc/internal/fswatch/inotify_linux.go:inotifyBackend.subscribe': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn subscribe(&mut self, watch: &Arc<DirWatch>) -> Result<(), Error> {',
        'The mode parameter selects inotify or fanotify registration; recursive descriptor-relative walks preserve '
        'logical/physical roots. Fanotify additionally probes the rename mask once.'),
    'tsc/internal/fswatch/inotify_linux.go:inotifyBackend.closeWatch': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn close_watch(&mut self, watch: &Arc<DirWatch>) {',
        'The common cleanup removes only subscriptions for this directory identity, releases the native '
        'mark/descriptor when the key has no subscribers, and continues despite teardown errors. Every supported '
        'native caller discards the inotify closeWatch error result, as public Close does.'),
    'tsc/internal/fswatch/inotify_linux.go:inotifyBackend.handleEvents': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn run(state: Arc<Mutex<State>>, fd: Arc<OwnedFd>, wake: OwnedFd) -> Result<(), Error> {',
        'One poll/read worker dispatches mode-specific record decoders under the state mutex, deduplicates touched '
        'directories, then notifies outside the lock.'),
    'tsc/internal/fswatch/fanotify_linux.go:init#1': _equivalent(
        'crates/tsr_fswatch/src/watcher.rs', 'fn backend(&self) -> Result<Arc<dyn Backend>, Error> {',
        'Compile-time platform branches dispatch the named native backend; fanotify availability probes required '
        'kernel flags before selection instead of installing a Go package factory.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.start': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn new(mode: Mode, no_rename: bool) -> Result<Arc<Self>, Error> {',
        'The shared constructor opens the mode-specific descriptor and wake pipe, then spawns the poll/read '
        'worker. Owned descriptors clean up every partial failure; construction returning is the start barrier.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.closeFDs': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'state.fd.take();',
        'Worker exit retires the shared native descriptor before notifying failures; later subscriptions fail. '
        'OwnedFd releases the read descriptor and wake reader, while shutdown takes the writer once and joins '
        'unless invoked from that worker itself.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.shutdown': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn shutdown(&self) {',
        'The common backend writes a wake byte once, then joins unless called on its own worker. Both mode '
        'variants use this same shutdown operation.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.subscribe': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn subscribe(&mut self, watch: &Arc<DirWatch>) -> Result<(), Error> {',
        'The mode parameter selects inotify or fanotify registration; recursive descriptor-relative walks preserve '
        'logical/physical roots. Fanotify additionally probes the rename mask once.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.closeWatch': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn close_watch(&mut self, watch: &Arc<DirWatch>) {',
        'The common cleanup removes only subscriptions for this directory identity, releases the native '
        'mark/descriptor when the key has no subscribers, and continues despite teardown errors. Every supported '
        'native caller discards the inotify closeWatch error result, as public Close does.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.handleEvents': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn run(state: Arc<Mutex<State>>, fd: Arc<OwnedFd>, wake: OwnedFd) -> Result<(), Error> {',
        'One poll/read worker dispatches mode-specific record decoders under the state mutex, deduplicates touched '
        'directories, then notifies outside the lock.'),
    'tsc/internal/fswatch/inotify_linux.go:inotifyBackend.watchDir': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn add_dir(',
        'The Mode::Inotify branch registers the physical directory and appends the logical-path subscription under '
        'its returned kernel descriptor.'),
    'tsc/internal/fswatch/inotify_linux.go:inotifyBackend.handleEvent': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn inotify_events(',
        'The safe record decoder contains both the descriptor subscription dispatch and the native ordered mask '
        'branches: create/move-to, modify, delete/move-from, recursive registration/removal and terminal root '
        'errors.'),
    'tsc/internal/fswatch/inotify_linux.go:inotifyBackend.handleSubscription': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn inotify_events(',
        'The safe record decoder contains both the descriptor subscription dispatch and the native ordered mask '
        'branches: create/move-to, modify, delete/move-from, recursive registration/removal and terminal root '
        'errors.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.markDir': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn add_dir(',
        'The Mode::Fanotify branch marks the physical directory, reads its FID key and unmarks on key failure, '
        'then inserts the logical-path subscription.'),
    'tsc/internal/fswatch/fanotify_linux.go:makeFanotifyHandleKey': _equivalent(
        'crates/tsr_fswatch/src/fanotify.rs', 'pub(crate) struct HandleKey {',
        'Owned fsid, handle_type and opaque handle bytes with derived Eq/Hash replace the Go comparable '
        'struct/string constructor; no opaque kernel bytes are interpreted as pointers.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.handleOverflow': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn overflow(',
        'Overflow visits active subscriptions, latches the error and deduplicates touched directory owners; dead '
        'roots without native subscriptions are excluded.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.handleRenameEvent': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'if mask & libc::FAN_RENAME != 0 {',
        'A paired rename is split into delete-old then create-new records, preserving FAN_ONDIR. The shared per- '
        'subscription handler performs the same descendant removal and new recursive watch registration.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.handleParsedEvent': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn fanotify_event(',
        'The shared handler combines FID-key dispatch and the native subscription logic: merged create/delete '
        'existence check, delete-first processing, recursive tracking and modified events.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.handleSubscription': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn fanotify_event(',
        'The shared handler combines FID-key dispatch and the native subscription logic: merged create/delete '
        'existence check, delete-first processing, recursive tracking and modified events.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.dropSubsForPathLocked': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn drop_path(',
        'A single helper takes recursive=false and removes exactly matching or matching/descendant logical paths '
        'from every FID-key subscription list; fanotify marks have the same documented moved-out-inode limitation.'),
    'tsc/internal/fswatch/fanotify_linux.go:fanotifyBackend.dropSubsForPathAndDescendantsLocked': _equivalent(
        'crates/tsr_fswatch/src/linux.rs', 'fn drop_path(',
        'A single helper takes recursive=true and removes exactly matching or matching/descendant logical paths '
        'from every FID-key subscription list; fanotify marks have the same documented moved-out-inode limitation.'),
    'tsc/internal/fswatch/walkdir_unix.go:readDirEntries': _equivalent(
        'crates/tsr_fswatch/src/walkdir.rs', 'let mut reader = Dir::read_from(fd.as_fd())?;',
        'rustix decodes native dirents; the loop copies name/type for every nonzero-inode non-dot record before '
        'descending. DT_UNKNOWN uses no-follow metadata, and native read errors propagate.'),
    'tsc/internal/fswatch/walkdir_dirent_darwin.go:reclenOf': _equivalent(
        'crates/tsr_fswatch/src/walkdir.rs', 'let mut reader = Dir::read_from(fd.as_fd())?;',
        'The selected rustix platform Dir decoder consumes the platform ABI record length; Rust never casts a '
        'buffer into an unchecked dirent struct.'),
    'tsc/internal/fswatch/walkdir_dirent_darwin.go:inoOf': _equivalent(
        'crates/tsr_fswatch/src/walkdir.rs', 'if include_entry(entry.ino(), name) {',
        "rustix DirEntry::ino provides the native inode field; the caller preserves the pin's zero-inode "
        'exclusion.'),
    'tsc/internal/fswatch/walkdir_dirent_linux.go:reclenOf': _equivalent(
        'crates/tsr_fswatch/src/walkdir.rs', 'let mut reader = Dir::read_from(fd.as_fd())?;',
        'The selected rustix platform Dir decoder consumes the platform ABI record length; Rust never casts a '
        'buffer into an unchecked dirent struct.'),
    'tsc/internal/fswatch/walkdir_dirent_linux.go:inoOf': _equivalent(
        'crates/tsr_fswatch/src/walkdir.rs', 'if include_entry(entry.ino(), name) {',
        "rustix DirEntry::ino provides the native inode field; the caller preserves the pin's zero-inode "
        'exclusion.'),
    'tsc/internal/fswatch/walkdir.go:walkDirGeneric': _equivalent(
        'crates/tsr_fswatch/src/walkdir.rs', 'pub(crate) fn walk_dir(',
        'The generic fallback has no production caller on the selected Linux/macOS targets; their build tags '
        'select walkdir_unix.go. The Rust native traversal covers the same no-follow recursive filesystem contract '
        'and its selected-platform tests; no unselected-platform fallback is shipped.'),
    'tsc/internal/fswatch/walkdir.go:walkDirGenericVisit': _equivalent(
        'crates/tsr_fswatch/src/walkdir.rs', 'fn visit(',
        'The generic fallback has no production caller on the selected Linux/macOS targets; their build tags '
        'select walkdir_unix.go. The Rust native traversal covers the same no-follow recursive filesystem contract '
        'and its selected-platform tests; no unselected-platform fallback is shipped.'),
})

# X5 compiler/watch lifecycle review, 2026-10-02.
REVIEWED.update({
    'tsc/internal/compiler/includeprocessor.go:updateFileIncludeProcessor': _equivalent(
        'crates/tsr_compiler/src/reuse.rs', 'map(|reason| Arc::new(reason.fresh_for_program()))',
        'Reuse retains reason data and processing diagnostics but reconstructs every program-relative '
        'location/diagnostic cache, including fresh include explanations and OnceLock diagnostics. The existing '
        'position regression checks an import moved by two lines after reuse.'),
    'tsc/internal/compiler/program.go:Program.FilesByPath': _equivalent(
        'crates/tsr_compiler/src/loader.rs', 'pub fn file(&self, path: &[u8]) -> Option<&ProgramFile> {',
        'Go exposes its map; Rust keeps by_path private and combines files() iteration with this checked lookup. '
        'Watch source membership, eviction decisions, replacement and graph cloning use that same program index; '
        'no additional map copy is needed.'),
    'tsc/internal/compiler/program.go:Program.needsImportHelpersImportSpecifier': _equivalent(
        'crates/tsr_compiler/src/reuse.rs', 'if options.import_helpers.is_true()',
        'synthetic_imports folds this helper with the JSX-runtime requirement. Callers supply the redirected '
        'options for each canonical or supplemental file; JS/declaration/isolated-module/external-module '
        'conditions match the pin.'),
    'tsc/internal/compiler/program.go:equalCheckJSDirectives': _equivalent(
        'crates/tsr_compiler/src/reuse.rs', 'a.check_js_directive.as_ref().map(|d| d.enabled)',
        'Option equality in can_replace_file distinguishes absent from false/true and ignores directive positions '
        'just as the native helper does.'),
    'tsc/internal/compiler/program.go:equalFileReferences': _equivalent(
        'crates/tsr_compiler/src/reuse.rs', 'fn equal_references<',
        'Iterator comparison combines native slice equality with the FileName, ResolutionMode and Preserve '
        'predicate; it compares lengths and ignores source spans.'),
    'tsc/internal/compiler/program.go:equalModuleAugmentationNames': _equivalent(
        'crates/tsr_compiler/src/reuse.rs', 'fn equal_names(',
        'The shared name comparison uses always_text=true for augmentation names and requires equal kinds and raw '
        'bytes.'),
    'tsc/internal/compiler/program.go:equalModuleSpecifiers': _equivalent(
        'crates/tsr_compiler/src/reuse.rs', 'if !equal_names(old_view, left, new_view, right, false)?',
        'The shared name comparison uses always_text=false for imports: kinds must match; only StringLiteral text '
        'participates. The caller also compares usage-site resolution modes.'),
    'tsc/internal/compiler/program.go:lazyValue.tryReuse': _equivalent(
        'crates/tsr_compiler/src/reuse.rs', 'package_resolver: std::sync::Mutex::new(',
        'Reuse clones the immutable resolution graph and forks the resolver cache for the new host. '
        'GetSymlinkCache lives in each checker host and remains lazily recomputed from that graph; '
        'packageNames/unresolvedImports consumers are already assigned to Phase5. There is no Go-style program '
        'lazyValue wrapper to copy, and no derived value is computed eagerly by reuse.'),
    'tsc/internal/execute/watcher.go:Watcher.DoCycle': _equivalent(
        'crates/tsr_tsc/src/watcher.rs', 'fn cycle(&mut self, manager: &WatchManager)',
        'The public trait method acquires the cycle and state guards, then this private method drains events, '
        'invalidates configuration/sources, performs relevance and mapper refresh checks, and builds. Guard drop '
        'supplies the native deferred unlock.'),
    'tsc/internal/execute/watcher.go:Watcher.evictChangedSourceFiles': _equivalent(
        'crates/tsr_tsc/src/watcher.rs', 'self.cache.evict(path.as_bytes());',
        'The relevant-event loop evicts the canonical file-cache entry before classifying directory, new-root, '
        'mapped and non-source dependency events; existing owners remain retained by old programs.'),
    'tsc/internal/execute/watcher.go:Watcher.parseConfigFile': _equivalent(
        'crates/tsr_tsc/src/watcher.rs', 'let result = cache.read_config_file(',
        'Configuration parsing and read-error/status handling are folded into recheck_config; the live config host '
        'and fresh extended-config cache retain the native reread boundary.'),
    'tsc/internal/execute/watcher.go:Watcher.reconcileWatches': _equivalent(
        'crates/tsr_tsc/src/watcher.rs', 'if let Err(error) = manager.reconcile_watches(&self.desired_watches(manager, &seen)?)',
        'The build completion path composes computeDesiredWatches and manager reconciliation directly; a failed '
        'native subscription prints its error and latches overflow for the next cycle.'),
    'tsc/internal/execute/watcher.go:Watcher.start': _equivalent(
        'crates/tsr_tsc/src/watcher.rs', 'pub fn start(context: &Context, options: Options)',
        'Combined constructor/start creates the mapper session, reads previous build info, prints the starting '
        'status, builds once and enters the native cancellation loop. Test sessions return the retained watcher; '
        'drop releases session resources.'),
    'tsc/internal/execute/watcher.go:Watcher.tryUpdateProgram': _equivalent(
        'crates/tsr_tsc/src/watcher.rs', 'let reuse = self',
        'The build fast path is folded into build: one changed ordinary source, stable root set/config, no '
        'overflow or structural dependency change, then Program::reuse_program and incremental publication. A '
        'retained speculative owner survives until full fallback loading to avoid duplicate parsing.'),
    'tsc/internal/execute/watcher.go:createWatcher': _equivalent(
        'crates/tsr_tsc/src/watcher.rs', 'let mut state = State {',
        'Typed Options plus State and WatchManager construction replace the standalone native constructor; status '
        'locale/options are snapshotted and the optional test backend is injected before the first build.'),
    'tsc/internal/execute/watcher.go:equalJSXImplicitImport': _equivalent(
        'crates/tsr_compiler/src/reuse.rs', 'fn synthetic_imports(',
        'The native watcher equality precheck is folded into the stricter ReuseProgram guard which rejects either '
        'version requiring a JSX runtime synthetic import, as native ReuseProgram itself does. Therefore a changed '
        'runtime import cannot take the fast path; the existing JSX pragma watch regression exercises it.'),
    'tsc/internal/execute/watcher.go:watchCompilerHost.GetSourceFile': _equivalent(
        'crates/tsr_compiler/src/cache.rs', 'pub(crate) fn acquire(',
        'The session FileCache stores weak immutable parsed/bound owners; source events evict paths and loader '
        'acquisition checks actual bytes plus parse context before reuse. This replaces the native mutable mtime '
        'cache under the existing ownership contract. Full/overflow/config changes clear it and fallback retains '
        'the speculative owner.'),
    'tsc/internal/execute/watchmanager/watchmanager.go:DirWatchSet.canonical': _equivalent(
        'crates/tsr_tsc/src/watchmanager.rs', 'tsr_tspath::canonical(dir, self.options.use_case_sensitive_file_names).into_owned(),',
        'Canonicalization is inlined at insertion and coverage lookup using the same case-sensitivity option; '
        'existing case-sensitive and case-insensitive coverage/dedup regressions exercise both sites.'),
    'tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.DoCycleCh': _equivalent(
        'crates/tsr_tsc/src/watchmanager.rs', 'while !changes.signalled && context.err().is_none() {',
        'A condition-variable predicate replaces the exposed Go capacity-one channel. Event callbacks set a '
        'coalesced signalled bit; the run loop consumes it, and cancellation wakes an idle wait.'),
    'tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.Unlock': _equivalent(
        'crates/tsr_tsc/src/watchmanager.rs', "pub fn lock(&self) -> MutexGuard<'_, ()> {",
        'The cycle mutex guard returned by lock releases on scope exit, including errors and unwinding. An '
        'independent public Unlock operation would permit invalid use and is unnecessary.'),
    'tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.createDirWatchRequest': _equivalent(
        'crates/tsr_tsc/src/watchmanager.rs', 'let requests = additions',
        'Request construction is folded into batch reconciliation: per-entry identity, recursive and ignore '
        'options, and weak owner callbacks preserve termination invalidation without creating an ownership cycle.'),
    'tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.createDirWatches': _equivalent(
        'crates/tsr_tsc/src/watchmanager.rs', 'match backend.watch_directories(requests) {',
        'Batch creation and result installation are folded into reconcile_watches. Provisional identity entries '
        'allow synchronous termination callbacks; successful subscriptions publish only to the same identity and '
        'failed batches remove their provisional entries.'),
})

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
    after = [entry for entry in phase4 if entry.get("kind") in ("source", "harness") and entry["go"] not in MOVED_OUT]
    # The plan's historical denominator predates decision 4. Reconstruct it
    # explicitly even after the live ledger applies those moves.
    before = after + [dict(entry, kind="source") for entry in entries if entry["go"] in MOVED_OUT]
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
    """Every marker under crates/ and tools/ with all of its sites, by file (a
    file that carries one marker twice lists twice, so duplicates still show)."""
    sites = {}
    for base in MARKER_ROOTS:
        for path in sorted((root / base).rglob("*.rs")):
            if "target" in path.relative_to(root).parts:
                continue
            for line in path.read_text(errors="replace").splitlines():
                match = MARKER.match(line)
                if match and ":" in match.group(1):
                    sites.setdefault(match.group(1), []).append(path.relative_to(root).as_posix())
    return sites


def resolve_site(site, root=ROOT):
    """A reviewed (path, anchor) site as its path, or None when the anchor does not occur exactly once there."""
    path, anchor = site
    target = root / path
    if not path.startswith(tuple(f"{base}/" for base in MARKER_ROOTS)) or target.suffix != ".rs" \
            or not target.is_file():
        return None
    return path if sum(anchor in line for line in target.read_text().splitlines()) == 1 else None


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
                    problems.append(f"{identity}: equivalent site {review.get('rust')} does not resolve to one anchor")
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
