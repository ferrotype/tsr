package packagejson

// Access only: replays an ordered action trace against the pinned
// internal/packagejson package and records what each action observed.
//
// The subject of this group is the part of internal/packagejson that the s07
// package.json dataset never reaches. That dataset drives `Parse` and reads
// the parsed fields, so it witnesses the parse, the two unmarshalers, the
// `Expected` accessors the adapter calls and the whole `ExportsOrImports`
// object-kind machine. It never builds an info cache, never asks a field what
// type it *expected*, never calls the dependency helpers and never asks for
// the typesVersions mappings. Those are what the actions below drive.
//
// This file is an IN-PACKAGE test file (`package packagejson`), not an
// external one. All four pinned _test.go files in this directory are
// `package packagejson_test`, so there is no in-package test file to collide
// with, and being in-package is what lets the probe read `Expected`'s
// unexported `actualJSONType` field directly instead of going through
// `ActualJSONType()`, which the s07 dataset already witnesses.
//
// -trimpath must be off for this probe. `go test ./internal/packagejson`
// compiles the external `packagejson_test` package into the same binary, and
// its package-level `packageJsonFixtures` var calls `repo.RootPath()` at init
// (packagejson_test.go:20-22). `repo` panics with "repo root cannot be found
// when built with -trimpath" (internal/repo/paths.go:19), so the binary dies
// before any test runs, whatever `-run` selects. `run_probe` takes
// `trimpath=False` for exactly this reason
// (scripts/phase1_capture.py:501-519) and records the choice in provenance.
//
// Five rules keep an observation portable.
//
// Nothing derived from a Go map or from sync.Map iteration is recorded in
// iteration order. `DependencyFields.RangeDependencies` ranges four Go maps,
// `GetRuntimeDependencyNames` returns a `collections.Set` whose Keys() is a
// map, and `InfoCache.Range` walks a `collections.SyncMap`. Each of those is
// recorded sorted, and the deterministic part -- which of the four dependency
// *fields* was entered, in what order, and how many entries a stopped Range
// visited -- is recorded separately. An early-stopped Range over more than one
// entry records its count only: which entry it happened to see is scheduling,
// not semantics.
//
// Every panic is reduced to a class. The pinned accessors panic with
// fmt.Sprintf("expected string, got %v", v.Type) (jsonvalue.go:80) and the
// nil-receiver WithPackageDirectory raises a runtime panic whose text names
// dynamic types and moves with the toolchain, so only the class crosses the
// boundary.
//
// A diagnostic is recorded as its code plus its rendered arguments, never as
// message text. The text is locale data; the code is the stable identity, and
// `crates/tsr_diagnostics` carries the same codes.
//
// Pointer identity is recorded as a boolean, never as an address.
//
// No wall clock, host path or temporary directory reaches an observation: the
// paths the cache actions use are synthetic strings ("/repo/..."), and nothing
// here touches a filesystem.

