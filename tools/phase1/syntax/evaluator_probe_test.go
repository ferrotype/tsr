package evaluator

// Phase 1 F4a, `evaluator` group (plan task 7): access only. The pinned
// package has no test file and no testdata, and its only constructor call is
// the checker's (checker.go:939). This probe drives NewEvaluator directly with
// a RECORDING callback: each entity expression the evaluator hands it is
// looked up by its source text in the request's table, answered with the
// controlled value and flags there, and logged in call order. So the
// observation shows values, string-ness, the two cross-file flags, unknown
// results and exactly which callbacks ran in which order -- including the ones
// a template short-circuit skips -- without any checker name resolution.
//
// Direct requests call AnyToString, IsTruthy and NewResult on typed values,
// including the unhandled types whose panic text is the pinned behaviour.
//
// This is the package's first _test.go; every name is prefixed `phase1`.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math"
	"os"
	"runtime"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/jsnum"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/scanner"
)

// phase1Value is a typed value on the wire: exactly one member is set.
type phase1Value struct {
	Number      *string `json:"number"`
	String      *string `json:"string"`
	StringHex   *string `json:"string_hex"`
	Bool        *bool   `json:"bool"`
	BigInt      *string `json:"bigint"`
	Nil         bool    `json:"nil"`
	Unsupported bool    `json:"unsupported"`
}

type phase1Unsupported struct{}

func (v phase1Value) decode(t *testing.T) any {
	t.Helper()
	switch {
	case v.Number != nil:
		return jsnum.FromString(*v.Number)
	case v.String != nil:
		return *v.String
	case v.StringHex != nil:
		bytes, err := hex.DecodeString(*v.StringHex)
		if err != nil {
			t.Fatalf("invalid evaluator string_hex: %v", err)
		}
		return string(bytes)
	case v.Bool != nil:
		return *v.Bool
	case v.BigInt != nil:
		text := *v.BigInt
		negative := strings.HasPrefix(text, "-")
		digits := strings.TrimLeft(strings.TrimPrefix(text, "-"), "0")
		return jsnum.PseudoBigInt{Negative: negative && digits != "", Base10Value: digits}
	case v.Unsupported:
		return phase1Unsupported{}
	}
	return nil
}

// phase1Encode renders a Go value with its dynamic type, so a number that
// became a string (or -0 that became 0) is visible. Go strings carry arbitrary
// bytes, including the scanner's lone-surrogate encodings. JSON strings would
// replace those bytes, so string values always travel as hexadecimal bytes.
func phase1Encode(value any) any {
	switch v := value.(type) {
	case nil:
		return nil
	case jsnum.Number:
		// A NaN's sign bit is not part of any contract, so it is not recorded.
		return []any{"number", v.String(), math.Signbit(float64(v)) && !v.IsNaN(), v.IsNaN()}
	case string:
		return []any{"string_hex", hex.EncodeToString([]byte(v))}
	case bool:
		return []any{"bool", v}
	case jsnum.PseudoBigInt:
		return []any{"bigint", v.Negative, v.Base10Value}
	}
	return []any{"unsupported", fmt.Sprintf("%T", value)}
}

type phase1Entity struct {
	Value                 phase1Value `json:"value"`
	IsSyntacticallyString bool        `json:"is_syntactically_string"`
	ResolvedOtherFiles    bool        `json:"resolved_other_files"`
	HasExternalReferences bool        `json:"has_external_references"`
}

type phase1EvalRequest struct {
	Case      string                  `json:"case"`
	Operation string                  `json:"operation"`
	Subject   string                  `json:"subject"`
	Mode      string                  `json:"mode"`
	Source    string                  `json:"source"`
	Skip      []string                `json:"skip"`
	Entities  map[string]phase1Entity `json:"entities"`
	Values    []phase1Value           `json:"values"`
	Flags     [][3]bool               `json:"flags"`
}

