package tsoptions_test

// Pure test-renderer bridge. No parser, resolver, diagnostic formatter, or
// expected-output reader runs here. Rust supplies all result values and the
// bytes produced by its production diagnostic writer. The same conversion is
// applied to the native witness before checking the frozen envelope.
import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"fmt"
	"math"
	"os"
	"reflect"
	"runtime"
	"strconv"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/json"
)

func bridgeWire(value any) any {
	if value == nil {
		return []any{"nil"}
	}
	switch v := value.(type) {
	case *collections.OrderedMap[string, any]:
		if v == nil {
			return []any{"nil"}
		}
		entries := []any{}
		for k, v := range v.Entries() {
			entries = append(entries, []any{hex.EncodeToString([]byte(k)), bridgeWire(v)})
		}
		return []any{"map", entries}
	case *collections.OrderedMap[string, []string]:
		if v == nil {
			return []any{"nil"}
		}
		entries := []any{}
		for k, v := range v.Entries() {
			entries = append(entries, []any{hex.EncodeToString([]byte(k)), bridgeWire(v)})
		}
		return []any{"map", entries}
	}
	v := reflect.ValueOf(value)
	switch v.Kind() {
	case reflect.Pointer, reflect.Interface:
		if v.IsNil() {
			return []any{"nil"}
		}
		return bridgeWire(v.Elem().Interface())
	case reflect.String:
		return []any{"string", hex.EncodeToString([]byte(v.String()))}
	case reflect.Bool:
		return []any{"bool", v.Bool()}
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return []any{"int", v.Int()}
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return []any{"int", v.Uint()}
	case reflect.Float64:
		return []any{"float", fmt.Sprintf("%016x", math.Float64bits(v.Float()))}
	case reflect.Slice:
		if v.IsNil() {
			return []any{"slice", nil}
		}
		entries := []any{}
		for i := 0; i < v.Len(); i++ {
			entries = append(entries, bridgeWire(v.Index(i).Interface()))
		}
		return []any{"slice", entries}
	case reflect.Struct:
		fields := map[string]any{}
		for i := 0; i < v.NumField(); i++ {
			if v.Type().Field(i).IsExported() {
				fields[v.Type().Field(i).Name] = bridgeWire(v.Field(i).Interface())
			}
		}
		return []any{"struct", fields}
	default:
		panic(fmt.Sprintf("unrepresented renderer value %T", value))
	}
}
func bridgeBytes(value any) []byte {
	text, ok := value.(string)
	if !ok {
		panic("renderer bytes must be hex")
	}
	result, err := hex.DecodeString(text)
	if err != nil {
		panic(err)
	}
	return result
}
func bridgeDecode(value any) any {
	row, ok := value.([]any)
	if !ok || len(row) == 0 {
		panic("renderer value must be tagged")
	}
	tag, ok := row[0].(string)
	if !ok {
		panic("renderer tag must be string")
	}
	if tag == "nil" {
		if len(row) != 1 {
			panic("nil payload")
		}
		return nil
	}
	if len(row) != 2 {
		panic("renderer value has wrong arity")
	}
	switch tag {
	case "string":
		return string(bridgeBytes(row[1]))
	case "bool":
		v, ok := row[1].(bool)
		if !ok {
			panic("not bool")
		}
		return v
	case "int":
		v, ok := row[1].(stdjson.Number)
		if !ok {
			panic("not number")
		}
		n, err := v.Int64()
		if err != nil {
			panic(err)
		}
		return n
	case "float":
		v, ok := row[1].(string)
		if !ok || len(v) != 16 {
			panic("float must be IEEE-754 hex")
		}
		bits, err := strconv.ParseUint(v, 16, 64)
		if err != nil {
			panic(err)
		}
		return math.Float64frombits(bits)
	case "slice":
		if row[1] == nil {
			return []any(nil)
		}
		values, ok := row[1].([]any)
		if !ok {
			panic("not slice")
		}
		result := make([]any, len(values))
		for i, v := range values {
			result[i] = bridgeDecode(v)
		}
		return result
	case "map":
		entries, ok := row[1].([]any)
		if !ok {
			panic("not map entries")
		}
		result := collections.NewOrderedMapWithSizeHint[string, any](len(entries))
		for _, entry := range entries {
			pair, ok := entry.([]any)
			if !ok || len(pair) != 2 {
				panic("bad map pair")
			}
			key := string(bridgeBytes(pair[0]))
			if result.Has(key) {
				panic("duplicate map key")
			}
			result.Set(key, bridgeDecode(pair[1]))
		}
		return result
	case "struct":
		fields, ok := row[1].(map[string]any)
		if !ok {
			panic("not fields")
		}
		result := map[string]any{}
		for name, v := range fields {
			result[name] = bridgeDecode(v)
		}
		return result
	default:
		panic("unknown renderer tag " + tag)
	}
}

