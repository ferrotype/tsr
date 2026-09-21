package tspath

// Access only: replays an ordered action trace against the pinned tspath
// package and records what each action observed. It computes nothing of its
// own beyond rendering, and it never carries an expected value.
//
// It is an IN-PACKAGE test file, unlike the F1a leaf probes, because a third
// of this group's operations are unexported: getAnyExtensionFromPathWorker,
// getCommonParentsWorker, getFileUrlVolumeSeparatorEnd,
// getNormalizedPathComponentsFromCombined, hasRelativePathSegment,
// isAnyDirectorySeparator, pathComponents, reducePathComponents,
// simpleNormalizePath, trimRuneCount, tryGetExtensionFromPath and
// ComparePathsOptions.getEqualityComparer are reachable no other way. That
// choice also fixes what this file cannot reach: internal/symlinks imports
// tspath (upstream/tsc/internal/symlinks/knownsymlinks.go:10), so an
// in-package test file here cannot import it without an import cycle, and the
// KnownSymlinks operations need a probe of their own in internal/symlinks.
//
// Every name this file declares is prefixed `phase1`, because package tspath
// and its pinned test files share one scope and a later pin must not collide
// with a probe helper.
//
// Byte payloads are hex on the wire in both directions. encoding/json replaces
// invalid UTF-8 with U+FFFD, and the rows that matter here carry exactly those
// bytes: an argument travels as UTF-8 text under its own key or as hex under
// `<key>_hex`, and exactly one of the two must be present. An absent argument
// is a harness failure and not a defaulted observation, which is why the
// actions decode into a raw key map rather than into a struct whose missing
// fields would silently read as the zero value. An unknown action is a harness
// failure for the same reason: a row both sides could agree on without
// executing anything is worse than no row.
//
// Two rules keep a recorded observation portable. Anything derived from a Go
// map is sorted before it is recorded -- GetCommonParents returns its ignored
// set as a map and that iteration order is randomized, not contractual. A
// panic the Go runtime raises is reduced to a CLASS, because its wording names
// a dynamic length and no Rust port could reproduce the sentence, while a
// panic the pinned source raises itself keeps its literal text, because that
// text is the contract (GetCommonParents's "minComponents must be at least 1").

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"slices"
	"strings"
	"testing"
)

// phase1Action is one action's raw key map. Decoding into a map rather than a
// struct is what makes a missing key observable: a struct would read `path`
// as "" for an action that never carried one.
type phase1Action map[string]json.RawMessage

type phase1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared family schedule and
	// another group's actions need not fit this group's shapes.
	Actions json.RawMessage `json:"actions"`
}

func phase1Hex(value []byte) string {
	return hex.EncodeToString(value)
}

func phase1HexAll(values []string) []any {
	out := make([]any, 0, len(values))
	for _, value := range values {
		out = append(out, phase1Hex([]byte(value)))
	}
	return out
}

func phase1Raw(a phase1Action, key string) json.RawMessage {
	raw, ok := a[key]
	if !ok {
		panic("phase1: action is missing the key " + key)
	}
	return raw
}

func phase1JSONString(raw json.RawMessage, key string) string {
	var out string
	if err := json.Unmarshal(raw, &out); err != nil {
		panic("phase1: key " + key + " is not a JSON string: " + err.Error())
	}
	return out
}

// bytes reads a byte argument: UTF-8 text under `key`, or hex under
// `key+"_hex"`. Exactly one of the two must be present.
func (a phase1Action) bytes(key string) string {
	plain, hasPlain := a[key]
	encoded, hasEncoded := a[key+"_hex"]
	if hasPlain == hasEncoded {
		panic("phase1: action must carry exactly one of " + key + " and " + key + "_hex")
	}
	if hasPlain {
		return phase1JSONString(plain, key)
	}
	decoded, err := hex.DecodeString(phase1JSONString(encoded, key+"_hex"))
	if err != nil {
		panic("phase1: key " + key + "_hex is malformed: " + err.Error())
	}
	return string(decoded)
}

// optional reads a byte argument whose key must be present but whose value may
// be JSON null, which is how a trace says "no stop location".
func (a phase1Action) optional(key string) *string {
	var out *string
	if err := json.Unmarshal(phase1Raw(a, key), &out); err != nil {
		panic("phase1: key " + key + " is not a JSON string or null: " + err.Error())
	}
	return out
}

