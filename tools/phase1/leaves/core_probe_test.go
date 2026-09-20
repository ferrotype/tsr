package core

// Access only: replays ordered action traces against the pinned internal/core
// leaf functions and records what each action observed.
//
// It is an in-package test file, like the pinned pattern_test.go next to it.
// Everything it calls is exported, so nothing forces that choice; it keeps the
// probe free of an import of the package under test and adds no helper the
// pinned tests could pick up, because every identifier here is phase1-prefixed.
//
// Every probe is handed the whole family schedule, including the traces of
// groups it does not serve, so actions are decoded lazily: one group's typed
// action struct must never be able to reject another group's request. For the
// same reason the numeric arguments here are named for what they are
// (`tristate`, `kind`) rather than taking the generic `value` key that a
// neighbouring group already uses for a string.
//
// Byte payloads are hex on the wire in both directions. encoding/json replaces
// invalid UTF-8 with U+FFFD when it marshals a Go string, so a raw observation
// of pattern text would silently lose exactly the bytes those cases exist to
// pin. Results are ordered lists, because the comparison canonicalises with
// sorted keys and a JSON object's member order would not survive it.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"strings"
	"testing"
)

type phase1Action struct {
	Op              string   `json:"op"`
	Text            string   `json:"text"`
	Hex             string   `json:"hex"`
	Raw             string   `json:"raw"`
	Tristate        int64    `json:"tristate"`
	TristateDefault int64    `json:"tristate_default"`
	Kind            int64    `json:"kind"`
	Pos             int64    `json:"pos"`
	End             int64    `json:"end"`
	Pos2            int64    `json:"pos2"`
	End2            int64    `json:"end2"`
	Flag            bool     `json:"flag"`
	Patterns        []string `json:"patterns"`
	Values          []int64  `json:"values"`
}

type phase1Request struct {
	Case      string          `json:"case"`
	Operation string          `json:"operation"`
	Subject   string          `json:"subject"`
	Actions   json.RawMessage `json:"actions"`
}

// phase1Trace decodes a trace only for a subject this probe serves.
func phase1Trace(t *testing.T, request phase1Request) []phase1Action {
	t.Helper()
	if len(request.Actions) == 0 {
		return nil
	}
	var trace []phase1Action
	if err := json.Unmarshal(request.Actions, &trace); err != nil {
		t.Fatalf("case %q has an action trace this probe cannot read: %v", request.Case, err)
	}
	return trace
}

// phase1Input is an action's single string argument: hex when the case needs a
// byte string the JSON request cannot carry, plain text otherwise.
func phase1Input(t *testing.T, a phase1Action) []byte {
	t.Helper()
	if a.Hex == "" {
		return []byte(a.Text)
	}
	decoded, err := hex.DecodeString(a.Hex)
	if err != nil {
		t.Fatalf("action %q carries malformed hex %q: %v", a.Op, a.Hex, err)
	}
	return decoded
}

func phase1Hex(value []byte) string {
	return hex.EncodeToString(value)
}

func phase1Numbers(values []int64) []any {
	out := []any{}
	for _, value := range values {
		out = append(out, value)
	}
	return out
}

// phase1Guarded runs f and converts a panic into a recorded class, so a case
// whose subject is the panic boundary observes it instead of failing the probe.
func phase1Guarded(f func() any) (result any, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			panicked = phase1Classify(r)
		}
	}()
	return f(), ""
}

// phase1Classify reduces a panic to a class. The Go and Rust wordings for the
// same failure differ, so only the class is comparable; an unrecognized panic
// keeps its text under an `other:` prefix rather than being folded into a
// class it does not belong to.
func phase1Classify(r any) string {
	text := ""
	switch value := r.(type) {
	case error:
		text = value.Error()
	case string:
		text = value
	default:
		text = "non-string panic payload"
	}
	switch {
	case strings.Contains(text, "slice bounds out of range"),
		strings.Contains(text, "index out of range"):
		return "slice_bounds_out_of_range"
	case text == "candidate does not match pattern":
		return "candidate_does_not_match_pattern"
	default:
		return "other:" + text
	}
}