// Assign only an exact, fully specified value. Reflect is representation
// conversion, not ParseCompilerOptions: no defaults, normalization or option
// interpretation are allowed to substitute for the Rust result.
func bridgeAssign(dst reflect.Value, input any) {
	if input == nil {
		if dst.Kind() != reflect.Pointer {
			panic("untyped nil for non-pointer")
		}
		dst.SetZero()
		return
	}
	if dst.Type() == reflect.TypeFor[*collections.OrderedMap[string, []string]]() {
		src, ok := input.(*collections.OrderedMap[string, any])
		if !ok {
			panic("not paths map")
		}
		result := collections.NewOrderedMapWithSizeHint[string, []string](src.Size())
		for k, v := range src.Entries() {
			var list []string
			bridgeAssign(reflect.ValueOf(&list).Elem(), v)
			result.Set(k, list)
		}
		dst.Set(reflect.ValueOf(result))
		return
	}
	if dst.Kind() == reflect.Pointer {
		dst.Set(reflect.New(dst.Type().Elem()))
		bridgeAssign(dst.Elem(), input)
		return
	}
	switch dst.Kind() {
	case reflect.Struct:
		fields, ok := input.(map[string]any)
		if !ok {
			panic("not struct")
		}
		count := 0
		for i := 0; i < dst.NumField(); i++ {
			f := dst.Type().Field(i)
			if !f.IsExported() {
				continue
			}
			count++
			v, ok := fields[f.Name]
			if !ok {
				panic("missing renderer field " + f.Name)
			}
			bridgeAssign(dst.Field(i), v)
		}
		if len(fields) != count {
			panic("unknown renderer field")
		}
	case reflect.String:
		v, ok := input.(string)
		if !ok {
			panic("not string")
		}
		dst.SetString(v)
	case reflect.Bool:
		v, ok := input.(bool)
		if !ok {
			panic("not bool")
		}
		dst.SetBool(v)
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		v, ok := input.(int64)
		if !ok || dst.OverflowInt(v) {
			panic("invalid signed value")
		}
		dst.SetInt(v)
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		v, ok := input.(int64)
		if !ok || v < 0 || dst.OverflowUint(uint64(v)) {
			panic("invalid unsigned value")
		}
		dst.SetUint(uint64(v))
	case reflect.Slice:
		values, ok := input.([]any)
		if !ok {
			panic("not slice")
		}
		if values == nil {
			dst.SetZero()
			return
		}
		dst.Set(reflect.MakeSlice(dst.Type(), len(values), len(values)))
		for i, v := range values {
			bridgeAssign(dst.Index(i), v)
		}
	default:
		panic("unsupported renderer destination " + dst.Type().String())
	}
}
func bridgeRoundtrip(value any) any {
	// Exercise exactly the same JSON crossing for native and Rust witnesses.
	b, err := stdjson.Marshal(value)
	if err != nil {
		panic(err)
	}
	decoder := stdjson.NewDecoder(bytes.NewReader(b))
	decoder.UseNumber()
	var result any
	if err := decoder.Decode(&result); err != nil {
		panic(err)
	}
	return result
}
func bridgeCommandLine(request commandLineBaselineRequest, typed map[string]any) map[string]any {
	expected := []string{"compiler", "build", "files", "errors", "diagnostics"}
	if len(typed) != len(expected) {
		panic("wrong command-line renderer field count")
	}
	for _, key := range expected {
		if _, ok := typed[key]; !ok {
			panic("missing renderer input " + key)
		}
	}
	var files []string
	bridgeAssign(reflect.ValueOf(&files).Elem(), bridgeDecode(typed["files"]))
	errorText := string(bridgeBytes(typed["errors"]))
	var rendered string
	switch request.Operation {
	case commandLineOperation:
		if typed["build"] != nil {
			panic("build options in ordinary parse")
		}
		values, ok := bridgeDecode(typed["compiler"]).(*collections.OrderedMap[string, any])
		if !ok {
			panic("ordinary options must be ordered map")
		}
		b, err := json.Marshal(values)
		if err != nil {
			panic(err)
		}
		rendered = formatNewBaseline(request.Args, b, strings.Join(files, ","), errorText)
	case buildOptionsOperation:
		var compiler core.CompilerOptions
		var build core.BuildOptions
		bridgeAssign(reflect.ValueOf(&compiler).Elem(), bridgeDecode(typed["compiler"]))
		bridgeAssign(reflect.ValueOf(&build).Elem(), bridgeDecode(typed["build"]))
		c, err := json.Marshal(&compiler)
		if err != nil {
			panic(err)
		}
		b, err := json.Marshal(&build)
		if err != nil {
			panic(err)
		}
		rendered = formatNewBaselineBuild(request.Args, b, c, strings.Join(files, ","), errorText)
	default:
		panic("unknown renderer operation")
	}
	sum := sha256.Sum256([]byte(rendered))
	return map[string]any{"baseline": request.Baseline, "typed": typed, "rendered": rendered, "rendered_sha256": hex.EncodeToString(sum[:])}
}