func (a phase1Action) op() string {
	return phase1JSONString(phase1Raw(a, "op"), "op")
}

func (a phase1Action) number(key string) int {
	var out int
	if err := json.Unmarshal(phase1Raw(a, key), &out); err != nil {
		panic("phase1: key " + key + " is not a JSON integer: " + err.Error())
	}
	return out
}

func (a phase1Action) flag(key string) bool {
	var out bool
	if err := json.Unmarshal(phase1Raw(a, key), &out); err != nil {
		panic("phase1: key " + key + " is not a JSON boolean: " + err.Error())
	}
	return out
}

// list keeps the difference between a JSON null and an empty array: the pinned
// extension operations branch on `len(extensions) > 0`, and a nil list and an
// empty one reach that test as different arguments.
func (a phase1Action) list(key string) []string {
	var out []string
	if err := json.Unmarshal(phase1Raw(a, key), &out); err != nil {
		panic("phase1: key " + key + " is not a JSON array of strings: " + err.Error())
	}
	return out
}

func (a phase1Action) lists(key string) [][]string {
	var out [][]string
	if err := json.Unmarshal(phase1Raw(a, key), &out); err != nil {
		panic("phase1: key " + key + " is not a JSON array of arrays: " + err.Error())
	}
	return out
}

func phase1Actions(raw json.RawMessage) []phase1Action {
	if len(raw) == 0 {
		panic("phase1: missing actions")
	}
	var out []phase1Action
	if err := json.Unmarshal(raw, &out); err != nil {
		// Decoding happens before any pinned call. A malformed request must
		// fail the probe, never turn into an empty observation.
		panic("phase1: invalid action payload: " + err.Error())
	}
	if len(out) == 0 {
		panic("phase1: empty action trace")
	}
	return out
}

// phase1Guarded runs f and converts a panic into a recorded class, so a case
// whose subject is the panic boundary observes it instead of failing.
func phase1Guarded(f func() any) (result any, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			panicked = phase1Classify(r)
		}
	}()
	return f(), ""
}

func phase1Classify(r any) string {
	text := ""
	switch value := r.(type) {
	case error:
		text = value.Error()
	case string:
		text = value
	default:
		text = "non-error panic"
	}
	switch {
	case strings.Contains(text, "index out of range"),
		strings.Contains(text, "slice bounds out of range"):
		// The runtime spells out the offending index and the length, which is
		// toolchain wording rather than pinned source, so only the shape is
		// recorded. RemoveExtension and pathComponents reach this by slicing
		// without a guard.
		return "index_out_of_range"
	case strings.Contains(text, "nil pointer dereference"),
		strings.Contains(text, "invalid memory address"):
		return "nil_pointer_dereference"
	default:
		// Reached by a panic the pinned source raises itself, whose literal
		// text is the contract: GetCommonParents's "minComponents must be at
		// least 1".
		return "other:" + text
	}
}

func phase1Options(a phase1Action, key string) ComparePathsOptions {
	return ComparePathsOptions{
		UseCaseSensitiveFileNames: a.flag(key),
		CurrentDirectory:          a.bytes("cwd"),
	}
}

// phase1Walk replays one ancestor walk. The callback always carries a value
// out and stops only at the row's stop location, which is what makes the
// difference between "the callback's result" and "the zero value" observable.
func phase1Walk(path string, stop *string) ([]any, string, bool) {
	visited := []any{}
	returned, ok := ForEachAncestorDirectory(path, func(directory string) (string, bool) {
		visited = append(visited, phase1Hex([]byte(directory)))
		return "v:" + directory, stop != nil && directory == *stop
	})
	return visited, returned, ok
}

func phase1WalkPath(path string, stop *string) ([]any, string, bool) {
	visited := []any{}
	returned, ok := ForEachAncestorDirectoryPath(Path(path), func(directory Path) (string, bool) {
		visited = append(visited, phase1Hex([]byte(directory)))
		return "v:" + string(directory), stop != nil && string(directory) == *stop
	})
	return visited, returned, ok
}

