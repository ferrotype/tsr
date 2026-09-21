package glob

// Access only: replays an ordered action trace against the pinned LSP glob
// grammar and records what each action observed.
//
// It is an IN-PACKAGE test file, unlike the leaves probes, because twelve of
// the fifteen operations this group covers are unexported and unreachable from
// outside: parse, parseLiteral, readRangeRune, split, match and the String
// methods of the seven element types, which are methods on unexported types
// (glob.go:184-195 declares slash, literal, star, anyChar, starStar, group and
// charRange). Only Parse, Glob.String and Glob.Match are reachable. The
// package declares no test file of its own at this pin (internal/glob holds
// glob.go and nothing else), so every name declared here carries a phase1
// prefix to keep a later pinned test from colliding with it.
//
// The trace shape is the contract the Rust driver answers: one ordered result
// per action, under `ordered`, so member order survives canonicalisation.
//
// The dialect tag is load-bearing. internal/glob is the LSP/test grammar and
// internal/vfs/vfsmatch is the configuration matcher; they share no code and
// agree on almost nothing (`*` here spans a whole segment and refuses to meet a
// separator, `[a-z]` and `{a,b}` do not exist in vfsmatch at all). Every
// request carries its dialect and this probe refuses a claimed subject that is
// not tagged `lsp`, so a request can never be answered by the wrong adapter.
//
// Three rules keep an observation portable.
//
// Byte payloads travel as hex in both directions. A pattern or an input may
// hold invalid UTF-8 -- `[\xff-\xff]` and the literal "\xff" are both cases
// here -- and encoding/json would replace those bytes with U+FFFD, which is
// exactly the repair one of these cases exists to catch.
//
// A panic the Go runtime raises is reduced to a class: its wording names
// dynamic types and byte offsets and can move with the toolchain, and no Rust
// port could reproduce the sentence. Match reaches two of them, both real
// pinned behavior rather than probe error: glob.go:228 loops `for input[0] ==
// '/'` with no length guard, so any input whose separator run reaches the end
// panics with an index-out-of-range, and String and Match on a nil *Glob
// dereference g.elems. An error the pinned source *returns* is recorded with
// its literal text instead, because that text is the contract: all four are
// errors.New over a constant (glob.go:63, :85, :153-154) with nothing dynamic
// interpolated.
//
// The one panic the pinned source raises itself, glob.go:327's "segment type %T
// not implemented", is also reduced, against that rule, for a reason the rule
// anticipates: %T interpolates the dynamic Go type of the element, and the only
// way to reach that arm is to hand match an element type the switch does not
// list, which this file has to declare itself. The recorded class is therefore
// the arm, not the sentence, and the sentence would name a probe-local type.
//
// An element list is recorded as a structural kind array, never as %T output or
// as a Go-side rendering of the type name: the vocabulary below (slash, star,
// star_star, any_char, literal, char_range, group) is this file's own, chosen
// so a port with a closed element enum can answer it.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"strings"
	"testing"
)

// The grammar this probe owns. A request tagged anything else names the
// configuration matcher and must be answered by the vfsmatch adapter.
const phase1Dialect = "lsp"

// The subjects this probe serves. Every other subject in the shared filesystem
// schedule belongs to another adapter and is reported unavailable, not guessed
// at.
var phase1Subjects = map[string]bool{
	"glob.Glob":          true,
	"glob.element":       true,
	"glob.match":         true,
	"glob.parse":         true,
	"glob.parseLiteral":  true,
	"glob.readRangeRune": true,
	"glob.split":         true,
}

type phase1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	Dialect   string `json:"dialect"`
	// Raw, because every probe decodes the whole shared family schedule and
	// another group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

// phase1Action is the whole action vocabulary of this group. Every payload
// field is a pointer: an absent key must fail the probe rather than default,
// because a defaulted empty pattern is a perfectly plausible observation and
// would be indistinguishable from a request that meant to carry one.
type phase1Action struct {
	Op         string            `json:"op"`
	PatternHex *string           `json:"pattern_hex"`
	InputHex   *string           `json:"input_hex"`
	Nested     *bool             `json:"nested"`
	Elems      *[]phase1ElemSpec `json:"elems"`
}

// phase1ElemSpec builds one pinned element value directly, so a trace can drive
// match and the element String methods without going through Parse.
type phase1ElemSpec struct {
	Kind       string              `json:"kind"`
	LiteralHex *string             `json:"literal_hex"`
	Negate     *bool               `json:"negate"`
	Low        *int32              `json:"low"`
	High       *int32              `json:"high"`
	Members    *[][]phase1ElemSpec `json:"members"`
}

// phase1Foreign is an element type the match switch does not list, which is the
// only way to reach its default arm: every pinned element type is covered by a
// case, so the arm is otherwise dead. It is not a pinned type and is never
// produced by Parse.
type phase1Foreign struct{}

func (phase1Foreign) String() string { return "phase1-foreign" }

