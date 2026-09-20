package core

// Access only: replays the compiler-option action traces against the pinned
// internal/core option getters, its reflective Clone and the three hand-written
// enum renderers, and records what each action observed.
//
// It is an in-package test file, like the pinned compileroptions_test.go next
// to it. Being in the package is what lets it build a CompilerOptions at all:
// the struct embeds an unexported noCopy, and the probe sets fields through
// reflection over the pinned declaration rather than repeating a field list
// that could drift from it.
//
// Every probe is handed the whole family schedule, including the traces of
// groups it does not serve, so actions are decoded lazily: one group's typed
// action struct must never be able to reject another group's request. For the
// same reason the numeric arguments here are named for what they are (`jsx`,
// `module_kind`, `new_line`) rather than taking a generic `value` key that a
// neighbouring group already decodes as a string.
//
// An option value travels as a JSON object keyed by the pinned `json` tags, so
// a reader can tell what an action set without reading this file. A name the
// pinned struct does not declare fails the probe: the Rust driver refuses the
// same name, so neither side can quietly ignore a field and then agree by
// accident.
//
// Two panics are reachable here -- GetEffectiveTypeRoots on an empty base, and
// the zero-value and unhandled-value arms of the two String() methods. All
// three are raised by the pinned source itself, so their literal text is the
// contract and is recorded as written, unlike the runtime panics a neighbouring
// group has to reduce to a class.
//
// Results are ordered lists, because the comparison canonicalises with sorted
// keys and a JSON object's member order would not survive it. Nothing nested
// inside a row is an object either: the per-field clone report is an entry
// list, and the two-valued GetEffectiveTypeRoots result is a pair list.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"reflect"
	"runtime"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/collections"
)

type phase1OptionsAction struct {
	Op               string                     `json:"op"`
	Options          map[string]json.RawMessage `json:"options"`
	StrictOption     string                     `json:"strict_option"`
	CurrentDirectory string                     `json:"current_directory"`
	ModuleKindValue  int64                      `json:"module_kind"`
	Jsx              int64                      `json:"jsx"`
	ModuleResolution int64                      `json:"module_resolution"`
	NewLine          int64                      `json:"new_line"`
	// FileName is the path AllowImportingTsExtensionsFrom consults; Text is
	// the newline literal GetNewLineKind classifies.
	FileName string `json:"file_name"`
	Text     string `json:"text"`
}

type phase1OptionsRequest struct {
	Case      string          `json:"case"`
	Operation string          `json:"operation"`
	Subject   string          `json:"subject"`
	Actions   json.RawMessage `json:"actions"`
}

type phase1OptionField struct {
	name  string
	index int
}

// phase1OptionRoster is the pinned struct's exported fields in declaration
// order, each keyed by the `json` tag the option is named by everywhere else.
// Clone copies exactly the exported fields (the IsExported test at compileroptions.go:187), so this is
// also the roster a clone has to preserve.
var phase1OptionRoster = phase1BuildOptionRoster()

func phase1BuildOptionRoster() []phase1OptionField {
	optionsType := reflect.TypeFor[CompilerOptions]()
	roster := make([]phase1OptionField, 0, optionsType.NumField())
	for i := range optionsType.NumField() {
		field := optionsType.Field(i)
		if !field.IsExported() {
			continue
		}
		name, _, _ := strings.Cut(field.Tag.Get("json"), ",")
		if name == "" {
			continue
		}
		roster = append(roster, phase1OptionField{name: name, index: i})
	}
	return roster
}

// phase1OptionsTrace decodes a trace only for a subject this probe serves.
func phase1OptionsTrace(t *testing.T, request phase1OptionsRequest) []phase1OptionsAction {
	t.Helper()
	if len(request.Actions) == 0 {
		return nil
	}
	var trace []phase1OptionsAction
	if err := json.Unmarshal(request.Actions, &trace); err != nil {
		t.Fatalf("case %q has an action trace this probe cannot read: %v", request.Case, err)
	}
	return trace
}

func phase1DecodeOption(t *testing.T, name string, raw json.RawMessage, target any) {
	t.Helper()
	if err := json.Unmarshal(raw, target); err != nil {
		t.Fatalf("option %q carries a value this probe cannot read: %v", name, err)
	}
}