func phase1WalkGlobalCache(globalCache string, path string, stop *string) ([]any, string) {
	visited := []any{}
	returned := ForEachAncestorDirectoryStoppingAtGlobalCache(
		globalCache,
		path,
		func(directory string) (string, bool) {
			visited = append(visited, phase1Hex([]byte(directory)))
			return "v:" + directory, stop != nil && directory == *stop
		},
	)
	return visited, returned
}

// phase1Rooted answers the five root-shaped predicates over one input.
func phase1Rooted(a phase1Action, row map[string]any) {
	path := a.bytes("path")
	row["path_is_absolute"] = PathIsAbsolute(path)
	row["is_rooted_disk_path"] = IsRootedDiskPath(path)
	row["is_url"] = IsUrl(path)
	row["is_disk_path_root"] = IsDiskPathRoot(path)
	row["is_dynamic_file_name"] = IsDynamicFileName(path)
}

func phase1RowAlgebra(a phase1Action, op string, row map[string]any) bool {
	switch op {
	case "encoded_root_length":
		path := a.bytes("path")
		row["encoded"] = GetEncodedRootLength(path)
		row["root"] = GetRootLength(path)
	case "normalize_slashes":
		row["result"] = phase1Hex([]byte(NormalizeSlashes(a.bytes("path"))))
	case "combine":
		row["result"] = phase1Hex([]byte(CombinePaths(a.bytes("first"), a.list("paths")...)))
	case "directory_path":
		row["result"] = phase1Hex([]byte(GetDirectoryPath(a.bytes("path"))))
	case "normalize_path":
		row["result"] = phase1Hex([]byte(NormalizePath(a.bytes("path"))))
	case "resolve_path":
		row["result"] = phase1Hex([]byte(ResolvePath(a.bytes("path"), a.list("paths")...)))
	case "normalized_absolute_path":
		row["result"] = phase1Hex([]byte(GetNormalizedAbsolutePath(a.bytes("path"), a.bytes("cwd"))))
	case "normalized_absolute_path_without_root":
		row["result"] = phase1Hex([]byte(
			GetNormalizedAbsolutePathWithoutRoot(a.bytes("path"), a.bytes("cwd"))))
	case "to_file_name_lower_case":
		row["result"] = phase1Hex([]byte(ToFileNameLowerCase(a.bytes("path"))))
	case "canonical_file_name":
		row["result"] = phase1Hex([]byte(GetCanonicalFileName(
			a.bytes("path"), a.flag("use_case_sensitive_file_names"))))
	case "to_path":
		row["result"] = phase1Hex([]byte(ToPath(
			a.bytes("file"), a.bytes("base"), a.flag("case_sensitive"))))
	case "base_file_name":
		row["result"] = phase1Hex([]byte(GetBaseFileName(a.bytes("path"))))
	case "has_extension":
		row["result"] = HasExtension(a.bytes("path"))
	case "resolve_tripleslash_reference":
		row["result"] = phase1Hex([]byte(ResolveTripleslashReference(
			a.bytes("module_name"), a.bytes("containing_file"))))
	case "convert_to_relative_path":
		row["result"] = phase1Hex([]byte(ConvertToRelativePath(
			a.bytes("path"), phase1Options(a, "use_case_sensitive_file_names"))))
	case "relative_to_directory_or_url":
		options := ComparePathsOptions{
			UseCaseSensitiveFileNames: a.flag("case_sensitive"),
			CurrentDirectory:          a.bytes("cwd"),
		}
		row["result"] = phase1Hex([]byte(GetRelativePathToDirectoryOrUrl(
			a.bytes("from"), a.bytes("to"), a.flag("absolute_path_as_url"), options)))
	default:
		return false
	}
	return true
}