var phase1OuterKinds = map[string]ast.OuterExpressionKinds{
	"parentheses":           ast.OEKParentheses,
	"type_assertions":       ast.OEKTypeAssertions,
	"non_null_assertions":   ast.OEKNonNullAssertions,
	"satisfies":             ast.OEKSatisfies,
	"expressions_with_type": ast.OEKExpressionsWithTypeArguments,
	"all":                   ast.OEKAll,
}

func phase1Guarded(run func() any) (result any) {
	defer func() {
		if value := recover(); value != nil {
			result = []any{"panic", fmt.Sprint(value)}
		}
	}()
	return run()
}

func phase1Result(result Result) []any {
	return []any{phase1Encode(result.Value), result.IsSyntacticallyString, result.ResolvedOtherFiles, result.HasExternalReferences}
}

func phase1Evaluate(t *testing.T, request phase1EvalRequest) []any {
	file := parser.ParseSourceFile(ast.SourceFileParseOptions{FileName: "/evaluate.ts", Path: "/evaluate.ts"}, request.Source, core.ScriptKindTS)
	if len(file.Diagnostics()) != 0 {
		t.Fatalf("%s: the request source does not parse cleanly", request.Case)
	}
	text := func(node *ast.Node) string {
		return strings.TrimSpace(file.Text()[scanner.SkipTrivia(file.Text(), node.Pos()):node.End()])
	}
	var skip ast.OuterExpressionKinds
	for _, name := range request.Skip {
		kind, ok := phase1OuterKinds[name]
		if !ok {
			t.Fatalf("%s: unknown outer expression kind %s", request.Case, name)
		}
		skip |= kind
	}
	var calls []any
	callback := func(expr *ast.Node, location *ast.Node) Result {
		name := text(expr)
		calls = append(calls, []any{int(expr.Kind), expr.Pos(), expr.End(), name, int(location.Kind), location.Pos()})
		entity, ok := request.Entities[name]
		if !ok {
			return Result{}
		}
		return Result{entity.Value.decode(t), entity.IsSyntacticallyString, entity.ResolvedOtherFiles, entity.HasExternalReferences}
	}
	evaluate := NewEvaluator(callback, skip)
	rows := []any{}
	for _, statement := range file.Statements.Nodes {
		if statement.Kind != ast.KindExpressionStatement {
			t.Fatalf("%s: every statement must be an expression statement", request.Case)
		}
		expr := statement.AsExpressionStatement().Expression
		calls = []any{}
		result := phase1Guarded(func() any { return phase1Result(evaluate(expr, expr)) })
		rows = append(rows, []any{text(expr), result, calls})
	}
	return rows
}

func phase1Direct(t *testing.T, request phase1EvalRequest) []any {
	rows := []any{}
	switch request.Mode {
	case "any_to_string":
		for _, value := range request.Values {
			decoded := value.decode(t)
			rows = append(rows, []any{phase1Encode(decoded), phase1Guarded(func() any { return phase1Encode(AnyToString(decoded)) })})
		}
	case "is_truthy":
		for _, value := range request.Values {
			decoded := value.decode(t)
			rows = append(rows, []any{phase1Encode(decoded), phase1Guarded(func() any { return IsTruthy(decoded) })})
		}
	case "new_result":
		for i, flags := range request.Flags {
			var value any = nil
			if i < len(request.Values) {
				value = request.Values[i].decode(t)
			}
			rows = append(rows, phase1Result(NewResult(value, flags[0], flags[1], flags[2])))
		}
	default:
		t.Fatalf("%s: unknown mode %s", request.Case, request.Mode)
	}
	return rows
}

func TestPhase1SyntaxEvaluator(t *testing.T) {
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
		var request phase1EvalRequest
		if err := json.Unmarshal(raw, &request); err != nil {
			t.Fatal(err)
		}
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		switch {
		case request.Subject != "evaluator":
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the evaluator probe"
		case request.Mode == "evaluate":
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": phase1Evaluate(t, request)}
		default:
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": phase1Direct(t, request)}
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