func phase1Tristate(trace []phase1Action) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "predicates":
			t := Tristate(byte(a.Tristate))
			row["tristate"] = a.Tristate
			row["is_true"] = t.IsTrue()
			row["is_false"] = t.IsFalse()
			row["is_unknown"] = t.IsUnknown()
			row["is_true_or_unknown"] = t.IsTrueOrUnknown()
			row["is_false_or_unknown"] = t.IsFalseOrUnknown()
		case "default_if_unknown":
			t := Tristate(byte(a.Tristate))
			row["tristate"] = a.Tristate
			row["tristate_default"] = a.TristateDefault
			row["result"] = int64(t.DefaultIfUnknown(Tristate(byte(a.TristateDefault))))
		case "bool_to_tristate":
			row["flag"] = a.Flag
			row["result"] = int64(BoolToTristate(a.Flag))
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1TristateJSON(trace []phase1Action) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "marshal":
			encoded, err := json.Marshal(Tristate(byte(a.Tristate)))
			row["tristate"] = a.Tristate
			row["json"] = string(encoded)
			row["error"] = err != nil
		case "unmarshal":
			// The operation under test, called directly on the exact bytes.
			var t Tristate
			err := t.UnmarshalJSON([]byte(a.Raw))
			row["raw"] = a.Raw
			row["result"] = int64(t)
			row["error"] = err != nil
		case "unmarshal_via_json":
			// The same operation reached through encoding/json, which decides
			// what bytes the method ever sees and can fail before calling it.
			var t Tristate
			err := json.Unmarshal([]byte(a.Raw), &t)
			row["raw"] = a.Raw
			row["result"] = int64(t)
			row["error"] = err != nil
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1TextRange(trace []phase1Action) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new":
			r := NewTextRange(int(a.Pos), int(a.End))
			row["pos_in"] = a.Pos
			row["end_in"] = a.End
			row["pos"] = int64(r.Pos())
			row["end"] = int64(r.End())
			row["len"] = int64(r.Len())
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1TextRangePredicates(trace []phase1Action) []any {
	var first, second TextRange
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "undefined":
			u := UndefinedTextRange()
			row["pos"] = int64(u.Pos())
			row["end"] = int64(u.End())
			row["is_valid"] = u.IsValid()
		case "make":
			first = NewTextRange(int(a.Pos), int(a.End))
			row["pos"] = int64(first.Pos())
			row["end"] = int64(first.End())
			row["is_valid"] = first.IsValid()
		case "make2":
			second = NewTextRange(int(a.Pos2), int(a.End2))
			row["pos"] = int64(second.Pos())
			row["end"] = int64(second.End())
			row["is_valid"] = second.IsValid()
		case "contains":
			row["pos"] = a.Pos
			row["contains"] = first.Contains(int(a.Pos))
			row["contains_inclusive"] = first.ContainsInclusive(int(a.Pos))
			row["contains_exclusive"] = first.ContainsExclusive(int(a.Pos))
		case "relate":
			row["contained_by"] = first.ContainedBy(second)
			row["overlaps"] = first.Overlaps(second)
			row["intersects"] = first.Intersects(second)
			row["compare"] = int64(CompareTextRanges(first, second))
		case "with_pos":
			moved := first.WithPos(int(a.Pos))
			row["pos"] = int64(moved.Pos())
			row["end"] = int64(moved.End())
		case "with_end":
			moved := first.WithEnd(int(a.End))
			row["pos"] = int64(moved.Pos())
			row["end"] = int64(moved.End())
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1ScriptKind(t *testing.T, trace []phase1Action) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "from_file_name":
			name := phase1Input(t, a)
			row["name_hex"] = phase1Hex(name)
			row["kind"] = int64(GetScriptKindFromFileName(string(name)))
		case "ensure_from_file_name":
			name := phase1Input(t, a)
			row["name_hex"] = phase1Hex(name)
			row["kind"] = int64(EnsureScriptKindFromFileName(string(name)))
		case "default_extension":
			row["kind"] = a.Kind
			row["extension"] = GetDefaultExtensionForScriptKind(ScriptKind(int32(a.Kind)))
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1Pattern(t *testing.T, trace []phase1Action) []any {
	// The zero Pattern before any parse, which is the invalid value itself.
	var pattern Pattern
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "parse":
			pattern = TryParsePattern(string(phase1Input(t, a)))
			row["text_hex"] = phase1Hex([]byte(pattern.Text))
			row["star_index"] = int64(pattern.StarIndex)
			row["is_valid"] = pattern.IsValid()
		case "is_valid":
			row["is_valid"] = pattern.IsValid()
		case "matches":
			candidate := phase1Input(t, a)
			row["candidate_hex"] = phase1Hex(candidate)
			value, panicked := phase1Guarded(func() any { return pattern.Matches(string(candidate)) })
			row["result"], row["panic"] = value, panicked
		case "matched_text":
			candidate := phase1Input(t, a)
			row["candidate_hex"] = phase1Hex(candidate)
			value, panicked := phase1Guarded(func() any {
				return phase1Hex([]byte(pattern.MatchedText(string(candidate))))
			})
			row["result"], row["panic"] = value, panicked
		case "find_best":
			candidate := phase1Input(t, a)
			names := a.Patterns
			// Values are 1-based indices into `patterns`, so the zero value the
			// pinned generic returns when nothing matches stays distinguishable
			// from a selected element. An index outside the list is a malformed
			// request, not a pinned behavior, so it fails the probe rather than
			// being answered with a Pattern the probe made up; the Rust driver
			// rejects the same trace.
			for _, value := range a.Values {
				if value < 1 || value > int64(len(names)) {
					t.Fatalf("find_best index %d is outside the %d pattern(s) of action %q",
						value, len(names), a.Op)
				}
			}
			get := func(value int64) Pattern {
				return TryParsePattern(names[value-1])
			}
			row["candidate_hex"] = phase1Hex(candidate)
			row["values"] = phase1Numbers(a.Values)
			value, panicked := phase1Guarded(func() any {
				return FindBestPatternMatch(a.Values, get, string(candidate))
			})
			row["result"], row["panic"] = value, panicked
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func TestPhase1LeavesCore(t *testing.T) {
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
		var replayed []any
		switch request.Subject {
		case "core.Tristate":
			replayed = phase1Tristate(phase1Trace(t, request))
		case "core.TristateJson":
			replayed = phase1TristateJSON(phase1Trace(t, request))
		case "core.TextRange":
			replayed = phase1TextRange(phase1Trace(t, request))
		case "core.TextRangePredicates":
			replayed = phase1TextRangePredicates(phase1Trace(t, request))
		case "core.ScriptKind":
			replayed = phase1ScriptKind(t, phase1Trace(t, request))
		case "core.Pattern":
			replayed = phase1Pattern(t, phase1Trace(t, request))
		default:
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the core probe"
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
