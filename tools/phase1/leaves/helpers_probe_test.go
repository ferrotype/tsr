package core

// Access only: replays ordered action traces against the pinned generic
// helpers in internal/core/core.go and records what each action observed.
//
// It is an in-package test file, like the pinned pattern_test.go next to it.
// In-package access is required here and not merely convenient:
// comparableValuesEqual (core.go:765) is unexported, and it is the default
// equality DiffMaps hands to DiffMapsFunc, so it cannot be reached from
// outside the package at all. Every identifier this file declares is
// phase1Helpers-prefixed, so it adds nothing the pinned tests could pick up.
//
// Every probe is handed the whole family schedule, including the traces of
// groups it does not serve, so actions are decoded lazily: one group's typed
// action struct must never be able to reject another group's request.
//
// WHAT THESE TRACES ARE FOR. These helpers are one-line generics; "Filter
// filters" is not worth a case. What is worth a case is the part of each
// pinned body that a competent Rust port silently drops:
//
//   - nil versus empty. Filter's rejecting arm returns slices.Clone of a
//     zero-length prefix, which is a NON-nil empty slice, while MapFiltered,
//     FlatMap and Flatten accumulate into `var result []U` and return nil.
//     Those marshal as [] and null and are not the same answer.
//   - return-value identity. Filter, Concatenate, AppendIfUnique, Deduplicate
//     and SameMap return an INPUT SLICE ITSELF when nothing changed, so the
//     result aliases the caller's storage. Same (core.go:183) is the pinned
//     instrument for that, and a mutate-then-read-back action is the second,
//     independent witness.
//   - how many times a callback actually ran. Some stops at the first true and
//     SameMap calls f exactly len(slice) times; both produce the right values
//     under a port that calls f for every element, and only the count differs.
//   - Memoize's create is cleared AFTER it returns, so a zero-valued result is
//     still cached and a panicking create is retried.
//
// Panics are recorded as classes, never as sentences: no Rust port could
// reproduce a Go runtime message. The one exception is a panic whose payload
// is the error a case itself supplied (Must), whose text is the case's own and
// therefore portable; it is recorded with the payload kind beside it, because
// Must panics with the error VALUE, not with a rendering of it.
//
// Byte payloads travel as hex in both directions, because StringifyJson is
// reached with invalid UTF-8 and encoding/json would replace those bytes with
// U+FFFD on the way out, losing exactly what the case pins.
//
// Results are ordered lists, because the comparison canonicalises with sorted
// keys and a JSON object's member order would not survive it.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"iter"
	"os"
	"runtime"
	"strconv"
	"strings"
	"testing"
)

type phase1HelpersAction struct {
	Op string `json:"op"`
	// Target and Other name registers of the slice machine ("s", "t", "base",
	// "result"). A register holds a slice header, so identity and the nil /
	// non-nil-empty distinction survive being stored in one.
	Target string     `json:"target"`
	Other  string     `json:"other"`
	Items  []string   `json:"items"`
	Groups [][]string `json:"groups"`
	// Nil builds a nil slice where Items would build a non-nil one. `items: []`
	// would also decode to a non-nil empty slice, but saying so explicitly
	// keeps a request readable as the distinction it is testing.
	Nil       bool   `json:"nil"`
	Flag      bool   `json:"flag"`
	Index     int    `json:"index"`
	Value     string `json:"value"`
	Fallback  string `json:"fallback"`
	Message   string `json:"message"`
	Predicate string `json:"predicate"`
	Rule      string `json:"rule"`
	Shape     string `json:"shape"`
	Prefix    string `json:"prefix"`
	Indent    string `json:"indent"`
	Hex       string `json:"hex"`
	Text      string `json:"text"`
}

type phase1HelpersRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared schedule and another
	// group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

// phase1HelpersTrace decodes a trace only once its subject has matched. A
// malformed request fails the probe; it never becomes an empty observation.
func phase1HelpersTrace(t *testing.T, request phase1HelpersRequest) []phase1HelpersAction {
	t.Helper()
	if len(request.Actions) == 0 {
		t.Fatalf("case %q has no action trace", request.Case)
	}
	var trace []phase1HelpersAction
	if err := json.Unmarshal(request.Actions, &trace); err != nil {
		t.Fatalf("case %q has an action trace this probe cannot read: %v", request.Case, err)
	}
	if len(trace) == 0 {
		t.Fatalf("case %q has an empty action trace", request.Case)
	}
	return trace
}