// phase1BuildOptions applies an action's option object to a fresh value. The
// roster is walked in declaration order rather than the request map, because Go
// map iteration is randomized and nothing observable may depend on it.
func phase1BuildOptions(t *testing.T, a phase1OptionsAction) (*CompilerOptions, int) {
	t.Helper()
	options := &CompilerOptions{}
	value := reflect.ValueOf(options).Elem()
	applied := 0
	for _, field := range phase1OptionRoster {
		raw, ok := a.Options[field.name]
		if !ok {
			continue
		}
		phase1SetOption(t, value.Field(field.index), field.name, raw)
		applied++
	}
	if applied != len(a.Options) {
		t.Fatalf("action %q sets %d name(s) the pinned CompilerOptions does not declare",
			a.Op, len(a.Options)-applied)
	}
	return options, applied
}

func phase1SetOption(t *testing.T, field reflect.Value, name string, raw json.RawMessage) {
	t.Helper()
	switch field.Kind() {
	case reflect.Uint8: // Tristate, which is a byte and takes an out-of-domain one.
		var value uint64
		phase1DecodeOption(t, name, raw, &value)
		field.SetUint(value)
	case reflect.Int32: // Jsx, Module, ModuleResolution, ModuleDetection, NewLine, Target.
		var value int64
		phase1DecodeOption(t, name, raw, &value)
		field.SetInt(value)
	case reflect.String:
		var value string
		phase1DecodeOption(t, name, raw, &value)
		field.SetString(value)
	case reflect.Slice: // []string. `null` is the nil slice, `[]` the empty non-nil one.
		var values []string
		phase1DecodeOption(t, name, raw, &values)
		if values == nil {
			field.Set(reflect.Zero(field.Type()))
			return
		}
		field.Set(reflect.ValueOf(values))
	case reflect.Pointer:
		if name == "paths" {
			field.Set(reflect.ValueOf(phase1DecodePaths(t, name, raw)))
			return
		}
		var value *int
		phase1DecodeOption(t, name, raw, &value)
		if value == nil {
			field.Set(reflect.Zero(field.Type()))
			return
		}
		field.Set(reflect.ValueOf(value))
	default:
		t.Fatalf("option %q has kind %s, which this probe cannot set", name, field.Kind())
	}
}

// phase1DecodePaths reads the `paths` map as an entry list, so its insertion
// order survives the request and a nil value list stays distinguishable from an
// empty one. A `null` map and an empty one are also distinct in the request,
// though Size() reports 0 for both (ordered_map.go:190).
func phase1DecodePaths(t *testing.T, name string, raw json.RawMessage) *collections.OrderedMap[string, []string] {
	t.Helper()
	var entries [][2]json.RawMessage
	phase1DecodeOption(t, name, raw, &entries)
	if entries == nil {
		return nil
	}
	paths := collections.NewOrderedMapWithSizeHint[string, []string](len(entries))
	for _, entry := range entries {
		var key string
		phase1DecodeOption(t, name, entry[0], &key)
		var values []string
		phase1DecodeOption(t, name, entry[1], &values)
		paths.Set(key, values)
	}
	return paths
}

func phase1RenderOption(t *testing.T, field reflect.Value, name string) any {
	t.Helper()
	switch field.Kind() {
	case reflect.Uint8:
		return int64(field.Uint())
	case reflect.Int32:
		return field.Int()
	case reflect.String:
		return field.String()
	case reflect.Slice:
		if field.IsNil() {
			return nil
		}
		values := make([]string, field.Len())
		for i := range field.Len() {
			values[i] = field.Index(i).String()
		}
		return phase1OptionStrings(values)
	case reflect.Pointer:
		if field.IsNil() {
			return nil
		}
		if name == "paths" {
			entries := []any{}
			paths, ok := field.Interface().(*collections.OrderedMap[string, []string])
			if !ok {
				t.Fatalf("option %q is not the ordered path map this probe expects", name)
			}
			for key, values := range paths.Entries() {
				entries = append(entries, []any{key, phase1OptionStrings(values)})
			}
			return entries
		}
		return field.Elem().Int()
	default:
		t.Fatalf("option %q has kind %s, which this probe cannot render", name, field.Kind())
		return nil
	}
}