import (
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"fmt"
	"os"
	"runtime"
	"sort"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

// Every name declared here is prefixed. This is an in-package file in a
// package that other Phase 1 work may also overlay, and an unprefixed
// `action` or `state` would be a collision waiting to happen.

// phase1SubjectName is the request subject this probe serves. Every other
// subject in the shared config schedule is declined.
const phase1SubjectName = "packageJson"

// phase1FieldNames is the ordered vocabulary the field actions address. It is
// every `Expected[T]` the pinned `Fields` carries, including the three inside
// `ContentMapperFields`, in declaration order (packagejson.go:8-121). Both
// sides walk this list, so a port that grew or lost a field shows up as a
// length difference rather than as a silently skipped row.
var phase1FieldNames = []string{
	"name", "version", "type",
	"tsconfig", "main", "types", "typings",
	"dependencies", "devDependencies", "peerDependencies", "optionalDependencies",
	"contentMapper",
	"contentMapper.exec", "contentMapper.compilerOptions", "contentMapper.dynamicConfig",
}

// phase1Validated returns the field as the pinned `TypeValidatedField`
// interface (validated.go:3-8), which is how resolver.go:1877-1883 reaches
// these methods: the concrete type is erased and only the four members remain.
func phase1Validated(f *Fields, name string) TypeValidatedField {
	switch name {
	case "name":
		return &f.Name
	case "version":
		return &f.Version
	case "type":
		return &f.Type
	case "tsconfig":
		return &f.TSConfig
	case "main":
		return &f.Main
	case "types":
		return &f.Types
	case "typings":
		return &f.Typings
	case "dependencies":
		return &f.Dependencies
	case "devDependencies":
		return &f.DevDependencies
	case "peerDependencies":
		return &f.PeerDependencies
	case "optionalDependencies":
		return &f.OptionalDependencies
	case "contentMapper":
		return &f.ContentMapper
	case "contentMapper.exec":
		return &f.ContentMapper.Value.Exec
	case "contentMapper.compilerOptions":
		return &f.ContentMapper.Value.CompilerOptions
	case "contentMapper.dynamicConfig":
		return &f.ContentMapper.Value.DynamicConfig
	default:
		panic("phase1: unknown field name: " + name)
	}
}

// phase1JSONValue returns one of the three untyped JSON-valued fields.
// `typesVersions` is a bare JSONValue; `exports` and `imports` are
// ExportsOrImports, which embeds JSONValue, so the accessors this group drives
// reach them by promotion -- which is exactly how resolver.go:2172 and
// resolver.go:2253 call them.
func phase1JSONValue(f *Fields, name string) *JSONValue {
	switch name {
	case "typesVersions":
		return &f.TypesVersions
	case "exports":
		return &f.Exports.JSONValue
	case "imports":
		return &f.Imports.JSONValue
	default:
		panic("phase1: unknown JSON-valued field: " + name)
	}
}

type phase1Action struct {
	Op string `json:"op"`
	// Every payload field is a pointer. An absent key is a malformed request
	// and must fail the probe: a field that silently defaulted would let both
	// sides agree on a row neither of them actually executed.
	Source        *string `json:"source"`
	Field         *string `json:"field"`
	Name          *string `json:"name"`
	Value         *int    `json:"value"`
	Kind          *string `json:"kind"`
	Receiver      *string `json:"receiver"`
	Directory     *string `json:"directory"`
	Path          *string `json:"path"`
	Label         *string `json:"label"`
	Exists        *bool   `json:"exists"`
	CaseSensitive *bool   `json:"case_sensitive"`
	StopAfter     *int    `json:"stop_after"`
}

func (a phase1Action) needString(field string, raw *string) string {
	if raw == nil {
		panic("phase1: action " + a.Op + " requires `" + field + "`")
	}
	return *raw
}

func (a phase1Action) needInt(field string, raw *int) int {
	if raw == nil {
		panic("phase1: action " + a.Op + " requires `" + field + "`")
	}
	return *raw
}

func (a phase1Action) needBool(field string, raw *bool) bool {
	if raw == nil {
		panic("phase1: action " + a.Op + " requires `" + field + "`")
	}
	return *raw
}

type phase1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared family schedule and
	// another group's actions need not fit this group's action type.
	Actions stdjson.RawMessage `json:"actions"`
}

func phase1Decode(raw stdjson.RawMessage) []phase1Action {
	if len(raw) == 0 {
		panic("phase1: missing actions")
	}
	var out []phase1Action
	if err := stdjson.Unmarshal(raw, &out); err != nil {
		panic("phase1: invalid action payload: " + err.Error())
	}
	if len(out) == 0 {
		panic("phase1: empty action trace")
	}
	return out
}

// phase1Classify renders a recovered panic as a class. Nothing is recorded
// verbatim: the accessor panics embed the JSONValueType name through %v, and
// the runtime's nil-dereference text names dynamic types and moves with the
// toolchain.
func phase1Classify(recovered any) string {
	var text string
	switch value := recovered.(type) {
	case nil:
		return ""
	case error:
		text = value.Error()
	case string:
		text = value
	default:
		return "non_error_panic"
	}
	switch {
	case strings.HasPrefix(text, "expected string, got "):
		return "as_string_off_type"
	case strings.HasPrefix(text, "expected object, got "):
		return "as_object_off_type"
	case strings.HasPrefix(text, "expected array, got "):
		return "as_array_off_type"
	case strings.Contains(text, "nil pointer dereference"),
		strings.Contains(text, "invalid memory address"):
		return "nil_pointer_dereference"
	default:
		return "other:" + text
	}
}

func phase1GuardedString(f func() string) (result string, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = ""
			panicked = phase1Classify(r)
		}
	}()
	return f(), ""
}

func phase1GuardedEntry(f func() *InfoCacheEntry) (result *InfoCacheEntry, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			panicked = phase1Classify(r)
		}
	}()
	return f(), ""
}