// phase1HelpersInput is an action's byte argument: hex when the case needs a
// byte string the JSON request cannot carry, plain text otherwise.
func phase1HelpersInput(t *testing.T, a phase1HelpersAction) []byte {
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

func phase1HelpersHex(value []byte) string {
	return hex.EncodeToString(value)
}

// phase1HelpersList renders a slice as a JSON array that is never null, so the
// nil / non-nil distinction is carried by the explicit `nil` field of the row
// rather than by the shape of `values`, where it would be easy to read as an
// accident of marshalling.
func phase1HelpersList(values []string) []any {
	out := []any{}
	for _, value := range values {
		out = append(out, value)
	}
	return out
}

// phase1HelpersResult records a slice-valued result: its contents, whether it
// is nil, and its length. Capacity is deliberately not recorded: it is the Go
// allocator's answer, not the pinned function's, and no port reproduces it.
// The effect capacity has -- append writing into a caller's backing array --
// is observed directly instead, by reading the caller's register back.
func phase1HelpersResult(row map[string]any, values []string) {
	row["values"] = phase1HelpersList(values)
	row["nil"] = values == nil
	row["len"] = len(values)
}

// phase1HelpersGuarded runs f and converts a panic into a recorded class, so a
// case whose subject is the panic boundary observes it instead of failing the
// probe. It reports the payload kind separately from the class, because Must
// panics with the error value itself and that is part of its contract.
func phase1HelpersGuarded(f func() any) (result any, class string, payload string, message string) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			class, payload, message = phase1HelpersClassify(r)
		}
	}()
	return f(), "", "", ""
}

// phase1HelpersClassify reduces a panic to a class. A Go runtime message names
// dynamic types and moves with the toolchain, so only its class is comparable
// and its text is dropped. A panic carrying an error a case itself supplied is
// different: the text is the case's own, so it survives as `message`, and the
// payload kind survives beside it because Must's contract is that a recover
// sees the error rather than a rendering of it.
func phase1HelpersClassify(r any) (class string, payload string, message string) {
	text := ""
	switch value := r.(type) {
	case error:
		payload, text = "error", value.Error()
	case string:
		payload, text = "string", value
	default:
		payload, text = "other", "non-string panic payload"
	}
	switch {
	case strings.Contains(text, "index out of range"),
		strings.Contains(text, "slice bounds out of range"):
		return "index_out_of_range", payload, ""
	case strings.Contains(text, "nil pointer dereference"),
		strings.Contains(text, "invalid memory address"):
		return "nil_pointer_dereference", payload, ""
	case text == phase1HelpersCreateFailed:
		// Raised by this probe's own memoized producer, so the class stands for
		// a request-chosen failure rather than for anything the pin says.
		return "create_panicked", payload, ""
	case payload == "error":
		return "error_value", payload, text
	default:
		// The file header promises panics are recorded as classes, never as
		// sentences. Carrying the text here would break that promise the first
		// time an unclassified runtime panic arrived, and no Rust port could
		// reproduce a Go runtime message, so the class stands alone.
		return "unclassified_runtime_panic", payload, ""
	}
}

const phase1HelpersCreateFailed = "phase1: memoized create failed"

// phase1HelpersBox is the one non-string instantiation these cases use. It
// exists for the two operations whose contract is about a pointer:
// SingleElementSlice, which returns nil for a nil element and otherwise a
// slice holding the caller's own pointer, and comparableValuesEqual, whose ==
// on a pointer-bearing struct is address equality.
type phase1HelpersBox struct {
	Name string
}

type phase1HelpersPair struct {
	Name string
	Ref  *phase1HelpersBox
}

// phase1HelpersPredicate resolves a named rule from the request. The rule
// travels in the request rather than living in this file so that a Rust driver
// reading the same request knows what the callback is; an unknown name is a
// malformed request and fails the probe, and the counter it closes over is how
// a case observes early stop.
func phase1HelpersPredicate(name string, argument string, calls *int) func(string) bool {
	var test func(string) bool
	switch name {
	case "always":
		test = func(string) bool { return true }
	case "never":
		test = func(string) bool { return false }
	case "nonempty":
		test = func(value string) bool { return value != "" }
	case "empty":
		test = func(value string) bool { return value == "" }
	case "len_gt_1":
		test = func(value string) bool { return len(value) > 1 }
	case "equals_value":
		test = func(value string) bool { return value == argument }
	default:
		panic("phase1: unsupported predicate: " + name)
	}
	return func(value string) bool {
		*calls++
		return test(value)
	}
}

