package module_test

// Access only: replays an ordered action trace against the pinned module
// resolver and records what each action observed.
//
// It compiles into `package module_test`, the package
// upstream/tsc/internal/module/resolver_test.go already declares, for two
// reasons. First, every entry point this group drives is exported, so no
// in-package overlay is needed: the unexported `resolutionState`, `tracer`,
// `resolved` and cache methods are reached THROUGH those entry points and are
// witnessed by the trace lines their own bodies emit, not by direct calls.
// Second, `resolutionHostStub` (resolver_test.go:15-21) is already declared
// there, so the probe inherits the pin's own ResolutionHost rather than
// restating it. Nothing here redeclares a name the pinned test file owns
// (`resolutionHostStub`, `blockingFS`, `waitForSignal`, `flipFileExistsFS`);
// every identifier below is prefixed `p1`.
//
// The trace shape is the contract the Rust driver answers: one ordered result
// per action, under `ordered`, so member order survives canonicalisation.
// Anything that carries order travels as an array, and no value nested inside
// an ordered element is a multi-key object, because canonicalisation sorts
// object keys. A trace callback is therefore [code, [arg, ...]] and a result
// is an entry array [["field", value], ...].
//
// Every byte string is hex encoded. Paths, module names and trace arguments
// are Go `string`s that are not required to be valid UTF-8, and the Rust side
// carries them as `JsString` byte strings; hex is the only encoding both sides
// can emit without a lossy conversion. This follows the existing seam at
// tools/s07/module-trace/export_test.go:60-68, which hex encodes trace text
// arguments for exactly this reason.
//
// Two instruments are used, and both are deterministic and single threaded:
//
//   - The trace stream. `tracer.write` (resolver.go:55-59) is the pin's own
//     record of every file lookup, every skipped directory, every condition
//     considered and every package.json field read. At this pin there is no
//     FailedLookupLocations and no AffectingLocations field on either result
//     type -- `grep -rn -i affectinglocation upstream/tsc/internal` is empty --
//     so the trace IS the failed-lookup record, and a case that wants one asks
//     for `traceResolution`.
//   - `p1CountingFS`, which counts the resolver's calls into `vfs.FS`. It is
//     used only to answer "did this action touch the filesystem at all", which
//     is what separates a cache hit from a cache miss. Exact per-method counts
//     are deliberately NOT recorded: they would couple every cache case to the
//     two ports' lookup order, which the trace cases already compare directly.
//
// `p1CountingFS` is also swappable (`p1swap`), which is how a case observes a
// negative lookup followed by changed host state. The swap is an explicit
// ordered action, never a background mutation, so no observation here depends
// on goroutine scheduling. The three race regressions in resolver_test.go
// (TestResolveModuleNameTrailingSlashRace :141, TestResolveSubpathNilContentsRace
// :216, TestResolvePeerDependencyNilContentsRace :339) are NOT reproduced: see
// NOTES.md for which of their contracts are reachable single threaded and
// which are not.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"runtime"
	"slices"
	"sort"
	"strings"
	"testing"
	"testing/fstest"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	tsjson "github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/module"
	"github.com/microsoft/TypeScript/tsc/internal/packagejson"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

// p1CasePrefix claims a request. The schedule is shared with every other
// config group, and a subject string could collide with a neighbour's, so
// ownership is keyed on the case id this group was assigned.
const p1CasePrefix = "config/module/"

// p1Subject is the subject this group serves. A case under the prefix that
// declares another subject is a schedule error, not a decline.
const p1Subject = "moduleResolution"

type p1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared schedule and another
	// group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

// p1File is one entry of a fixture map. `kind` is "file" or "symlink"; a
// symlink's target is stored verbatim and must be rooted, which is what
// vfstest.FromMap's own checkPath demands (vfstest.go:88-93).
type p1File struct {
	Path    *string `json:"path"`
	Kind    *string `json:"kind"`
	Content *string `json:"content"`
	Target  *string `json:"target"`
}