func phase1RowRoots(a phase1Action, op string, row map[string]any) bool {
	switch op {
	case "root_predicates":
		phase1Rooted(a, row)
	case "byte_predicates":
		value := a.number("byte")
		if value < 0 || value > 255 {
			panic("phase1: byte argument out of range")
		}
		row["is_volume_character"] = IsVolumeCharacter(byte(value))
		row["is_any_directory_separator"] = isAnyDirectorySeparator(byte(value))
	case "file_url_volume_separator_end":
		row["result"] = getFileUrlVolumeSeparatorEnd(a.bytes("url"), a.number("start"))
	case "split_volume_path":
		volume, rest, ok := SplitVolumePath(a.bytes("path"))
		row["result"] = []any{phase1Hex([]byte(volume)), phase1Hex([]byte(rest)), ok}
	case "module_name_shape":
		path := a.bytes("path")
		row["ensure_non_module_name"] = phase1Hex([]byte(EnsurePathIsNonModuleName(path)))
		row["is_external_module_name_relative"] = IsExternalModuleNameRelative(path)
	case "trailing_separator_family":
		path := a.bytes("path")
		row["has"] = HasTrailingDirectorySeparator(path)
		row["remove"] = phase1Hex([]byte(RemoveTrailingDirectorySeparator(path)))
		row["remove_all"] = phase1Hex([]byte(RemoveTrailingDirectorySeparators(path)))
		row["ensure"] = phase1Hex([]byte(EnsureTrailingDirectorySeparator(path)))
		row["path_remove"] = phase1Hex([]byte(Path(path).RemoveTrailingDirectorySeparator()))
		row["path_ensure"] = phase1Hex([]byte(Path(path).EnsureTrailingDirectorySeparator()))
	case "contains_ignored_path":
		row["result"] = ContainsIgnoredPath(a.bytes("path"))
	case "starts_with_directory":
		row["result"] = StartsWithDirectory(
			a.bytes("file"), a.bytes("directory"), a.flag("case_sensitive"))
	default:
		return false
	}
	return true
}

func phase1RowComponents(a phase1Action, op string, row map[string]any) bool {
	switch op {
	case "path_components":
		row["components"] = phase1HexAll(GetPathComponents(a.bytes("path"), a.bytes("cwd")))
		row["panic"] = ""
	case "path_components_split":
		path, rootLength := a.bytes("path"), a.number("root_length")
		value, panicked := phase1Guarded(func() any {
			return phase1HexAll(pathComponents(path, rootLength))
		})
		row["components"], row["panic"] = value, panicked
	case "reduce_path_components":
		row["components"] = phase1HexAll(reducePathComponents(a.list("components")))
	case "normalized_components_from_combined":
		row["components"] = phase1HexAll(getNormalizedPathComponentsFromCombined(a.bytes("path")))
	case "path_components_relative_to":
		options := ComparePathsOptions{
			UseCaseSensitiveFileNames: a.flag("case_sensitive"),
			CurrentDirectory:          a.bytes("cwd"),
		}
		row["components"] = phase1HexAll(
			GetPathComponentsRelativeTo(a.bytes("from"), a.bytes("to"), options))
	case "simple_normalize_path":
		result, ok := simpleNormalizePath(a.bytes("path"))
		row["result"], row["ok"] = phase1Hex([]byte(result)), ok
	case "has_relative_path_segment":
		row["result"] = hasRelativePathSegment(a.bytes("path"))
	case "trim_rune_count":
		row["result"] = phase1Hex([]byte(trimRuneCount(a.bytes("s"), a.number("rune_count"))))
	case "common_parents":
		row["parents"], row["ignored"], row["panic"] = phase1CommonParents(a)
	case "common_parents_worker":
		options := phase1Options(a, "use_case_sensitive_file_names")
		groups := getCommonParentsWorker(a.lists("groups"), a.number("min_components"), options)
		rendered := make([]any, 0, len(groups))
		for _, group := range groups {
			rendered = append(rendered, phase1HexAll(group))
		}
		row["groups"] = rendered
	default:
		return false
	}
	return true
}

// phase1CommonParents renders the exported entry point. The ignored set is a
// Go map, so it is recorded sorted: its iteration order is randomized and is
// not the contract, while the parents keep the order the worker produced.
func phase1CommonParents(a phase1Action) (any, any, string) {
	paths := a.list("paths")
	minComponents := a.number("min_components")
	options := phase1Options(a, "use_case_sensitive_file_names")
	value, panicked := phase1Guarded(func() any {
		parents, ignored := GetCommonParents(paths, minComponents, GetPathComponents, options)
		keys := make([]string, 0, len(ignored))
		for key := range ignored {
			keys = append(keys, key)
		}
		slices.Sort(keys)
		return []any{phase1HexAll(parents), phase1HexAll(keys)}
	})
	if panicked != "" {
		return nil, nil, panicked
	}
	pair, ok := value.([]any)
	if !ok || len(pair) != 2 {
		panic("phase1: common parents rendering lost its shape")
	}
	return pair[0], pair[1], ""
}