func phase1HelpersMapper(name string, calls *int) func(string) string {
	var apply func(string) string
	switch name {
	case "identity":
		apply = func(value string) string { return value }
	case "upper":
		apply = strings.ToUpper
	case "upper_if_a":
		apply = func(value string) string {
			if value == "a" {
				return "A"
			}
			return value
		}
	case "upper_if_c":
		apply = func(value string) string {
			if value == "c" {
				return "C"
			}
			return value
		}
	default:
		panic("phase1: unsupported rule: " + name)
	}
	return func(value string) string {
		*calls++
		return apply(value)
	}
}

func phase1HelpersIndexMapper(name string, calls *int) func(string, int) string {
	var apply func(string, int) string
	switch name {
	case "with_index":
		apply = func(value string, index int) string { return value + ":" + strconv.Itoa(index) }
	case "identity_index":
		apply = func(value string, _ int) string { return value }
	default:
		panic("phase1: unsupported rule: " + name)
	}
	return func(value string, index int) string {
		*calls++
		return apply(value, index)
	}
}

func phase1HelpersFilterMapper(name string, calls *int) func(string) (string, bool) {
	var apply func(string) (string, bool)
	switch name {
	case "keep_nonempty":
		apply = func(value string) (string, bool) { return value, value != "" }
	case "keep_all":
		apply = func(value string) (string, bool) { return value, true }
	case "drop_all":
		apply = func(value string) (string, bool) { return value, false }
	default:
		panic("phase1: unsupported rule: " + name)
	}
	return func(value string) (string, bool) {
		*calls++
		return apply(value)
	}
}

func phase1HelpersFlatMapper(name string, calls *int) func(string) []string {
	var apply func(string) []string
	switch name {
	case "split_chars":
		apply = func(value string) []string {
			out := []string{}
			for i := range len(value) {
				out = append(out, value[i:i+1])
			}
			return out
		}
	case "single":
		apply = func(value string) []string { return []string{value} }
	case "empty":
		apply = func(string) []string { return nil }
	default:
		panic("phase1: unsupported rule: " + name)
	}
	return func(value string) []string {
		*calls++
		return apply(value)
	}
}

// phase1HelpersRegisters is the state of the slice machine: named []string
// registers, so that identity -- which is the subject of half these cases --
// can be carried from one action to the next and asked about with Same.
type phase1HelpersRegisters map[string][]string

func (r phase1HelpersRegisters) get(t *testing.T, name string) []string {
	t.Helper()
	values, ok := r[name]
	if !ok {
		t.Fatalf("action names register %q, which no earlier action set", name)
	}
	return values
}