// phase1OptionStrings renders a string slice, keeping nil distinct from empty.
func phase1OptionStrings(values []string) any {
	if values == nil {
		return nil
	}
	out := []any{}
	for _, value := range values {
		out = append(out, value)
	}
	return out
}

// phase1OptionList renders a result slice that is never nil, so an empty
// result is the empty list rather than null.
func phase1OptionList(values []string) []any {
	out := []any{}
	for _, value := range values {
		out = append(out, value)
	}
	return out
}

// phase1OptionGuarded runs f and records a panic's literal text. Every panic
// reachable from this group is raised by the pinned source itself, so the
// wording is the contract and nothing is folded into a class.
func phase1OptionGuarded(f func() any) (result any, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			switch value := r.(type) {
			case error:
				panicked = value.Error()
			case string:
				panicked = value
			default:
				panicked = "non-string panic payload"
			}
		}
	}()
	return f(), ""
}

// phase1OptionTristate reads the named strict sub-option out of the value the
// action built. The pinned GetStrictOptionValue takes a Tristate, not a name,
// so resolving the name is the harness's job and a name that is not a tristate
// option is a malformed request rather than a pinned behavior.
func phase1OptionTristate(t *testing.T, options *CompilerOptions, name string) Tristate {
	t.Helper()
	value := reflect.ValueOf(options).Elem()
	for _, field := range phase1OptionRoster {
		if field.name != name {
			continue
		}
		slot := value.Field(field.index)
		if slot.Kind() != reflect.Uint8 {
			break
		}
		return Tristate(byte(slot.Uint()))
	}
	t.Fatalf("strict_option %q is not a tristate option of the pinned CompilerOptions", name)
	return TSUnknown
}