// p1CacheSeed is one pre-seeded packagejson.InfoCache entry. `contents` nil
// seeds the negative shape -- a non-nil entry whose Contents is nil -- which
// getPackageJsonInfo short-circuits on at resolver.go:1773-1778 and which is
// otherwise only produced by losing LoadOrStore.
type p1CacheSeed struct {
	PackageJSONPath *string `json:"package_json_path"`
	DirectoryExists *bool   `json:"directory_exists"`
	Contents        *string `json:"contents"`
}

type p1Redirect struct {
	ConfigName *string         `json:"config_name"`
	Options    json.RawMessage `json:"options"`
}

type p1PathInput struct {
	Path     *string `json:"path"`
	IsFolder *bool   `json:"is_folder"`
}

type p1ResolvedSpec struct {
	Extension                    *string `json:"extension"`
	ResolvedUsingExtraExtensions *bool   `json:"resolved_using_extra_extensions"`
}

// p1Action is the union of every key this group's actions carry. Every payload
// field is a pointer: an absent key must fail the probe, never default to a
// zero value that both sides could agree on without executing anything.
type p1Action struct {
	Op string `json:"op"`

	// new_resolver / new_resolver_with_options
	Cwd              *string         `json:"cwd"`
	CaseSensitive    *bool           `json:"case_sensitive"`
	Files            []p1File        `json:"files"`
	Options          json.RawMessage `json:"options"`
	TypingsLocation  *string         `json:"typings_location"`
	ProjectName      *string         `json:"project_name"`
	ExtraExtensions  []string        `json:"extra_extensions"`
	PackageJSONCache []p1CacheSeed   `json:"package_json_cache"`

	// mutate
	Add    []p1File `json:"add"`
	Remove []string `json:"remove"`

	// set_trace
	Enabled *bool `json:"enabled"`

	// resolve / resolve_type_reference / resolve_package_directory
	Name *string `json:"name"`
	File *string `json:"file"`
	Mode *int32  `json:"mode"`

	Redirect *p1Redirect `json:"redirect"`

	// entrypoints
	Directory             *string `json:"directory"`
	PackageName           *string `json:"package_name"`
	EnableDirectorySearch *bool   `json:"enable_directory_search"`

	// pure helpers
	Keys                   []string        `json:"keys"`
	Names                  []string        `json:"names"`
	Inputs                 []p1PathInput   `json:"inputs"`
	Paths                  json.RawMessage `json:"paths"`
	Candidates             []string        `json:"candidates"`
	Resolved               *p1ResolvedSpec `json:"resolved"`
	IsDeclarationFile      *bool           `json:"is_declaration_file"`
	ModuleFileNames        []string        `json:"module_file_names"`
	TypeReferenceFileNames []string        `json:"type_reference_file_names"`
	PackageIds             []p1PackageId   `json:"package_ids"`
}

type p1PackageId struct {
	Name             *string `json:"name"`
	SubModuleName    *string `json:"sub_module_name"`
	Version          *string `json:"version"`
	PeerDependencies *string `json:"peer_dependencies"`
}

func p1hex(text string) string { return hex.EncodeToString([]byte(text)) }

func p1require[T any](value *T, field string, op string) T {
	if value == nil {
		panic(fmt.Sprintf("phase1: action %q is missing required field %q", op, field))
	}
	return *value
}

// p1CountingFS counts what the resolver asks of the filesystem and lets a case
// replace the backing filesystem between actions. Only the methods the
// resolver actually calls are counted; the rest fall through to the embedded
// interface unchanged.
type p1CountingFS struct {
	vfs.FS
	calls int
}

func (f *p1CountingFS) FileExists(path string) bool {
	f.calls++
	return f.FS.FileExists(path)
}

func (f *p1CountingFS) DirectoryExists(path string) bool {
	f.calls++
	return f.FS.DirectoryExists(path)
}