var bridgeRenderers = map[string]func(stdjson.RawMessage, map[string]any) map[string]any{}

func TestPhase1ConfigRender(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests     []stdjson.RawMessage `json:"requests"`
		Observations []map[string]any     `json:"observations"`
	}
	decoder := stdjson.NewDecoder(bytes.NewReader(input))
	decoder.UseNumber()
	if err := decoder.Decode(&document); err != nil {
		t.Fatal(err)
	}
	if len(document.Requests) != len(document.Observations) {
		t.Fatal("renderer schedule size")
	}
	for i, rawRequest := range document.Requests {
		var request commandLineBaselineRequest
		if err := stdjson.Unmarshal(rawRequest, &request); err != nil {
			t.Fatal(err)
		}
		row := document.Observations[i]
		if row["case"] != request.Case || row["operation"] != request.Operation {
			t.Fatal("renderer schedule mismatch")
		}
		if request.Operation != commandLineOperation && request.Operation != buildOptionsOperation && bridgeRenderers[request.Operation] == nil {
			continue
		}
		if row["result"] != "observed" {
			continue
		}
		typed, ok := row["observation"].(map[string]any)
		if !ok {
			t.Fatal("missing typed result")
		}
		if renderer := bridgeRenderers[request.Operation]; renderer != nil {
			row["observation"] = renderer(rawRequest, typed)
		} else {
			row["observation"] = bridgeCommandLine(request, typed)
		}
	}
	sum := sha256.Sum256(input)
	result := map[string]any{"version": 1, "request_sha256": hex.EncodeToString(sum[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "observations": document.Observations}
	b, err := stdjson.MarshalIndent(result, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), b, 0600); err != nil {
		t.Fatal(err)
	}
}

func TestPhase1RendererContract(t *testing.T) {
	mustPanic := func(name string, f func()) {
		t.Run(name, func(t *testing.T) {
			defer func() {
				if recover() == nil {
					t.Fatal("malformed typed state accepted")
				}
			}()
			f()
		})
	}
	for _, mutation := range []string{"missing", "unknown", "wrong-type"} {
		mustPanic(mutation, func() {
			fields := bridgeDecode(bridgeRoundtrip(bridgeWire(&core.CompilerOptions{}))).(map[string]any)
			switch mutation {
			case "missing":
				delete(fields, "Strict")
			case "unknown":
				fields["Invented"] = int64(0)
			case "wrong-type":
				fields["Strict"] = false
			}
			var options core.CompilerOptions
			bridgeAssign(reflect.ValueOf(&options).Elem(), fields)
		})
	}
	mustPanic("duplicate-map-name", func() {
		bridgeDecode(bridgeRoundtrip([]any{"map", []any{[]any{"61", []any{"nil"}}, []any{"61", []any{"nil"}}}}))
	})
	opts := core.CompilerOptions{Paths: collections.NewOrderedMapWithSizeHint[string, []string](2)}
	opts.Paths.Set("z", nil)
	opts.Paths.Set("a", []string{})
	opts.Types = []string{}
	var round core.CompilerOptions
	bridgeAssign(reflect.ValueOf(&round).Elem(), bridgeDecode(bridgeRoundtrip(bridgeWire(&opts))))
	if !reflect.DeepEqual(bridgeWire(&opts), bridgeWire(&round)) {
		t.Fatal("nil/empty/order changed")
	}
	request := commandLineBaselineRequest{Operation: buildOptionsOperation, Args: []string{}}
	observation := func(strict core.Tristate, files []string) map[string]any {
		return bridgeCommandLine(request, bridgeRoundtrip(map[string]any{
			"compiler": bridgeWire(&core.CompilerOptions{Strict: strict}), "build": bridgeWire(&core.BuildOptions{}),
			"files": bridgeWire(files), "errors": "", "diagnostics": []any{},
		}).(map[string]any))
	}
	before := observation(core.TSUnknown, []string{"a", "b"})
	for _, after := range []map[string]any{observation(core.TSFalse, []string{"a", "b"}), observation(core.TSUnknown, []string{"b", "a"})} {
		if reflect.DeepEqual(before["typed"], after["typed"]) || before["rendered"] == after["rendered"] {
			t.Fatal("changed typed input disappeared in renderer")
		}
	}
}
