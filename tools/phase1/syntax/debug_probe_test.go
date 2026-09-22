package debug

// Phase 1 F4a, `debug` group: access only. The five operations of the pinned
// debug package are panic-text contracts: the scanner, parser, binder, core and
// tsoptions call Assert and FailBadSyntaxKind, and the text of the panic is
// what a caller sees. Each request calls one entry point with typed arguments
// and records the recovered panic text, or that it returned.
//
// In-package, beside upstream's own debug_test.go; every name is prefixed
// `phase1` so it cannot collide with that file's mocks.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"testing"
)

type phase1KindString struct{ kind string }

func (n phase1KindString) KindString() string { return n.kind }

type phase1Stringer struct{ text string }

func (s phase1Stringer) String() string { return s.text }

type phase1Both struct{ kind, text string }

func (b phase1Both) KindString() string { return b.kind }
func (b phase1Both) String() string     { return b.text }

// phase1Arg is one typed argument: exactly one member is set.
type phase1Arg struct {
	String     *string `json:"string"`
	Int        *int    `json:"int"`
	KindString *string `json:"kind_string"`
	Stringer   *string `json:"stringer"`
	Both       *[2]string `json:"both"`
	Nil        bool    `json:"nil"`
}

func (a phase1Arg) value() any {
	switch {
	case a.String != nil:
		return *a.String
	case a.Int != nil:
		return *a.Int
	case a.KindString != nil:
		return phase1KindString{*a.KindString}
	case a.Stringer != nil:
		return phase1Stringer{*a.Stringer}
	case a.Both != nil:
		return phase1Both{a.Both[0], a.Both[1]}
	}
	return nil
}

type phase1DebugRequest struct {
	Case      string      `json:"case"`
	Operation string      `json:"operation"`
	Subject   string      `json:"subject"`
	Call      string      `json:"call"`
	Reason    string      `json:"reason"`
	Value     bool        `json:"value"`
	Member    phase1Arg   `json:"member"`
	Message   []phase1Arg `json:"message"`
}

func phase1Outcome(call func()) (result []any) {
	defer func() {
		if value := recover(); value != nil {
			result = []any{"panic", fmt.Sprint(value)}
		}
	}()
	call()
	return []any{"returned"}
}

func phase1Call(t *testing.T, request phase1DebugRequest) []any {
	message := make([]any, 0, len(request.Message))
	for _, arg := range request.Message {
		message = append(message, arg.value())
	}
	switch request.Call {
	case "fail":
		return phase1Outcome(func() { Fail(request.Reason) })
	case "fail_bad_syntax_kind":
		node, ok := request.Member.value().(interface{ KindString() string })
		if !ok {
			t.Fatalf("%s: FailBadSyntaxKind needs a KindString member", request.Case)
		}
		return phase1Outcome(func() { FailBadSyntaxKind(node, message...) })
	case "assert_never":
		return phase1Outcome(func() { AssertNever(request.Member.value(), message...) })
	case "assert":
		return phase1Outcome(func() { Assert(request.Value, message...) })
	}
	t.Fatalf("%s: unknown call %s", request.Case, request.Call)
	return nil
}

func TestPhase1SyntaxDebug(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []json.RawMessage `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}
	observations := make([]map[string]any, 0, len(document.Requests))
	for _, raw := range document.Requests {
		var request phase1DebugRequest
		if err := json.Unmarshal(raw, &request); err != nil {
			t.Fatal(err)
		}
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject != "debug" {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the debug probe"
		} else {
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": []any{phase1Call(t, request)}}
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
