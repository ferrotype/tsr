# F5b compiler destination audit

This record corrects I17's compiler scope classification at pinned Go
`1f70213d4922b434345f639b441681e470c7cfc1`. It applies the accepted Phase 1
plan; it does not assert new owner approvals, waive behavior, or grant coverage.
The exact operation identities, source hashes and evidence are in
[`coverage-review.json`](../data/phase1/coverage-review.json). The syntax roster
exempts a preparation *step*, not the whole phase.

## Authority and result

[Plan section 2](PHASE1-implementation-plan.md#2-scope-and-boundaries) includes
module resolution/options and the compiler runner's parse/bind/syntactic work.
F3a tasks 5 and 7 include redirection, loader file/include graphs and option
diagnostics. The later-phase exclusions cover checker services (Phase 2),
emission/transforms (Phase 3), CLI/watch and project-reference **build
scheduling** (Phase 4), and editor/project-system/production mapper work
(Phase 5). Loading a reference's configuration or redirecting its source to an
already-built declaration file is not build scheduling.

The earlier record incorrectly placed every project-reference operation in
Phase 4, citing the missing corpus inputs and Rust's explicit rejection as
scope evidence. Neither fact authorizes a phase exclusion.

| Classification | Count | Treatment |
| --- | ---: | --- |
| Other accepted-plan exclusions, retained | 175 | Explicit plan clause and source/caller evidence; no new approval claimed |
| Project-reference checker-facing services | 6 | Phase 2 |
| Project-reference editor-facing services | 24 | Phase 5 |
| Ordinary reference loading/configuration | 23 | Unresolved Phase 1 work; pending |
| Unused reference wrapper/helper chain | 1 | `unused_at_pin`, with complete pinned-tree caller check |

There are now 205 reviewed later-phase destinations: 63 in Phase 2, 57 in
Phase 3, 24 in Phase 4 and 61 in Phase 5. The 23 unresolved operations remain
in the coverage report with `compiler_destination_unreviewed`; they cannot
contribute preparation or implementation completion. `later_step` on their
syntax-roster entries means the config/program loader owns preparation, not
that Phase 1 is discharged. No original operation ID is removed.

This was a bounded scope audit. It checked the 54 reference-related entries
against their actual call paths and retained the other 175 established
exclusions under explicit shared plan authority. It did not re-review every
implementation body or assert absence of Rust implementations.

## Why the reference paths differ

The normal loader calls `addProjectReferenceTasks` before creating its module
resolver (`fileloader.go:177-178`). Reference parse tasks read configurations
and input/output names (`projectreferenceparser.go:19-30`); ordinary file parse
tasks consult redirects (`filesparser.go:73-76`). Option verification calls
`verifyProjectReferences` (`program.go:1047`), including missing configuration,
`composite`/`noEmit` and build-info collision errors. These do not run another
project's build.

The source-as-output host is different: its constructor runs only when
`canUseProjectReferenceSource` is true (`projectreferenceparser.go:77-78`). The
only pinned non-test `UseSourceOfProjectReference: true` initializer is the
editor's project construction (`internal/project/project.go:454`). Its 20
host/VFS operations belong to that Phase 5 service, including unsupported
resolver-host methods that deliberately panic. Four reference-tree query
accessors have the same editor boundary.

The six Phase 2 rows are checker-facing Program accessors and the
`isSourceFromProjectReference` helper. Their loader-side mapper counterparts
are separate identities and remain pending where normal loading uses them.

## Unresolved ordinary-loader operations

These entries are not requests for an exception. Their next step is to assess
and prepare the ordinary loading behavior as Phase 1 work; no implementation
or completion is claimed by this record.

| Operation | Concrete pinned caller/path |
| --- | --- |
| `fileloader.go:fileLoader.addProjectReferenceTasks` | newFileLoader/load setup at fileloader.go:177, before module.NewResolver at :178; method initializes a mapper even when references are empty (:327-334). |
| `filesparser.go:parseTask.redirect` | parseTask.load at filesparser.go:73-76, before parsing the requested source |
| `host.go:compilerHost.GetResolvedProjectReference` | projectReferenceParseTask.parse at projectreferenceparser.go:24 |
| `program.go:Program.GetSourceOfProjectReferenceIfOutputIncluded` | processingDiagnostic.createDiagnosticExplainingFile -> includeProcessor.explainRedirectAndImpliedFormat (processingDiagnostic.go:106 / includeprocessor.go:144); Program.GetProgramDiagnostics at program.go:794-800 requests those global load diagnostics. Also emit and CLI ExplainFiles. |
| `program.go:Program.RangeResolvedProjectReference` | Program.verifyProjectReferences at program.go:1375, plus project-system consumers |
| `program.go:Program.verifyProjectReferences` | Program.verifyCompilerOptions at program.go:1047 during NewProgram; reports missing config, composite/noEmit and output-path conflicts |
| `program.go:ProgramOptions.canUseProjectReferenceSource` | ordinary parseTask.load redirect predicate through projectreferencefilemapper.go:36, filesparser.go:351,491, and projectReferenceParser.initMapper/initMapperWorker. True is enabled only by project/project.go:454; false controls ordinary .d.ts redirection. |
| `projectreferencefilemapper.go:projectReferenceFileMapper.getCompilerOptionsForFile` | fileLoader.loadSourceFile at fileloader.go:404 |
| `projectreferencefilemapper.go:projectReferenceFileMapper.getParseFileRedirect` | parseTask.load at filesparser.go:73; also Program.GetParseFileRedirect |
| `projectreferencefilemapper.go:projectReferenceFileMapper.getProjectReferenceFromOutputDts` | getRedirectForResolution at projectreferencefilemapper.go:99 and getSourceToDtsIfSymlink at :183; also source-mode/editor accesses |
| `projectreferencefilemapper.go:projectReferenceFileMapper.getProjectReferenceFromSource` | getParseFileRedirect .d.ts branch at projectreferencefilemapper.go:47 and getRedirectForResolution at :93; also checker/emit Program accessor |
| `projectreferencefilemapper.go:projectReferenceFileMapper.getRedirectForResolution` | ordinary module and type-reference resolution at fileloader.go:774,828 |
| `projectreferencefilemapper.go:projectReferenceFileMapper.getRedirectParsedCommandLineForResolution` | getCompilerOptionsForFile at projectreferencefilemapper.go:81 and ordinary import loading at fileloader.go:886 |
| `projectreferencefilemapper.go:projectReferenceFileMapper.getSourceToDtsIfSymlink` | getRedirectForResolution at projectreferencefilemapper.go:104; metadata is populated during ordinary loading when preserveSymlinks is enabled |
| `projectreferencefilemapper.go:projectReferenceFileMapper.rangeResolvedProjectReference` | Program.RangeResolvedProjectReference -> Program.verifyProjectReferences at program.go:1375 |
| `projectreferencefilemapper.go:projectReferenceFileMapper.rangeResolvedReferenceWorker` | rangeResolvedProjectReference at projectreferencefilemapper.go:126; also recursive traversal and Phase 5 child traversal |
| `projectreferencefilemapper.go:projectReferenceFileMapper.rootConfigPath` | projectReferenceParser.initMapper at projectreferenceparser.go:76 and rangeResolvedProjectReference at projectreferencefilemapper.go:123 |
| `projectreferenceparser.go:createProjectReferenceParseTasks` | fileLoader.addProjectReferenceTasks (fileloader.go:340) and projectReferenceParseTask.parse (projectreferenceparser.go:30) |
| `projectreferenceparser.go:projectReferenceParseTask.parse` | projectReferenceParser.start queues task.parse; fileLoader.addProjectReferenceTasks calls projectReferenceParser.parse (fileloader.go:341) |
| `projectreferenceparser.go:projectReferenceParser.initMapper` | projectReferenceParser.parse after RunAndWait (projectreferenceparser.go:52) |
| `projectreferenceparser.go:projectReferenceParser.initMapperWorker` | projectReferenceParser.initMapper and recursive child mapping (projectreferenceparser.go:76,113) |
| `projectreferenceparser.go:projectReferenceParser.parse` | projectReferenceParser.start queues task.parse; fileLoader.addProjectReferenceTasks calls projectReferenceParser.parse (fileloader.go:341) |
| `projectreferenceparser.go:projectReferenceParser.start` | projectReferenceParser.parse and its own recursion (projectreferenceparser.go:50,64) |

The full reason and accepted-plan clause for each row are retained in
`unresolved_compiler_evidence`, beside `unresolved_compiler_destinations`.

## Unused helper verification

`projectReferenceFileMapper.getResolvedReferenceFor` has one textual caller:
`Program.GetResolvedProjectReferenceFor` at `program.go:202`. The latter was
already classified `unused_at_pin`. A complete Go-tree search for both names
finds only the two definitions and that one wrapper call, and no pinned
interface requires the wrapper. The helper therefore joins the existing
unused wrapper; this is a closed unused chain, not a claim of zero textual
callers. Reproduce without executing upstream code:

```sh
rg -n 'GetResolvedProjectReferenceFor\(|getResolvedReferenceFor\(' upstream --glob '*.go'
```

The existing reviewed-unused `Program.ResolveModuleName` entry is unchanged.
Inventory and coverage views must be regenerated after the active captures;
this audit itself runs no producer, compilation or benchmark.