func (f *p1CountingFS) ReadFile(path string) (string, bool) {
	f.calls++
	return f.FS.ReadFile(path)
}

func (f *p1CountingFS) Realpath(path string) string {
	f.calls++
	return f.FS.Realpath(path)
}

func (f *p1CountingFS) GetAccessibleEntries(path string) vfs.Entries {
	f.calls++
	return f.FS.GetAccessibleEntries(path)
}

func (f *p1CountingFS) swap(next vfs.FS) { f.FS = next }

// p1Reference is a ResolvedProjectReference (types.go:24-27) a case supplies
// so the redirect path can be driven at all. The pin has no other
// implementation outside the compiler.
type p1Reference struct {
	name    string
	options *core.CompilerOptions
}

func (r *p1Reference) ConfigName() string                     { return r.name }
func (r *p1Reference) CompilerOptions() *core.CompilerOptions { return r.options }

// p1State is one case's replay state.
type p1State struct {
	cwd           string
	caseSensitive bool
	files         map[string]any
	counting      *p1CountingFS
	host          *resolutionHostStub
	options       *core.CompilerOptions
	resolver      *module.Resolver
	packageCache  *packagejson.InfoCache
}

func p1decodeOptions(raw json.RawMessage) *core.CompilerOptions {
	options := &core.CompilerOptions{}
	if len(raw) == 0 {
		return options
	}
	// The pinned collections.OrderedMap implements UnmarshalJSONFrom
	// (collections/ordered_map.go:263) against tsc/internal/json, NOT
	// encoding/json's Unmarshaler, so `paths` only decodes -- and only keeps
	// its key order -- through the pinned json package.
	if err := tsjson.Unmarshal(raw, options); err != nil {
		panic(fmt.Sprintf("phase1: compiler options do not decode: %v", err))
	}
	return options
}

// p1reference builds a ResolvedProjectReference. A reference whose `options`
// key is absent or JSON null carries NIL compiler options, which is the middle
// of the three cases GetCompilerOptionsWithRedirect distinguishes
// (resolver.go:139-147): it returns the caller's own options, exactly as a nil
// reference does. Decoding an absent key into an empty struct would erase that
// distinction and make the case unable to test it.
func p1reference(spec *p1Redirect, op string) module.ResolvedProjectReference {
	raw := spec.Options
	var options *core.CompilerOptions
	if len(raw) != 0 && string(raw) != "null" {
		options = p1decodeOptions(raw)
	}
	return &p1Reference{name: p1require(spec.ConfigName, "config_name", op), options: options}
}

func p1decodePaths(raw json.RawMessage) *collections.OrderedMap[string, []string] {
	paths := &collections.OrderedMap[string, []string]{}
	if len(raw) == 0 {
		return paths
	}
	if err := tsjson.Unmarshal(raw, paths); err != nil {
		panic(fmt.Sprintf("phase1: paths do not decode: %v", err))
	}
	return paths
}

func p1applyFiles(files map[string]any, entries []p1File) {
	for _, entry := range entries {
		path := p1require(entry.Path, "path", "files")
		switch p1require(entry.Kind, "kind", "files") {
		case "file":
			files[path] = p1require(entry.Content, "content", "files")
		case "symlink":
			files[path] = vfstest.Symlink(p1require(entry.Target, "target", "files"))
		case "directory":
			files[path] = &fstest.MapFile{Mode: fs.ModeDir}
		default:
			panic("phase1: unknown fixture entry kind")
		}
	}
}

func (s *p1State) rebuild() {
	s.counting.swap(vfstest.FromMap(s.files, s.caseSensitive))
}

func (s *p1State) requireResolver() *module.Resolver {
	if s.resolver == nil {
		panic("phase1: action needs a resolver, but the case never built one")
	}
	return s.resolver
}