func phase1RowCompare(a phase1Action, op string, row map[string]any) bool {
	switch op {
	case "compare_paths_wrappers":
		left, right, cwd := a.bytes("a"), a.bytes("b"), a.bytes("cwd")
		row["sensitive"] = ComparePathsCaseSensitive(left, right, cwd)
		row["insensitive"] = ComparePathsCaseInsensitive(left, right, cwd)
	case "path_comparer":
		options := ComparePathsOptions{
			UseCaseSensitiveFileNames: a.flag("use_case_sensitive_file_names"),
		}
		row["result"] = options.GetComparer()(a.bytes("left"), a.bytes("right"))
	case "path_equality_comparer":
		options := ComparePathsOptions{
			UseCaseSensitiveFileNames: a.flag("use_case_sensitive_file_names"),
		}
		row["result"] = options.getEqualityComparer()(a.bytes("left"), a.bytes("right"))
	case "path_comparer_sort":
		options := ComparePathsOptions{
			UseCaseSensitiveFileNames: a.flag("use_case_sensitive_file_names"),
		}
		items := slices.Clone(a.list("items"))
		slices.SortStableFunc(items, options.GetComparer())
		row["items"] = phase1HexAll(items)
	case "compare_number_of_directory_separators":
		row["result"] = CompareNumberOfDirectorySeparators(a.bytes("left"), a.bytes("right"))
	case "typed_path_contains":
		parent, child := a.bytes("parent"), a.bytes("child")
		row["typed"] = Path(parent).ContainsPath(Path(child))
		row["free_case_sensitive"] = ContainsPath(parent, child,
			ComparePathsOptions{UseCaseSensitiveFileNames: true})
		row["free_case_insensitive"] = ContainsPath(parent, child,
			ComparePathsOptions{UseCaseSensitiveFileNames: false})
	case "typed_path_directory":
		path := a.bytes("path")
		row["typed"] = phase1Hex([]byte(Path(path).GetDirectoryPath()))
		row["free"] = phase1Hex([]byte(GetDirectoryPath(path)))
	default:
		return false
	}
	return true
}

func phase1RowAncestors(a phase1Action, op string, row map[string]any) bool {
	switch op {
	case "ancestor_walk":
		visited, _, _ := phase1Walk(a.bytes("path"), nil)
		row["visited"] = visited
	case "ancestor_walk_stop_at":
		visited, returned, ok := phase1Walk(a.bytes("path"), a.optional("stop_at"))
		row["visited"], row["returned"], row["ok"] = visited, phase1Hex([]byte(returned)), ok
	case "ancestor_walk_path":
		if dialect := a.bytes("dialect"); dialect != "path" {
			panic("phase1: the Path-typed walk refuses dialect " + dialect)
		}
		visited, returned, ok := phase1WalkPath(a.bytes("path"), a.optional("stop_at"))
		row["visited"], row["returned"], row["ok"] = visited, phase1Hex([]byte(returned)), ok
	case "ancestor_walk_stopping_at_global_cache":
		visited, returned := phase1WalkGlobalCache(
			a.bytes("global_cache"), a.bytes("path"), a.optional("stop_at"))
		row["visited"], row["returned"] = visited, phase1Hex([]byte(returned))
	default:
		return false
	}
	return true
}

func phase1RowExtensions(a phase1Action, op string, row map[string]any) bool {
	switch op {
	case "extension_is_ts":
		row["result"] = ExtensionIsTs(a.bytes("ext"))
	case "extension_is_one_of":
		row["result"] = ExtensionIsOneOf(a.bytes("ext"), a.list("extensions"))
	case "file_extension_is":
		row["result"] = FileExtensionIs(a.bytes("path"), a.bytes("extension"))
	case "file_extension_is_one_of":
		row["result"] = FileExtensionIsOneOf(a.bytes("path"), a.list("extensions"))
	case "extension_family_predicates":
		path := a.bytes("path")
		row["ts"] = HasTSFileExtension(path)
		row["js"] = HasJSFileExtension(path)
		row["json"] = HasJSONFileExtension(path)
		row["implementation_ts"] = HasImplementationTSFileExtension(path)
	case "is_declaration_file_name":
		row["result"] = IsDeclarationFileName(a.bytes("path"))
	case "remove_file_extension":
		row["result"] = phase1Hex([]byte(RemoveFileExtension(a.bytes("path"))))
	case "extension_extract_tables":
		phase1ExtractTables(a.bytes("path"), row)
	default:
		return phase1RowExtensionRewrite(a, op, row)
	}
	return true
}