// phase1Entries turns the pinned entry vocabulary into concrete receivers.
// `nil` is the typed nil the three guarded readers are written for; the others
// vary the two members those readers look at.
func phase1Entry(kind string) *InfoCacheEntry {
	switch kind {
	case "nil":
		return nil
	case "empty_contents":
		return &InfoCacheEntry{PackageDirectory: "/repo/pkg", DirectoryExists: true, Contents: nil}
	case "with_contents":
		return &InfoCacheEntry{PackageDirectory: "/repo/pkg", DirectoryExists: true, Contents: &PackageJson{}}
	case "with_contents_absent_directory":
		return &InfoCacheEntry{PackageDirectory: "/repo/pkg", DirectoryExists: false, Contents: &PackageJson{}}
	case "empty_directory":
		return &InfoCacheEntry{PackageDirectory: "", DirectoryExists: false, Contents: &PackageJson{}}
	default:
		panic("phase1: unknown entry receiver: " + kind)
	}
}

// phase1ExpectedOf drives the pinned generic constructor at each type argument
// the pin instantiates `Expected[T]` with. It reports the constructed field's
// exported state plus the unexported `actualJSONType` the constructor writes,
// read directly rather than through `ActualJSONType()`, which the s07 dataset
// already witnesses.
func phase1ExpectedOf(kind string) (valid bool, null bool, actual string, expected string) {
	switch kind {
	case "string":
		e := ExpectedOf("sample")
		return e.Valid, e.Null, e.actualJSONType, e.ExpectedJSONType()
	case "empty_string":
		e := ExpectedOf("")
		return e.Valid, e.Null, e.actualJSONType, e.ExpectedJSONType()
	case "string_map":
		e := ExpectedOf(map[string]string(nil))
		return e.Valid, e.Null, e.actualJSONType, e.ExpectedJSONType()
	case "string_slice":
		e := ExpectedOf([]string(nil))
		return e.Valid, e.Null, e.actualJSONType, e.ExpectedJSONType()
	case "bool":
		e := ExpectedOf(false)
		return e.Valid, e.Null, e.actualJSONType, e.ExpectedJSONType()
	case "content_mapper_fields":
		e := ExpectedOf(ContentMapperFields{})
		return e.Valid, e.Null, e.actualJSONType, e.ExpectedJSONType()
	default:
		panic("phase1: unknown ExpectedOf kind: " + kind)
	}
}

// phase1NilExpectedJSONType calls ExpectedJSONType on a typed nil receiver,
// which is what ExpectedOf itself does (expected.go:74). The method reads only
// its type parameter, never `e`, so the call is defined; this records that it
// is, rather than asserting it.
func phase1NilExpectedJSONType(kind string) (result string, panicked string) {
	switch kind {
	case "string":
		return phase1GuardedString(func() string { return (*Expected[string])(nil).ExpectedJSONType() })
	case "string_map":
		return phase1GuardedString(func() string {
			return (*Expected[map[string]string])(nil).ExpectedJSONType()
		})
	case "string_slice":
		return phase1GuardedString(func() string { return (*Expected[[]string])(nil).ExpectedJSONType() })
	case "bool":
		return phase1GuardedString(func() string { return (*Expected[bool])(nil).ExpectedJSONType() })
	case "content_mapper_fields":
		return phase1GuardedString(func() string {
			return (*Expected[ContentMapperFields])(nil).ExpectedJSONType()
		})
	default:
		panic("phase1: unknown ExpectedOf kind: " + kind)
	}
}

// phase1DependencyFieldOrder is the order RangeDependencies enters the four
// fields (packagejson.go:58-87). It is the deterministic part of that walk;
// the names inside one field come out of a Go map and are sorted before they
// are recorded.
var phase1DependencyFieldOrder = map[string]int{
	"dependencies":         0,
	"devDependencies":      1,
	"peerDependencies":     2,
	"optionalDependencies": 3,
}

type phase1Visit struct {
	order   int
	name    string
	version string
	field   string
}

type phase1State struct {
	fields Fields
	loaded bool
	// A fresh PackageJson per `load`, so each document gets an unfired
	// sync.Once and the first-call semantics of GetVersionPaths are reachable
	// more than once in a trace.
	pkg *PackageJson
	// Retained so that GetPaths' memoisation into the *retrieved value* is
	// observable across actions.
	versionPaths    VersionPaths
	haveVersion     bool
	cache           *InfoCache
	cacheSensitive  bool
	cacheDirectory  string
	lastStoredEntry *InfoCacheEntry
}