func p1traceRows(traces []module.DiagAndArgs) []any {
	rows := []any{}
	for _, trace := range traces {
		args := []any{}
		for _, arg := range trace.Args {
			switch arg := arg.(type) {
			case string:
				args = append(args, map[string]any{"text_hex": p1hex(arg)})
			case bool:
				args = append(args, map[string]any{"bool": arg})
			default:
				// resolver.go only ever writes string and bool arguments; a
				// third type is a pin change, not an observation.
				panic(fmt.Sprintf("phase1: unknown trace argument type %T", arg))
			}
		}
		rows = append(rows, []any{int64(trace.Message.Code()), args})
	}
	return rows
}

func p1diagnosticCodes(diagnostics []*ast.Diagnostic) []any {
	codes := []any{}
	for _, diagnostic := range diagnostics {
		codes = append(codes, int64(diagnostic.Code()))
	}
	return codes
}

func p1moduleRows(resolved *module.ResolvedModule) []any {
	if resolved == nil {
		return []any{[]any{"nil_result", true}}
	}
	return []any{
		[]any{"nil_result", false},
		[]any{"is_resolved", resolved.IsResolved()},
		[]any{"resolved_file_name_hex", p1hex(resolved.ResolvedFileName)},
		[]any{"original_path_hex", p1hex(resolved.OriginalPath)},
		[]any{"extension_hex", p1hex(resolved.Extension)},
		[]any{"resolved_using_ts_extension", resolved.ResolvedUsingTsExtension},
		[]any{"resolved_using_extra_extensions", resolved.ResolvedUsingExtraExtensions},
		[]any{"is_external_library_import", resolved.IsExternalLibraryImport},
		[]any{"alternate_result_hex", p1hex(resolved.AlternateResult)},
		[]any{"package_id_name_hex", p1hex(resolved.PackageId.Name)},
		[]any{"package_id_sub_module_name_hex", p1hex(resolved.PackageId.SubModuleName)},
		[]any{"package_id_version_hex", p1hex(resolved.PackageId.Version)},
		[]any{"package_id_peer_dependencies_hex", p1hex(resolved.PackageId.PeerDependencies)},
		[]any{"diagnostic_codes", p1diagnosticCodes(resolved.ResolutionDiagnostics)},
	}
}

func p1typeReferenceRows(resolved *module.ResolvedTypeReferenceDirective) []any {
	if resolved == nil {
		return []any{[]any{"nil_result", true}}
	}
	return []any{
		[]any{"nil_result", false},
		[]any{"is_resolved", resolved.IsResolved()},
		[]any{"primary", resolved.Primary},
		[]any{"resolved_file_name_hex", p1hex(resolved.ResolvedFileName)},
		[]any{"original_path_hex", p1hex(resolved.OriginalPath)},
		[]any{"is_external_library_import", resolved.IsExternalLibraryImport},
		[]any{"package_id_name_hex", p1hex(resolved.PackageId.Name)},
		[]any{"package_id_sub_module_name_hex", p1hex(resolved.PackageId.SubModuleName)},
		[]any{"package_id_version_hex", p1hex(resolved.PackageId.Version)},
		[]any{"package_id_peer_dependencies_hex", p1hex(resolved.PackageId.PeerDependencies)},
		[]any{"diagnostic_codes", p1diagnosticCodes(resolved.ResolutionDiagnostics)},
	}
}

// p1classify reduces a recovered panic to a portable class. A panic the pinned
// source raises itself keeps its literal text, because that sentence IS the
// contract a port has to reproduce; anything else is the toolchain's wording
// and only its shape is recorded.
//
// The Go RUNTIME's wording is the case that matters here. "runtime error: index
// out of range [16] with length 16" is not a fact about the pinned port -- it
// is a fact about the Go toolchain, and its phrasing and bracketed values have
// changed across releases. Freezing it would make a case fail on a Go upgrade
// that changed no behaviour, while what the case actually witnesses is that the
// pin indexes without a guard. So a runtime.Error keeps only its kind, taken
// from the leading words before the first `[` or `:` detail, and everything
// else keeps its text.
func p1classify(value any) string {
	if _, isRuntime := value.(runtime.Error); isRuntime {
		text := value.(error).Error()
		text = strings.TrimPrefix(text, "runtime error: ")
		if cut := strings.IndexByte(text, '['); cut >= 0 {
			text = strings.TrimSpace(text[:cut])
		}
		return "runtime: " + text
	}
	switch value := value.(type) {
	case string:
		return value
	case error:
		return "error: " + value.Error()
	default:
		return fmt.Sprintf("%T", value)
	}
}