func phase1DecodeActions(raw json.RawMessage) []phase1Action {
	if len(raw) == 0 {
		panic("phase1: missing actions")
	}
	var out []phase1Action
	if err := json.Unmarshal(raw, &out); err != nil {
		// Decoding happens before any guarded call into the pinned code. A
		// malformed request must fail the probe, never turn into an empty
		// observation both sides could agree on.
		panic("phase1: invalid action payload: " + err.Error())
	}
	if len(out) == 0 {
		panic("phase1: empty action trace")
	}
	return out
}

func phase1Hex(value string) string { return hex.EncodeToString([]byte(value)) }

// phase1Bytes reads a required hex field. It refuses a missing key and refuses
// malformed hex; both are harness failures, not observations.
func phase1Bytes(field *string, name string) string {
	if field == nil {
		panic("phase1: action is missing " + name)
	}
	raw, err := hex.DecodeString(*field)
	if err != nil {
		panic("phase1: " + name + " is not hex: " + err.Error())
	}
	return string(raw)
}

func phase1Flag(field *bool, name string) bool {
	if field == nil {
		panic("phase1: action is missing " + name)
	}
	return *field
}

// phase1Guard runs f and converts a panic into a recorded class, so a case
// whose subject is the panic boundary observes it instead of failing.
func phase1Guard(f func() any) (result any, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			panicked = phase1Class(r)
		}
	}()
	return f(), ""
}

func phase1Class(r any) string {
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
		// glob.go:228: the slash arm consumes a separator run with no length
		// guard, so an input whose run reaches the end indexes past it.
		return "index_out_of_range"
	case strings.Contains(text, "nil pointer dereference"),
		strings.Contains(text, "invalid memory address"):
		// String and Match both read g.elems off the receiver with no nil test.
		return "nil_pointer_dereference"
	case strings.HasPrefix(text, "segment type "):
		// glob.go:327. The sentence interpolates the dynamic Go type of the
		// element with %T, so only the arm is recorded; see the file header.
		return "unimplemented_segment_type"
	default:
		return "other:" + text
	}
}