func (s *phase1State) needFields() *Fields {
	if !s.loaded {
		panic("phase1: the trace read a field before a `load` action parsed a document")
	}
	return &s.fields
}

func (s *phase1State) needPackage() *PackageJson {
	if s.pkg == nil {
		panic("phase1: the trace used the package before a `load` action parsed a document")
	}
	return s.pkg
}

func (s *phase1State) needVersionPaths() *VersionPaths {
	if !s.haveVersion {
		panic("phase1: the trace read version paths before a `get_version_paths` action retrieved them")
	}
	return &s.versionPaths
}

func (s *phase1State) needCache() *InfoCache {
	if s.cache == nil {
		panic("phase1: the trace used the cache before a `new_info_cache` action built it")
	}
	return s.cache
}

func phase1Strings(in []string) []any {
	out := []any{}
	for _, value := range in {
		out = append(out, value)
	}
	return out
}

//nolint:gocyclo // One switch, one arm per action; splitting it would hide the vocabulary.
func phase1Replay(request phase1Request) []any {
	state := &phase1State{}
	ordered := []any{}
	for _, a := range phase1Decode(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "load":
			// Substrate, not subject: the parse is packagejson.go:Parse, which
			// the s07 dataset already witnesses. Only whether it succeeded is
			// recorded, because a later action's result is meaningless if it
			// did not.
			source := a.needString("source", a.Source)
			parsed, err := Parse([]byte(source))
			state.fields = parsed
			state.loaded = true
			state.pkg = &PackageJson{Fields: parsed, Parseable: err == nil}
			state.haveVersion = false
			state.versionPaths = VersionPaths{}
			row["parsed"] = err == nil

		case "expected_is_valid":
			// The interface's validity member, over every declared field. A
			// field that was never present and a field whose JSON was the
			// wrong type both answer false; the duplicate-name documents are
			// where they come apart.
			result := []any{}
			for _, name := range phase1FieldNames {
				result = append(result, []any{name, phase1Validated(state.needFields(), name).IsValid()})
			}
			row["result"] = result

		case "expected_json_type":
			// Answered from the declared type parameter alone
			// (expected.go:51-67), so it is recorded against whatever document
			// happens to be loaded and must not vary with it.
			result := []any{}
			for _, name := range phase1FieldNames {
				result = append(result, []any{name, phase1Validated(state.needFields(), name).ExpectedJSONType()})
			}
			row["result"] = result

		case "expected_of":
			kind := a.needString("kind", a.Kind)
			valid, null, actual, expected := phase1ExpectedOf(kind)
			row["kind"] = kind
			// actual_json_type is the unexported field the constructor writes,
			// read directly; expected_json_type is what the constructor asked
			// for. They are recorded side by side because the constructor sets
			// the first from the second (expected.go:74), which is what makes
			// a never-parsed field report as present.
			row["result"] = []any{valid, null, actual, expected, actual == expected}

		case "nil_receiver_expected_json_type":
			kind := a.needString("kind", a.Kind)
			result, panicked := phase1NilExpectedJSONType(kind)
			row["kind"] = kind
			row["result"] = []any{panicked, result}

		case "json_value_type_string":
			value := a.needInt("value", a.Value)
			if value < -128 || value > 127 {
				panic("phase1: json_value_type_string value is outside int8")
			}
			row["value"] = value
			row["result"] = JSONValueType(int8(value)).String()

		case "json_value_is_present":
			// Absent and explicitly null are different states here: the pinned
			// predicate tests against JSONValueTypeNotPresent only
			// (jsonvalue.go:46-48), so `"exports": null` is present.
			field := a.needString("field", a.Field)
			value := phase1JSONValue(state.needFields(), field)
			row["field"] = field
			row["result"] = []any{value.IsPresent(), int(value.Type)}

		case "json_value_as_string":
			field := a.needString("field", a.Field)
			value := phase1JSONValue(state.needFields(), field)
			// The accessor has a value receiver, so it is called on a copy;
			// the copy is taken here rather than through the field expression
			// so the call site is the same for all three fields.
			held := *value
			text, panicked := phase1GuardedString(func() string { return held.AsString() })
			row["field"] = field
			row["result"] = []any{panicked, text}

		case "has_dependency":
			name := a.needString("name", a.Name)
			row["name"] = name
			row["result"] = state.needFields().DependencyFields.HasDependency(name)

		case "range_dependencies":
			// The walk enters the four fields in declaration order and ranges
			// a Go map inside each, so the field sequence is recorded as
			// observed and the entries are sorted within their field. A
			// stopped walk is only deterministic when the field it stops in
			// holds a single entry; the requests that stop are built that way
			// and the case prose says so.
			stopAfter := a.needInt("stop_after", a.StopAfter)
			visits := []phase1Visit{}
			sequence := []string{}
			state.needFields().DependencyFields.RangeDependencies(func(name, version, field string) bool {
				order, ok := phase1DependencyFieldOrder[field]
				if !ok {
					panic("phase1: RangeDependencies named an unknown field: " + field)
				}
				if len(sequence) == 0 || sequence[len(sequence)-1] != field {
					sequence = append(sequence, field)
				}
				visits = append(visits, phase1Visit{order: order, name: name, version: version, field: field})
				return stopAfter < 0 || len(visits) < stopAfter
			})
			sort.SliceStable(visits, func(i, j int) bool {
				if visits[i].order != visits[j].order {
					return visits[i].order < visits[j].order
				}
				return visits[i].name < visits[j].name
			})
			sorted := []any{}
			for _, visit := range visits {
				sorted = append(sorted, []any{visit.order, visit.name, visit.version, visit.field})
			}
			row["stop_after"] = stopAfter
			row["count"] = len(visits)
			row["field_sequence"] = phase1Strings(sequence)
			row["visits"] = sorted

		case "runtime_dependency_names":
			// devDependencies are excluded by construction
			// (packagejson.go:89-108). The set's Keys() is a Go map, so the
			// names are sorted; the length and the set's non-nilness are the
			// order-free part.
			names := state.needFields().DependencyFields.GetRuntimeDependencyNames()
			collected := []string{}
			for name := range names.Keys() {
				collected = append(collected, name)
			}
			sort.Strings(collected)
			row["result"] = []any{names != nil, names.Len(), phase1Strings(collected)}

		case "entry_readers":
			// The three guarded readers over one receiver. A typed nil answers
			// all three without panicking (cache.go:129-145).
			kind := a.needString("receiver", a.Receiver)
			entry := phase1Entry(kind)
			row["receiver"] = kind
			row["result"] = []any{entry.Exists(), entry.GetContents() == nil, entry.GetDirectory()}

		case "with_package_directory":
			// The one method on InfoCacheEntry with no nil guard: it reads
			// p.PackageDirectory before deciding anything (cache.go:158-167).
			kind := a.needString("receiver", a.Receiver)
			directory := a.needString("directory", a.Directory)
			entry := phase1Entry(kind)
			result, panicked := phase1GuardedEntry(func() *InfoCacheEntry {
				return entry.WithPackageDirectory(directory)
			})
			row["receiver"] = kind
			row["directory"] = directory
			if panicked != "" || result == nil {
				row["result"] = []any{panicked}
				break
			}
			row["result"] = []any{
				panicked,
				result == entry,
				entry != nil && result.Contents == entry.Contents,
				result.PackageDirectory,
				result.DirectoryExists,
			}

		case "with_package_directory_shares_contents":
			// The corrected copy shares the Contents pointer, so a change made
			// through one view is visible through the other. Parseable is used
			// as the marker because it is a plain exported bool on the shared
			// PackageJson and changing it parses nothing.
			original := phase1Entry("with_contents")
			copied := original.WithPackageDirectory("/repo/other")
			before := original.Contents.Parseable
			copied.Contents.Parseable = !before
			row["result"] = []any{
				copied == original,
				copied.Contents == original.Contents,
				before,
				original.Contents.Parseable,
			}

		case "new_info_cache":
			directory := a.needString("directory", a.Directory)
			caseSensitive := a.needBool("case_sensitive", a.CaseSensitive)
			state.cache = NewInfoCache(directory, caseSensitive)
			state.cacheDirectory = directory
			state.cacheSensitive = caseSensitive
			state.lastStoredEntry = nil
			row["result"] = []any{directory, caseSensitive}

		case "cache_set":
			// Entries are labelled through PackageDirectory, and the label is
			// read off the struct field rather than through GetDirectory():
			// the reader is a different case's subject and this one is about
			// which entry the cache kept.
			path := a.needString("path", a.Path)
			label := a.needString("label", a.Label)
			exists := a.needBool("exists", a.Exists)
			entry := &InfoCacheEntry{PackageDirectory: label, DirectoryExists: exists, Contents: &PackageJson{}}
			state.lastStoredEntry = entry
			actual := state.needCache().Set(path, entry)
			row["path"] = path
			row["label"] = label
			// `kept_this_entry` false means the cache already held a value for
			// this key and gave it back: LoadOrStore, not Store.
			row["result"] = []any{actual != nil, actual == entry, actual.PackageDirectory}

		case "cache_get":
			path := a.needString("path", a.Path)
			found := state.needCache().Get(path)
			row["path"] = path
			if found == nil {
				row["result"] = []any{false, ""}
				break
			}
			row["result"] = []any{true, found.PackageDirectory}

		case "cache_range":
			// SyncMap.Range is unordered. A complete walk records its labels
			// sorted; a stopped walk records only how many entries it visited,
			// because which ones it saw is scheduling.
			stopAfter := a.needInt("stop_after", a.StopAfter)
			labels := []string{}
			count := 0
			state.needCache().Range(func(_ tspath.Path, value *InfoCacheEntry) bool {
				count++
				labels = append(labels, value.PackageDirectory)
				return stopAfter < 0 || count < stopAfter
			})
			// A walk that was allowed to stop records no labels at all, even
			// when it happened to see every entry: the schedule cannot know
			// how many entries a stop would have skipped, and a label list
			// that is sometimes complete is a label list that sometimes
			// depends on scheduling.
			complete := stopAfter < 0
			sort.Strings(labels)
			row["stop_after"] = stopAfter
			row["count"] = count
			row["complete"] = complete
			if complete {
				row["labels"] = phase1Strings(labels)
			} else {
				row["labels"] = []any{}
			}

		case "get_version_paths":
			// Untraced: the sync.Once still runs, it just emits nothing.
			paths := state.needPackage().GetVersionPaths(nil)
			state.versionPaths = paths
			state.haveVersion = true
			row["result"] = []any{state.versionPaths.Exists()}

		case "get_version_paths_traced":
			// The traces are replayed from the recorded slice on every call
			// with a non-nil callback, so a second call after a first --
			// traced or not -- must produce the same sequence.
			traced := []any{}
			paths := state.needPackage().GetVersionPaths(func(m *diagnostics.Message, args ...any) {
				rendered := []any{}
				for _, arg := range args {
					rendered = append(rendered, fmt.Sprintf("%v", arg))
				}
				traced = append(traced, []any{int(m.Code()), rendered})
			})
			state.versionPaths = paths
			state.haveVersion = true
			row["exists"] = state.versionPaths.Exists()
			row["traces"] = traced

		case "version_paths_mappings":
			// Ordered: pathsJSON is a collections.OrderedMap and the mapping
			// order is part of what a port owes. A value that is not an array
			// is dropped entirely; a non-string element inside an array leaves
			// an empty string in its slot, because the slice is pre-sized and
			// the loop only `continue`s (cache.go:105-118).
			paths := state.needVersionPaths().GetPaths()
			if paths == nil {
				row["result"] = nil
				break
			}
			mappings := []any{}
			for key, values := range paths.Entries() {
				mappings = append(mappings, []any{key, phase1Strings(values)})
			}
			row["result"] = mappings

		case "version_paths_identity":
			// GetVersionPaths returns a copy of the PackageJson's VersionPaths
			// struct, and GetPaths memoises into that copy. So two reads of
			// one retrieved value share a map, and two retrievals do not.
			first := state.needVersionPaths().GetPaths()
			second := state.needVersionPaths().GetPaths()
			retrieved := state.needPackage().GetVersionPaths(nil)
			third := retrieved.GetPaths()
			sizes := []any{first != nil, second != nil, third != nil}
			if first != nil && third != nil {
				sizes = append(sizes, first.Size() == third.Size())
			} else {
				sizes = append(sizes, first == nil && third == nil)
			}
			row["result"] = []any{first == second, first == third}
			row["shape"] = sizes

		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

// A compile-time note rather than a runtime one: the probe reaches the pinned
// package's collections dependency only through GetPaths' result, so the
// import is asserted here instead of being dropped and re-added by a later
// edit.
var _ = (*collections.OrderedMap[string, []string])(nil)

func TestPhase1ConfigPackageJson(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []phase1Request `json:"requests"`
	}
	if err := stdjson.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject == phase1SubjectName {
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": phase1Replay(request)}
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the packagejson probe"
		}
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
	data, err := stdjson.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