// p1guard runs body and turns a panic into a recorded class, so a case whose
// subject IS the panic boundary observes it instead of failing the probe. A
// half-written row is discarded first. It never swallows a harness panic:
// those all begin "phase1:" and are re-raised.
func p1guard(row map[string]any, body func()) {
	defer func() {
		recovered := recover()
		if recovered == nil {
			return
		}
		if text, ok := recovered.(string); ok && strings.HasPrefix(text, "phase1:") {
			panic(recovered)
		}
		for key := range row {
			if key != "op" {
				delete(row, key)
			}
		}
		row["panic"] = p1classify(recovered)
	}()
	body()
}

func p1replay(actions []p1Action) []any {
	state := &p1State{files: map[string]any{}}
	rows := make([]any, 0, len(actions))
	for _, action := range actions {
		row := map[string]any{"op": action.Op}
		current := action
		p1guard(row, func() { p1apply(state, current, row) })
		rows = append(rows, row)
	}
	return rows
}

func p1apply(state *p1State, action p1Action, row map[string]any) {
	switch action.Op {
	case "new_resolver", "new_resolver_with_options":
		state.cwd = p1require(action.Cwd, "cwd", action.Op)
		state.caseSensitive = p1require(action.CaseSensitive, "case_sensitive", action.Op)
		state.files = map[string]any{}
		p1applyFiles(state.files, action.Files)
		state.counting = &p1CountingFS{}
		state.rebuild()
		state.host = &resolutionHostStub{fs: state.counting, cwd: state.cwd}
		state.options = p1decodeOptions(action.Options)
		if action.Op == "new_resolver" {
			state.packageCache = nil
			state.resolver = module.NewResolver(
				state.host,
				state.options,
				p1require(action.TypingsLocation, "typings_location", action.Op),
				p1require(action.ProjectName, "project_name", action.Op),
				action.ExtraExtensions,
			)
		} else {
			state.packageCache = packagejson.NewInfoCache(state.cwd, state.caseSensitive)
			seeds := []any{}
			for _, seed := range action.PackageJSONCache {
				path := p1require(seed.PackageJSONPath, "package_json_path", action.Op)
				entry := &packagejson.InfoCacheEntry{
					PackageDirectory: tspath.GetDirectoryPath(path),
					DirectoryExists:  p1require(seed.DirectoryExists, "directory_exists", action.Op),
				}
				if seed.Contents != nil {
					fields, err := packagejson.Parse([]byte(*seed.Contents))
					entry.Contents = &packagejson.PackageJson{Fields: fields, Parseable: err == nil}
				}
				stored := state.packageCache.Set(path, entry)
				// LoadOrStore: a second seed whose path canonicalises onto an
				// earlier one is handed the FIRST writer's entry back
				// (packagejson/cache.go:190-194).
				seeds = append(seeds, []any{p1hex(path), stored == entry, p1hex(stored.GetDirectory()), stored.Exists()})
			}
			row["cache_seeds"] = seeds
			state.resolver = module.NewResolverWithOptions(
				state.host,
				state.options,
				p1require(action.TypingsLocation, "typings_location", action.Op),
				p1require(action.ProjectName, "project_name", action.Op),
				module.ResolverOptions{PackageJsonCache: state.packageCache},
			)
		}
		row["created"] = true

	case "mutate":
		if state.counting == nil {
			panic("phase1: mutate before a resolver was built")
		}
		p1applyFiles(state.files, action.Add)
		for _, path := range action.Remove {
			delete(state.files, path)
		}
		state.rebuild()
		row["file_count"] = int64(len(state.files))

	case "set_trace":
		if state.options == nil {
			panic("phase1: set_trace before a resolver was built")
		}
		// The resolver holds the caller's *core.CompilerOptions and re-reads
		// TraceResolution on every request (resolver.go:201-206), so flipping
		// it here changes the mode of the NEXT action without discarding the
		// resolver's caches. That is the only way the cache write path is
		// reachable twice for one key, because a cache read short-circuits it
		// whenever tracing is off (resolver.go:278-283).
		if p1require(action.Enabled, "enabled", action.Op) {
			state.options.TraceResolution = core.TSTrue
		} else {
			state.options.TraceResolution = core.TSFalse
		}
		row["trace_resolution"] = state.options.TraceResolution == core.TSTrue

	case "resolve", "resolve_with_redirect":
		resolver := state.requireResolver()
		before := state.counting.calls
		var redirect module.ResolvedProjectReference
		if action.Op == "resolve_with_redirect" {
			spec := action.Redirect
			if spec == nil {
				panic("phase1: resolve_with_redirect without a redirect")
			}
			redirect = p1reference(spec, action.Op)
		}
		resolved, traces := resolver.ResolveModuleName(
			p1require(action.Name, "name", action.Op),
			p1require(action.File, "file", action.Op),
			core.ResolutionMode(p1require(action.Mode, "mode", action.Op)),
			redirect,
		)
		row["traces"] = p1traceRows(traces)
		row["result"] = p1moduleRows(resolved)
		row["touched_filesystem"] = state.counting.calls != before

	case "resolve_type_reference":
		resolver := state.requireResolver()
		before := state.counting.calls
		resolved, traces := resolver.ResolveTypeReferenceDirective(
			p1require(action.Name, "name", action.Op),
			p1require(action.File, "file", action.Op),
			core.ResolutionMode(p1require(action.Mode, "mode", action.Op)),
			nil,
		)
		row["traces"] = p1traceRows(traces)
		row["result"] = p1typeReferenceRows(resolved)
		row["touched_filesystem"] = state.counting.calls != before

	case "resolve_package_directory":
		resolver := state.requireResolver()
		before := state.counting.calls
		resolved := resolver.ResolvePackageDirectory(
			p1require(action.Name, "name", action.Op),
			p1require(action.File, "file", action.Op),
			core.ResolutionMode(p1require(action.Mode, "mode", action.Op)),
			nil,
		)
		row["result"] = p1moduleRows(resolved)
		row["touched_filesystem"] = state.counting.calls != before

	case "automatic_type_directive_names":
		resolver := state.requireResolver()
		_ = resolver
		names := module.GetAutomaticTypeDirectiveNames(state.options, state.host)
		encoded := []any{}
		for _, name := range names {
			encoded = append(encoded, p1hex(name))
		}
		// Order is the pin's own: the wildcard expands in the position it was
		// written (resolver.go:2122-2131), so this array is ordered data.
		row["names"] = encoded

	case "package_json_cache_entries":
		resolver := state.requireResolver()
		type entry struct {
			key, directory string
			exists         bool
		}
		collected := []entry{}
		resolver.PackageJsonCacheEntries(func(key tspath.Path, value *packagejson.InfoCacheEntry) bool {
			collected = append(collected, entry{string(key), value.GetDirectory(), value.Exists()})
			return true
		})
		// SyncMap.Range order is a Go map's order and is NOT the contract, so
		// the key set is sorted before it is recorded.
		sort.Slice(collected, func(i, j int) bool { return collected[i].key < collected[j].key })
		encoded := []any{}
		for _, item := range collected {
			encoded = append(encoded, []any{p1hex(item.key), p1hex(item.directory), item.exists})
		}
		row["entries"] = encoded

	case "entrypoints":
		resolver := state.requireResolver()
		scope := resolver.GetPackageScopeForPath(p1require(action.Directory, "directory", action.Op))
		entrypoints := resolver.GetEntrypointsFromPackageJsonInfo(
			scope,
			p1require(action.PackageName, "package_name", action.Op),
			p1require(action.EnableDirectorySearch, "enable_directory_search", action.Op),
		)
		encoded := []any{}
		for _, entrypoint := range entrypoints {
			encoded = append(encoded, []any{
				p1hex(entrypoint.ResolvedFileName),
				p1hex(entrypoint.OriginalFileName),
				p1hex(entrypoint.SymlinkOrRealpath()),
				p1hex(entrypoint.ModuleSpecifier),
				int64(entrypoint.Ending),
			})
		}
		row["entrypoints"] = encoded

	case "compiler_options_with_redirect":
		options := p1decodeOptions(action.Options)
		var redirect module.ResolvedProjectReference
		if action.Redirect != nil {
			redirect = p1reference(action.Redirect, action.Op)
		}
		effective := module.GetCompilerOptionsWithRedirect(options, redirect)
		row["same_pointer_as_base"] = effective == options
		row["module_resolution"] = int64(effective.GetModuleResolutionKind())
		row["trace_resolution"] = effective.TraceResolution == core.TSTrue

	case "versioned_types_key":
		results := []any{}
		for _, key := range action.Keys {
			results = append(results, []any{p1hex(key), module.IsApplicableVersionedTypesKey(key)})
		}
		row["keys"] = results

	case "mangle_scoped":
		results := []any{}
		for _, name := range action.Names {
			results = append(results, []any{p1hex(name), p1hex(module.MangleScopedPackageName(name))})
		}
		row["names"] = results

	case "types_package_name":
		results := []any{}
		for _, name := range action.Names {
			results = append(results, []any{p1hex(name), p1hex(module.GetTypesPackageName(name))})
		}
		row["names"] = results

	case "unmangle_scoped":
		results := []any{}
		for _, name := range action.Names {
			results = append(results, []any{p1hex(name), p1hex(module.UnmangleScopedPackageName(name))})
		}
		row["names"] = results

	case "package_name_from_types_package_name":
		results := []any{}
		for _, name := range action.Names {
			results = append(results, []any{p1hex(name), p1hex(module.GetPackageNameFromTypesPackageName(name))})
		}
		row["names"] = results

	case "parse_package_name":
		results := []any{}
		for _, name := range action.Names {
			packageName, rest := module.ParsePackageName(name)
			results = append(results, []any{p1hex(name), p1hex(packageName), p1hex(rest)})
		}
		row["names"] = results

	case "parse_node_module_from_path":
		results := []any{}
		for _, input := range action.Inputs {
			path := p1require(input.Path, "path", action.Op)
			isFolder := p1require(input.IsFolder, "is_folder", action.Op)
			results = append(results, []any{p1hex(path), isFolder, p1hex(module.ParseNodeModuleFromPath(path, isFolder))})
		}
		row["inputs"] = results

	case "compare_pattern_keys":
		keys := slices.Clone(action.Keys)
		slices.SortFunc(keys, module.ComparePatternKeys)
		sorted := []any{}
		for _, key := range keys {
			sorted = append(sorted, p1hex(key))
		}
		// Ordered data: the sort IS the operation.
		row["sorted"] = sorted

	case "conditions":
		options := p1decodeOptions(action.Options)
		conditions := module.GetConditions(options, core.ResolutionMode(p1require(action.Mode, "mode", action.Op)))
		encoded := []any{}
		for _, condition := range conditions {
			encoded = append(encoded, p1hex(condition))
		}
		row["conditions"] = encoded

	case "resolution_diagnostic":
		options := p1decodeOptions(action.Options)
		spec := action.Resolved
		if spec == nil {
			panic("phase1: resolution_diagnostic without a resolved module")
		}
		resolved := &module.ResolvedModule{
			Extension:                    p1require(spec.Extension, "extension", action.Op),
			ResolvedUsingExtraExtensions: p1require(spec.ResolvedUsingExtraExtensions, "resolved_using_extra_extensions", action.Op),
		}
		file := &ast.SourceFile{}
		file.IsDeclarationFile = p1require(action.IsDeclarationFile, "is_declaration_file", action.Op)
		message := module.GetResolutionDiagnostic(options, resolved, file)
		if message == nil {
			row["code"] = nil
		} else {
			row["code"] = int64(message.Code())
		}

	case "is_resolved":
		modules := []any{}
		for _, name := range action.ModuleFileNames {
			resolved := &module.ResolvedModule{ResolvedFileName: name}
			modules = append(modules, []any{p1hex(name), resolved.IsResolved()})
		}
		references := []any{}
		for _, name := range action.TypeReferenceFileNames {
			resolved := &module.ResolvedTypeReferenceDirective{ResolvedFileName: name}
			references = append(references, []any{p1hex(name), resolved.IsResolved()})
		}
		row["modules"] = modules
		row["type_references"] = references

	case "package_id_string":
		results := []any{}
		for _, spec := range action.PackageIds {
			id := module.PackageId{
				Name:             p1require(spec.Name, "name", action.Op),
				SubModuleName:    p1require(spec.SubModuleName, "sub_module_name", action.Op),
				Version:          p1require(spec.Version, "version", action.Op),
				PeerDependencies: p1require(spec.PeerDependencies, "peer_dependencies", action.Op),
			}
			results = append(results, []any{p1hex(id.PackageName()), p1hex(id.String())})
		}
		row["package_ids"] = results

	case "parsed_patterns":
		paths := p1decodePaths(action.Paths)
		patterns := module.TryParsePatterns(paths)
		results := []any{}
		for _, candidate := range action.Candidates {
			matched := module.MatchPatternOrExact(patterns, candidate)
			// MatchedText panics on a non-matching pattern
			// (core/pattern.go:30-33), so it is only asked of a valid match.
			matchedText := ""
			if matched.IsValid() {
				matchedText = matched.MatchedText(candidate)
			}
			results = append(results, []any{
				p1hex(candidate),
				matched.IsValid(),
				p1hex(matched.Text),
				int64(matched.StarIndex),
				p1hex(matchedText),
			})
		}
		row["candidates"] = results

	default:
		panic(fmt.Sprintf("phase1: action %q is not served by the module probe", action.Op))
	}
}

func p1decodeActions(raw json.RawMessage) []p1Action {
	var actions []p1Action
	if err := json.Unmarshal(raw, &actions); err != nil {
		panic(fmt.Sprintf("phase1: actions do not decode: %v", err))
	}
	if len(actions) == 0 {
		panic("phase1: a case must carry at least one action")
	}
	return actions
}

func TestPhase1ConfigModule(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []p1Request `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if !strings.HasPrefix(request.Case, p1CasePrefix) {
			row["result"] = "native_unavailable"
			row["reason"] = "case is not served by the module probe"
			observations = append(observations, row)
			continue
		}
		if request.Subject != p1Subject {
			t.Fatalf("case %s declares subject %q, which the module probe does not serve",
				request.Case, request.Subject)
		}
		row["result"] = "observed"
		row["observation"] = map[string]any{"ordered": p1replay(p1decodeActions(request.Actions))}
		observations = append(observations, row)
	}

	hash := sha256.Sum256(input)
	output := map[string]any{
		"request_sha256": hex.EncodeToString(hash[:]),
		"go":             runtime.Version(),
		"goos":           runtime.GOOS,
		"goarch":         runtime.GOARCH,
		"version":        1,
		"observations":   observations,
	}
	data, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