func phase1OptionGetters(t *testing.T, trace []phase1OptionsAction) []any {
	t.Helper()
	ordered := []any{}
	for _, a := range trace {
		options, applied := phase1BuildOptions(t, a)
		row := map[string]any{"op": a.Op, "applied": applied}
		switch a.Op {
		case "get_allow_js":
			row["result"] = options.GetAllowJS()
		case "get_emit_script_target":
			row["result"] = int64(options.GetEmitScriptTarget())
		case "get_emit_module_kind":
			row["result"] = int64(options.GetEmitModuleKind())
		case "get_module_resolution_kind":
			row["result"] = int64(options.GetModuleResolutionKind())
		case "get_emit_module_detection_kind":
			row["result"] = int64(options.GetEmitModuleDetectionKind())
		case "get_resolve_json_module":
			row["result"] = options.GetResolveJsonModule()
		case "get_resolve_package_json_exports":
			row["result"] = options.GetResolvePackageJsonExports()
		case "get_resolve_package_json_imports":
			row["result"] = options.GetResolvePackageJsonImports()
		case "get_jsx_transform_enabled":
			row["result"] = options.GetJSXTransformEnabled()
		case "get_isolated_modules":
			row["result"] = options.GetIsolatedModules()
		case "get_emit_standard_class_fields":
			row["result"] = options.GetEmitStandardClassFields()
		case "get_emit_declarations":
			row["result"] = options.GetEmitDeclarations()
		case "get_are_declaration_maps_enabled":
			row["result"] = options.GetAreDeclarationMapsEnabled()
		case "uses_wildcard_types":
			row["result"] = options.UsesWildcardTypes()
		case "get_strict_option_value":
			value := phase1OptionTristate(t, options, a.StrictOption)
			row["strict_option"] = a.StrictOption
			row["option_value"] = int64(value)
			row["result"] = options.GetStrictOptionValue(value)
		case "get_allow_importing_ts_extensions":
			row["result"] = options.GetAllowImportingTsExtensions()
		case "allow_importing_ts_extensions_from":
			row["file_name"] = a.FileName
			row["result"] = options.AllowImportingTsExtensionsFrom(a.FileName)
		case "get_use_define_for_class_fields":
			row["result"] = options.GetUseDefineForClassFields()
		case "is_incremental":
			row["result"] = options.IsIncremental()
		case "should_preserve_const_enums":
			row["result"] = options.ShouldPreserveConstEnums()
		case "get_paths_base_path":
			row["current_directory"] = a.CurrentDirectory
			row["result"] = options.GetPathsBasePath(a.CurrentDirectory)
		case "get_effective_type_roots":
			// The result is a pair, carried as a two-element list: an object
			// nested in an ordered payload would not survive canonicalisation.
			row["current_directory"] = a.CurrentDirectory
			value, panicked := phase1OptionGuarded(func() any {
				roots, fromConfig := options.GetEffectiveTypeRoots(a.CurrentDirectory)
				return []any{phase1OptionList(roots), fromConfig}
			})
			row["result"], row["panic"] = value, panicked
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1OptionClone(t *testing.T, trace []phase1OptionsAction) []any {
	t.Helper()
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "clone":
			source, applied := phase1BuildOptions(t, a)
			copied := source.Clone()
			sourceValue := reflect.ValueOf(source).Elem()
			cloneValue := reflect.ValueOf(copied).Elem()
			// Field by field, so a copy that dropped a deprecated or internal
			// field names itself instead of hiding inside a whole-struct
			// comparison.
			fields := []any{}
			for _, field := range phase1OptionRoster {
				before := phase1RenderOption(t, sourceValue.Field(field.index), field.name)
				after := phase1RenderOption(t, cloneValue.Field(field.index), field.name)
				fields = append(fields, []any{field.name, after, reflect.DeepEqual(before, after)})
			}
			row["applied"] = applied
			row["field_count"] = len(fields)
			row["fields"] = fields
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

// GetNewLineKind is package level rather than a method, so it ignores the
// option value an action carries. The text travels as a literal because every
// value the classifier distinguishes is ASCII.
func phase1OptionNewLineFromText(trace []phase1OptionsAction) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new_line_kind_from_text":
			row["text"] = a.Text
			row["result"] = int64(GetNewLineKind(a.Text))
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1OptionModuleKind(trace []phase1OptionsAction) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "is_non_node_esm":
			row["module_kind"] = a.ModuleKindValue
			row["result"] = ModuleKind(int32(a.ModuleKindValue)).IsNonNodeESM()
		case "supports_import_attributes":
			row["module_kind"] = a.ModuleKindValue
			row["result"] = ModuleKind(int32(a.ModuleKindValue)).SupportsImportAttributes()
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1OptionJsxEmitText(trace []phase1OptionsAction) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "jsx_emit_text":
			// Hand written, not generated: the two default-less arms panic with
			// the literal text recorded here, so there is no numeric fallback.
			row["jsx"] = a.Jsx
			value, panicked := phase1OptionGuarded(func() any {
				return JsxEmit(int32(a.Jsx)).String()
			})
			row["result"], row["panic"] = value, panicked
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1OptionModuleResolutionText(trace []phase1OptionsAction) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "module_resolution_text":
			row["module_resolution"] = a.ModuleResolution
			value, panicked := phase1OptionGuarded(func() any {
				return ModuleResolutionKind(int32(a.ModuleResolution)).String()
			})
			row["result"], row["panic"] = value, panicked
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1OptionNewLineText(trace []phase1OptionsAction) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new_line_character":
			// One named case and a default, so this one cannot panic.
			row["new_line"] = a.NewLine
			row["result"] = NewLineKind(int32(a.NewLine)).GetNewLineCharacter()
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func TestPhase1LeavesOptions(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []phase1OptionsRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		var replayed []any
		switch request.Subject {
		case "options.Getters":
			replayed = phase1OptionGetters(t, phase1OptionsTrace(t, request))
		case "options.Clone":
			replayed = phase1OptionClone(t, phase1OptionsTrace(t, request))
		case "options.ModuleKind":
			replayed = phase1OptionModuleKind(phase1OptionsTrace(t, request))
		case "options.JsxEmitText":
			replayed = phase1OptionJsxEmitText(phase1OptionsTrace(t, request))
		case "options.ModuleResolutionText":
			replayed = phase1OptionModuleResolutionText(phase1OptionsTrace(t, request))
		case "options.NewLineText":
			replayed = phase1OptionNewLineText(phase1OptionsTrace(t, request))
		case "options.NewLineFromText":
			replayed = phase1OptionNewLineFromText(phase1OptionsTrace(t, request))
		default:
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the options probe"
			observations = append(observations, row)
			continue
		}
		row["result"] = "observed"
		row["observation"] = map[string]any{"ordered": replayed}
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