func phase1ExtractTables(path string, row map[string]any) {
	row["try_get_extension_from_path"] = phase1Hex([]byte(TryGetExtensionFromPath(path)))
	row["try_extract_ts_extension"] = phase1Hex([]byte(TryExtractTSExtension(path)))
	row["declaration_file_extension"] = phase1Hex([]byte(GetDeclarationFileExtension(path)))
	row["declaration_emit_extension"] = phase1Hex([]byte(GetDeclarationEmitExtensionForPath(path)))
	row["possible_original_input_extensions"] = phase1HexAll(
		GetPossibleOriginalInputExtensionForExtension(path))
}

func phase1RowExtensionRewrite(a phase1Action, op string, row map[string]any) bool {
	switch op {
	case "any_extension_from_path":
		row["result"] = phase1Hex([]byte(GetAnyExtensionFromPath(
			a.bytes("path"), a.list("extensions"), a.flag("ignore_case"))))
	case "longest_extension_from_path":
		row["result"] = phase1Hex([]byte(GetLongestExtensionFromPath(
			a.bytes("path"), a.list("extensions"), a.flag("ignore_case"))))
	case "any_extension_worker":
		row["result"] = phase1Hex([]byte(getAnyExtensionFromPathWorker(
			a.bytes("path"), a.list("extensions"), phase1Comparer(a))))
	case "try_get_extension_from_path":
		row["result"] = phase1Hex([]byte(tryGetExtensionFromPath(
			a.bytes("path"), a.bytes("extension"), phase1Comparer(a))))
	case "change_extension":
		row["result"] = phase1Hex([]byte(ChangeExtension(a.bytes("path"), a.bytes("ext"))))
	case "change_any_extension":
		row["result"] = phase1Hex([]byte(ChangeAnyExtension(
			a.bytes("path"), a.bytes("ext"), a.list("extensions"), a.flag("ignore_case"))))
	case "change_full_extension":
		row["result"] = phase1Hex([]byte(ChangeFullExtension(a.bytes("path"), a.bytes("ext"))))
	case "remove_extension":
		path, extension := a.bytes("path"), a.bytes("extension")
		value, panicked := phase1Guarded(func() any {
			return phase1Hex([]byte(RemoveExtension(path, extension)))
		})
		row["result"], row["panic"] = value, panicked
	case "remove_any_file_extension":
		row["result"] = phase1Hex([]byte(RemoveAnyFileExtension(a.bytes("path"))))
	default:
		return false
	}
	return true
}

// phase1Comparer builds the equality rule the unexported extension helpers
// take as an argument. The rule travels in the request as `ignore_case`,
// because a rule that lived only in this file would leave a port reading the
// frozen row with no way to know which comparison was made.
func phase1Comparer(a phase1Action) func(x, y string) bool {
	if a.flag("ignore_case") {
		return strings.EqualFold
	}
	return func(x, y string) bool { return x == y }
}

func phase1Row(a phase1Action) map[string]any {
	op := a.op()
	row := map[string]any{"op": op}
	claimed := phase1RowAlgebra(a, op, row) ||
		phase1RowRoots(a, op, row) ||
		phase1RowComponents(a, op, row) ||
		phase1RowCompare(a, op, row) ||
		phase1RowAncestors(a, op, row) ||
		phase1RowExtensions(a, op, row)
	if !claimed {
		panic("phase1: unsupported action: " + op)
	}
	return row
}

func phase1Replay(request phase1Request) []any {
	rows := []any{}
	for _, a := range phase1Actions(request.Actions) {
		rows = append(rows, phase1Row(a))
	}
	return rows
}

func TestPhase1FilesystemTspath(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []phase1Request `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject == "tspath" {
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": phase1Replay(request)}
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the tspath probe"
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
	data, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