//nolint:gocyclo
func phase1HelpersSlices(t *testing.T, trace []phase1HelpersAction) []any {
	registers := phase1HelpersRegisters{}
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		calls := 0
		switch a.Op {
		case "set":
			// A fresh backing array every time, so two registers set from the
			// same items are never accidentally the same slice.
			if a.Nil {
				registers[a.Target] = nil
			} else {
				registers[a.Target] = append([]string{}, a.Items...)
			}
			row["target"] = a.Target
			phase1HelpersResult(row, registers[a.Target])
		case "alias":
			registers[a.Target] = registers.get(t, a.Other)
			row["target"], row["other"] = a.Target, a.Other
			phase1HelpersResult(row, registers[a.Target])
		case "slice_from":
			source := registers.get(t, a.Other)
			if a.Index < 0 || a.Index > len(source) {
				t.Fatalf("slice_from index %d is outside register %q of length %d",
					a.Index, a.Other, len(source))
			}
			registers[a.Target] = source[a.Index:]
			row["target"], row["other"], row["index"] = a.Target, a.Other, a.Index
			phase1HelpersResult(row, registers[a.Target])
		case "slice_to":
			// Keeps the source's capacity, which is what lets a later
			// AppendIfUnique write past the prefix into the shared array.
			source := registers.get(t, a.Other)
			if a.Index < 0 || a.Index > len(source) {
				t.Fatalf("slice_to index %d is outside register %q of length %d",
					a.Index, a.Other, len(source))
			}
			registers[a.Target] = source[:a.Index]
			row["target"], row["other"], row["index"] = a.Target, a.Other, a.Index
			phase1HelpersResult(row, registers[a.Target])
		case "read":
			row["target"] = a.Target
			phase1HelpersResult(row, registers.get(t, a.Target))
		case "mutate":
			values := registers.get(t, a.Target)
			if a.Index < 0 || a.Index >= len(values) {
				t.Fatalf("mutate index %d is outside register %q of length %d",
					a.Index, a.Target, len(values))
			}
			values[a.Index] = a.Value
			row["target"], row["index"], row["value"] = a.Target, a.Index, a.Value
		case "same":
			row["target"], row["other"] = a.Target, a.Other
			row["same"] = Same(registers.get(t, a.Target), registers.get(t, a.Other))
		case "filter":
			registers["result"] = Filter(registers.get(t, a.Target),
				phase1HelpersPredicate(a.Predicate, a.Value, &calls))
			row["predicate"], row["calls"] = a.Predicate, calls
			phase1HelpersResult(row, registers["result"])
		case "deduplicate":
			registers["result"] = Deduplicate(registers.get(t, a.Target))
			phase1HelpersResult(row, registers["result"])
		case "append_if_unique":
			registers["result"] = AppendIfUnique(registers.get(t, a.Target), a.Value)
			row["value"] = a.Value
			phase1HelpersResult(row, registers["result"])
		case "concatenate":
			registers["result"] = Concatenate(registers.get(t, a.Target), registers.get(t, a.Other))
			row["target"], row["other"] = a.Target, a.Other
			phase1HelpersResult(row, registers["result"])
		case "map":
			registers["result"] = Map(registers.get(t, a.Target),
				phase1HelpersMapper(a.Rule, &calls))
			row["rule"], row["calls"] = a.Rule, calls
			phase1HelpersResult(row, registers["result"])
		case "map_index":
			registers["result"] = MapIndex(registers.get(t, a.Target),
				phase1HelpersIndexMapper(a.Rule, &calls))
			row["rule"], row["calls"] = a.Rule, calls
			phase1HelpersResult(row, registers["result"])
		case "map_filtered":
			registers["result"] = MapFiltered(registers.get(t, a.Target),
				phase1HelpersFilterMapper(a.Rule, &calls))
			row["rule"], row["calls"] = a.Rule, calls
			phase1HelpersResult(row, registers["result"])
		case "flat_map":
			registers["result"] = FlatMap(registers.get(t, a.Target),
				phase1HelpersFlatMapper(a.Rule, &calls))
			row["rule"], row["calls"] = a.Rule, calls
			phase1HelpersResult(row, registers["result"])
		case "flatten":
			// The outer slice comes straight from the request, so a null
			// element is a nil subarray and the case can say which it meant.
			registers["result"] = Flatten(a.Groups)
			row["groups"] = len(a.Groups)
			phase1HelpersResult(row, registers["result"])
		case "flatten_register":
			registers["result"] = Flatten([][]string{registers.get(t, a.Target)})
			row["target"] = a.Target
			phase1HelpersResult(row, registers["result"])
		case "same_map":
			registers["result"] = SameMap(registers.get(t, a.Target),
				phase1HelpersMapper(a.Rule, &calls))
			row["rule"], row["calls"] = a.Rule, calls
			phase1HelpersResult(row, registers["result"])
		case "some":
			row["predicate"] = a.Predicate
			row["result"] = Some(registers.get(t, a.Target),
				phase1HelpersPredicate(a.Predicate, a.Value, &calls))
			row["calls"] = calls
		case "find":
			row["predicate"] = a.Predicate
			row["result"] = Find(registers.get(t, a.Target),
				phase1HelpersPredicate(a.Predicate, a.Value, &calls))
			row["calls"] = calls
		case "find_last":
			row["predicate"] = a.Predicate
			row["result"] = FindLast(registers.get(t, a.Target),
				phase1HelpersPredicate(a.Predicate, a.Value, &calls))
			row["calls"] = calls
		case "find_index":
			row["predicate"] = a.Predicate
			row["index"] = FindIndex(registers.get(t, a.Target),
				phase1HelpersPredicate(a.Predicate, a.Value, &calls))
			row["calls"] = calls
		case "first_or_nil":
			row["target"] = a.Target
			row["result"] = FirstOrNil(registers.get(t, a.Target))
		case "last_or_nil":
			row["target"] = a.Target
			row["result"] = LastOrNil(registers.get(t, a.Target))
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1HelpersSingleElement(t *testing.T, trace []phase1HelpersAction) []any {
	var box *phase1HelpersBox
	var result []*phase1HelpersBox
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new_box":
			box = &phase1HelpersBox{Name: a.Value}
			row["value"] = a.Value
		case "clear_box":
			box = nil
		case "single":
			result = SingleElementSlice(box)
			names := []any{}
			for _, element := range result {
				if element == nil {
					names = append(names, nil)
					continue
				}
				names = append(names, element.Name)
			}
			row["values"] = names
			row["nil"] = result == nil
			row["len"] = len(result)
		case "mutate_element":
			if a.Index < 0 || a.Index >= len(result) {
				t.Fatalf("mutate_element index %d is outside a result of length %d",
					a.Index, len(result))
			}
			result[a.Index].Name = a.Value
			row["index"], row["value"] = a.Index, a.Value
		case "read_box":
			// Reads the element the caller still holds, which is how the trace
			// witnesses that the returned slice carries the pointer itself.
			if box == nil {
				row["box"] = nil
			} else {
				row["box"] = box.Name
			}
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1HelpersMemoize(trace []phase1HelpersAction) []any {
	calls := 0
	var memoized func() string
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "memoize":
			calls = 0
			value := a.Value
			memoized = Memoize(func() string {
				calls++
				return value
			})
			row["value"] = a.Value
		case "memoize_panicking":
			calls = 0
			failed := false
			value := a.Value
			memoized = Memoize(func() string {
				calls++
				if !failed {
					failed = true
					panic(phase1HelpersCreateFailed)
				}
				return value
			})
			row["value"] = a.Value
		case "call":
			if memoized == nil {
				panic("phase1: call before memoize")
			}
			result, class, payload, message := phase1HelpersGuarded(func() any { return memoized() })
			row["result"] = result
			row["panic"], row["payload"], row["message"] = class, payload, message
			row["calls"] = calls
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1HelpersMust(trace []phase1HelpersAction) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "must":
			// The error is the case's own, so the panic it produces carries the
			// case's text rather than anything the toolchain wrote.
			var failure error
			if a.Flag {
				failure = errors.New(a.Message)
			}
			value := a.Value
			result, class, payload, message := phase1HelpersGuarded(func() any {
				return Must(value, failure)
			})
			row["value"], row["fails"], row["message_in"] = a.Value, a.Flag, a.Message
			row["result"] = result
			row["panic"], row["payload"], row["message"] = class, payload, message
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1HelpersValues(trace []phase1HelpersAction) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "or_else":
			row["value"], row["fallback"] = a.Value, a.Fallback
			row["result"] = OrElse(a.Value, a.Fallback)
		case "if_else":
			row["flag"], row["value"], row["fallback"] = a.Flag, a.Value, a.Fallback
			row["result"] = IfElse(a.Flag, a.Value, a.Fallback)
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1HelpersSeqOf(items []string) iter.Seq[string] {
	return func(yield func(string) bool) {
		for _, value := range items {
			if !yield(value) {
				return
			}
		}
	}
}

func phase1HelpersConcatenateSeq(trace []phase1HelpersAction) []any {
	operands := []iter.Seq[string]{}
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "reset_seqs":
			operands = []iter.Seq[string]{}
		case "push_seq":
			operands = append(operands, phase1HelpersSeqOf(a.Items))
			row["items"] = phase1HelpersList(a.Items)
		case "push_nil_seq":
			// A nil iter.Seq, which is what a caller holds when the optional
			// half of its input is absent: format/indent.go:160-166 leaves
			// trailingRangesOfPreviousToken nil when there is no preceding
			// token and hands it straight to ConcatenateSeq.
			operands = append(operands, nil)
		case "collect":
			visited := 0
			values := []any{}
			stopped := false
			// A `break` out of a range-over-func returns false from yield,
			// which is the early stop the pinned body propagates.
			for value := range ConcatenateSeq(operands...) {
				visited++
				values = append(values, value)
				if a.Index > 0 && visited >= a.Index {
					stopped = true
					break
				}
			}
			row["values"], row["visited"], row["stopped"] = values, visited, stopped
			row["limit"] = a.Index
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1HelpersComparable(t *testing.T, trace []phase1HelpersAction) []any {
	boxes := map[string]*phase1HelpersBox{}
	// A nil operand must be declared by an action, never fallen into by naming
	// a register no action set: that would make a typo look like a nil test.
	boxAt := func(name string) *phase1HelpersBox {
		box, ok := boxes[name]
		if !ok {
			t.Fatalf("action names box %q, which no earlier action created", name)
		}
		return box
	}
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new_box":
			boxes[a.Target] = &phase1HelpersBox{Name: a.Value}
			row["target"], row["value"] = a.Target, a.Value
		case "new_nil_box":
			boxes[a.Target] = nil
			row["target"] = a.Target
		case "strings":
			row["value"], row["other"] = a.Value, a.Fallback
			row["equal"] = comparableValuesEqual(a.Value, a.Fallback)
		case "pointers":
			// Both operands are *phase1HelpersBox, so == is address equality.
			row["target"], row["other"] = a.Target, a.Other
			row["equal"] = comparableValuesEqual(boxAt(a.Target), boxAt(a.Other))
		case "pairs":
			// A comparable struct carrying a pointer: == compares the name and
			// the ADDRESS, never the pointee.
			first := phase1HelpersPair{Name: a.Value, Ref: boxAt(a.Target)}
			second := phase1HelpersPair{Name: a.Value, Ref: boxAt(a.Other)}
			row["value"], row["target"], row["other"] = a.Value, a.Target, a.Other
			row["equal"] = comparableValuesEqual(first, second)
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1HelpersStringify(t *testing.T, trace []phase1HelpersAction) []any {
	ordered := []any{}
	for _, a := range trace {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "stringify":
			var input any
			switch a.Shape {
			case "string":
				input = string(phase1HelpersInput(t, a))
			case "strings":
				input = append([]string{}, a.Items...)
			case "strings_empty":
				input = []string{}
			case "strings_nil":
				// A TYPED nil slice, which is not the same input as a nil
				// interface: json/v2 writes [] for the first and null for the
				// second.
				var absent []string
				input = absent
			case "nil_any":
				input = nil
			case "nested":
				input = a.Groups
			default:
				t.Fatalf("stringify action carries unknown shape %q", a.Shape)
			}
			output, err := StringifyJson(input, a.Prefix, a.Indent)
			row["shape"], row["prefix"], row["indent"] = a.Shape, a.Prefix, a.Indent
			// Hex, because the output can carry bytes a JSON string field
			// would not survive, and because the exact bytes are the contract.
			row["result_hex"] = phase1HelpersHex([]byte(output))
			row["error"] = err != nil
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

// phase1HelpersReplay maps a request's declared subject to the trace player
// that serves it. A subject with no player is not this probe's: the schedule is
// shared with the other leaf groups, and every request still gets a row.
func phase1HelpersReplay(subject string) func(*testing.T, []phase1HelpersAction) []any {
	switch subject {
	case "helpers.Slices":
		return phase1HelpersSlices
	case "helpers.SingleElementSlice":
		return phase1HelpersSingleElement
	case "helpers.Memoize":
		return func(_ *testing.T, trace []phase1HelpersAction) []any {
			return phase1HelpersMemoize(trace)
		}
	case "helpers.Must":
		return func(_ *testing.T, trace []phase1HelpersAction) []any {
			return phase1HelpersMust(trace)
		}
	case "helpers.Values":
		return func(_ *testing.T, trace []phase1HelpersAction) []any {
			return phase1HelpersValues(trace)
		}
	case "helpers.ConcatenateSeq":
		return func(_ *testing.T, trace []phase1HelpersAction) []any {
			return phase1HelpersConcatenateSeq(trace)
		}
	case "helpers.ComparableValues":
		return phase1HelpersComparable
	case "helpers.StringifyJson":
		return phase1HelpersStringify
	}
	return nil
}

func TestPhase1LeavesHelpers(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []phase1HelpersRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if replay := phase1HelpersReplay(request.Subject); replay != nil {
			row["result"] = "observed"
			row["observation"] = map[string]any{
				"ordered": replay(t, phase1HelpersTrace(t, request)),
			}
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the helpers probe"
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