// phase1Error records a returned error by its literal text. All four the parser
// can return are errors.New over a constant sentence with nothing interpolated,
// so the text is the pinned contract rather than toolchain wording.
func phase1Error(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// phase1Kinds renders an element list structurally. The vocabulary is this
// file's own rather than %T output, so a port whose elements are a closed enum
// can answer it. A literal carries its bytes as hex; a character range carries
// the negate flag the parser stores and everything downstream ignores, which is
// otherwise unobservable, since String drops it and match never reads it.
func phase1Kinds(elems []element) []any {
	out := []any{}
	for _, e := range elems {
		switch e := e.(type) {
		case slash:
			out = append(out, "slash")
		case star:
			out = append(out, "star")
		case starStar:
			out = append(out, "star_star")
		case anyChar:
			out = append(out, "any_char")
		case literal:
			out = append(out, []any{"literal", phase1Hex(string(e))})
		case charRange:
			out = append(out, []any{"char_range", e.negate, int64(e.low), int64(e.high)})
		case group:
			members := []any{}
			for _, member := range e {
				members = append(members, phase1Kinds(member.elems))
			}
			out = append(out, []any{"group", members})
		case phase1Foreign:
			out = append(out, "foreign")
		default:
			panic(fmt.Sprintf("phase1: unrecorded element kind %T", e))
		}
	}
	return out
}

// phase1Build turns element specs into real pinned element values.
func phase1Build(specs []phase1ElemSpec) []element {
	out := make([]element, 0, len(specs))
	for _, spec := range specs {
		switch spec.Kind {
		case "slash":
			out = append(out, slash{})
		case "star":
			out = append(out, star{})
		case "star_star":
			out = append(out, starStar{})
		case "any_char":
			out = append(out, anyChar{})
		case "literal":
			out = append(out, literal(phase1Bytes(spec.LiteralHex, "literal_hex")))
		case "char_range":
			if spec.Low == nil || spec.High == nil {
				panic("phase1: char_range needs low and high")
			}
			out = append(out, charRange{phase1Flag(spec.Negate, "negate"), rune(*spec.Low), rune(*spec.High)})
		case "group_nil":
			// A group whose slice is nil, distinct in memory from the empty
			// group below and, the trace shows, in nothing else.
			out = append(out, group(nil))
		case "group":
			if spec.Members == nil {
				panic("phase1: group needs members")
			}
			built := group{}
			for _, member := range *spec.Members {
				built = append(built, &Glob{elems: phase1Build(member)})
			}
			out = append(out, built)
		case "foreign":
			out = append(out, phase1Foreign{})
		default:
			panic("phase1: unsupported element kind: " + spec.Kind)
		}
	}
	return out
}

// phase1State is one case's replay state. `set` guards keep an action that
// needs a glob or an element list from silently answering over a zero value.
type phase1State struct {
	glob     *Glob
	globSet  bool
	elems    []element
	elemsSet bool
}

func (s *phase1State) currentGlob() *Glob {
	if !s.globSet {
		panic("phase1: action needs a glob, and no parse or build_elems action produced one")
	}
	return s.glob
}

func (s *phase1State) currentElems() []element {
	if !s.elemsSet {
		panic("phase1: action needs an element list, and no build_elems action produced one")
	}
	return s.elems
}

func phase1Replay(request phase1Request) []any {
	state := &phase1State{}
	ordered := []any{}
	for _, action := range phase1DecodeActions(request.Actions) {
		row := map[string]any{"op": action.Op}
		switch action.Op {
		case "parse":
			// The exported entry point, which is parse(pattern, false).
			pattern := phase1Bytes(action.PatternHex, "pattern_hex")
			parsed, err := Parse(pattern)
			state.glob, state.globSet = parsed, err == nil
			if err == nil {
				state.elems, state.elemsSet = parsed.elems, true
			}
			row["accepted"] = err == nil
			row["error"] = phase1Error(err)
			if err == nil {
				row["elems"] = phase1Kinds(parsed.elems)
			} else {
				row["elems"] = []any{}
			}

		case "parse_nested":
			// The internal parser, with the grouping flag the exported entry
			// point hardcodes to false, and the residual it discards.
			pattern := phase1Bytes(action.PatternHex, "pattern_hex")
			parsed, residual, err := parse(pattern, phase1Flag(action.Nested, "nested"))
			row["accepted"] = err == nil
			row["error"] = phase1Error(err)
			row["residual_hex"] = phase1Hex(residual)
			if err == nil {
				row["elems"] = phase1Kinds(parsed.elems)
			} else {
				row["elems"] = []any{}
			}

		case "parse_literal":
			// On a fresh Glob, so the row reports exactly what this call
			// appended rather than what a trace accumulated.
			fresh := new(Glob)
			residual := fresh.parseLiteral(
				phase1Bytes(action.PatternHex, "pattern_hex"),
				phase1Flag(action.Nested, "nested"),
			)
			row["appended"] = len(fresh.elems)
			row["elems"] = phase1Kinds(fresh.elems)
			row["residual_hex"] = phase1Hex(residual)

		case "read_range_rune":
			point, size, err := readRangeRune(phase1Bytes(action.InputHex, "input_hex"))
			row["rune"] = int64(point)
			row["size"] = size
			row["error"] = phase1Error(err)

		case "split":
			first, rest := split(phase1Bytes(action.InputHex, "input_hex"))
			row["first_hex"] = phase1Hex(first)
			row["rest_hex"] = phase1Hex(rest)

		case "build_elems":
			if action.Elems == nil {
				panic("phase1: build_elems is missing elems")
			}
			state.elems, state.elemsSet = phase1Build(*action.Elems), true
			state.glob, state.globSet = &Glob{elems: state.elems}, true
			row["count"] = len(state.elems)
			row["elems"] = phase1Kinds(state.elems)

		case "use_nil":
			// A nil *Glob is what a caller holds after ignoring Parse's error.
			state.glob, state.globSet = nil, true
			state.elems, state.elemsSet = nil, false

		case "string":
			// Glob.String over the current glob, guarded for the nil receiver.
			value, panicked := phase1Guard(func() any { return phase1Hex(state.currentGlob().String()) })
			row["string_hex"], row["panic"] = value, panicked

		case "string_elems":
			// Each element's own String, called directly, so the row reaches
			// the element methods and not Glob.String.
			rendered := []any{}
			for _, e := range state.currentElems() {
				rendered = append(rendered, phase1Hex(e.String()))
			}
			row["rendered"] = rendered

		case "match":
			// Glob.Match, guarded: a trailing separator run and a nil receiver
			// both panic in the pinned code.
			input := phase1Bytes(action.InputHex, "input_hex")
			value, panicked := phase1Guard(func() any { return state.currentGlob().Match(input) })
			row["result"], row["panic"] = value, panicked

		case "match_elems":
			// The free recursion, driven over a built element list without
			// going through Parse or Glob.Match.
			input := phase1Bytes(action.InputHex, "input_hex")
			elems := state.currentElems()
			value, panicked := phase1Guard(func() any { return match(elems, input) })
			row["result"], row["panic"] = value, panicked

		default:
			// Never an observation: a row both sides could agree on without
			// executing anything would be manufactured parity.
			panic("phase1: unsupported action: " + action.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func TestPhase1FilesystemGlob(t *testing.T) {
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
		switch {
		case !phase1Subjects[request.Subject]:
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the LSP glob probe"
		case request.Dialect != phase1Dialect:
			// A glob subject tagged with the other grammar is a routing error,
			// not a result: answering it here would let the LSP parser stand in
			// for the configuration matcher.
			t.Fatalf("case %s claims subject %s with dialect %q; this probe owns %q only",
				request.Case, request.Subject, request.Dialect, phase1Dialect)
		default:
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": phase1Replay(request)}
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
