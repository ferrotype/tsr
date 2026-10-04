// Access-only codec probe. It runs in the pinned lsproto package through go -overlay.
package lsproto

import (
	"encoding/hex"
	"encoding/json"
	"errors"
	wire "github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/jsonrpc"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"reflect"
	"strings"
	"testing"
)

type codecCase struct {
	Name          string   `json:"name"`
	Type          string   `json:"type"`
	Inputs        []string `json:"inputs"`
	ErrorContains string   `json:"error_contains"`
}
type codecResult struct {
	OK              bool   `json:"ok"`
	Encoded         string `json:"encoded"`
	MarshalPanicked bool   `json:"marshal_panicked"`
	MarshalFailed   bool   `json:"marshal_failed"`
}

func TestRustCodecMatrix(t *testing.T) {
	data, err := os.ReadFile(os.Getenv("TSR_CODEC_CASES"))
	if err != nil {
		t.Fatal(err)
	}
	var cases []codecCase
	if err = json.Unmarshal(data, &cases); err != nil {
		t.Fatal(err)
	}
	results := map[string][]codecResult{}
	for _, c := range cases {
		value := codecTarget(c.Type)
		for _, input := range c.Inputs {
			err := wire.Unmarshal([]byte(input), value)
			if c.ErrorContains != "" && (err == nil || !strings.Contains(err.Error(), c.ErrorContains)) {
				t.Fatalf("%s: expected %s; got %v", c.Name, c.ErrorContains, err)
			}
			result := codecResult{OK: err == nil}
			func() {
				defer func() {
					if recover() != nil {
						result.MarshalPanicked = true
					}
				}()
				data, err := wire.Marshal(value, wire.Deterministic(true))
				result.MarshalFailed = err != nil
				result.Encoded = string(data)
			}()
			results[c.Name] = append(results[c.Name], result)
		}
	}
	output, err := json.MarshalIndent(results, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("TSR_CODEC_RESULT"), append(output, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}

// Compare syntax trees, not a hand-maintained inventory: dprint groups adjacent
// var declarations and folds empty structs, while gofmt preserves those choices.
func TestRustResolverMatchesPinnedGo(t *testing.T) {
	parse := func(path string) *ast.File {
		f, err := parser.ParseFile(token.NewFileSet(), path, nil, parser.SkipObjectResolution)
		if err != nil {
			t.Fatal(err)
		}
		var decls []ast.Decl
		for _, d := range f.Decls {
			if g, ok := d.(*ast.GenDecl); ok && g.Tok == token.VAR {
				for _, spec := range g.Specs {
					decls = append(decls, &ast.GenDecl{Tok: token.VAR, Specs: []ast.Spec{spec}})
				}
			} else {
				decls = append(decls, d)
			}
		}
		f.Decls = decls
		erasePositions(reflect.ValueOf(f))
		return f
	}
	expected := parse(os.Getenv("TSR_PINNED_GO"))
	actual := parse(os.Getenv("TSR_GENERATED_GO"))
	if !reflect.DeepEqual(expected, actual) {
		t.Fatal("resolver output differs from the pinned Go declarations and codecs")
	}
}
func erasePositions(v reflect.Value) {
	if !v.IsValid() {
		return
	}
	if v.Type() == reflect.TypeFor[token.Pos]() {
		if v.CanSet() {
			v.SetInt(0)
		}
		return
	}
	switch v.Kind() {
	case reflect.Pointer, reflect.Interface:
		if !v.IsNil() {
			erasePositions(v.Elem())
		}
	case reflect.Struct:
		for i := 0; i < v.NumField(); i++ {
			erasePositions(v.Field(i))
		}
	case reflect.Slice:
		for i := 0; i < v.Len(); i++ {
			erasePositions(v.Index(i))
		}
	}
}

func codecTarget(name string) any {
	switch name {
	case "BooleanOrHoverOptions":
		return new(BooleanOrHoverOptions)
	case "CallHierarchyIncomingCall":
		return new(CallHierarchyIncomingCall)
	case "CallHierarchyIncomingCallsParams":
		return new(CallHierarchyIncomingCallsParams)
	case "ClientInfo":
		return new(ClientInfo)
	case "CompletionItem":
		return new(CompletionItem)
	case "DidChangeConfigurationParams":
		return new(DidChangeConfigurationParams)
	case "FoldingRange":
		return new(FoldingRange)
	case "Hover":
		return new(Hover)
	case "InitializationOptions":
		return new(InitializationOptions)
	case "InitializeParams":
		return new(InitializeParams)
	case "InitializeResult":
		return new(InitializeResult)
	case "InlayHint":
		return new(InlayHint)
	case "IntegerOrNull":
		return new(IntegerOrNull)
	case "IntegerOrString":
		return new(IntegerOrString)
	case "Location":
		return new(Location)
	case "Position":
		return new(Position)
	case "Registration":
		return new(Registration)
	case "SemanticTokens":
		return new(SemanticTokens)
	case "StringLiteralCreate":
		return new(StringLiteralCreate)
	case "StringOrTuple":
		return new(StringOrTuple)
	case "TextDocumentEdit":
		return new(TextDocumentEdit)
	case "TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile":
		return new(TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile)
	case "TextEditOrInsertReplaceEdit":
		return new(TextEditOrInsertReplaceEdit)
	case "WorkDoneProgressBeginOrReportOrEnd":
		return new(WorkDoneProgressBeginOrReportOrEnd)
	case "WorkDoneProgressOptions":
		return new(WorkDoneProgressOptions)
	case "WorkspaceEdit":
		return new(WorkspaceEdit)
	default:
		panic("unknown codec case type: " + name)
	}
}

// Exercise UnmarshalParams and the same error envelope that Server.sendError
// constructs. The native method supplies the entire message, including JSON
// decoder type, pointer and offset context.
func TestRustParamsMatrix(t *testing.T) {
	data, err := os.ReadFile(os.Getenv("TSR_PARAMS_CASES"))
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		Name   string
		Type   string
		Params *string
	}
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatal(err)
	}
	results := map[string]json.RawMessage{}
	for _, c := range cases {
		request := &RequestMessage{}
		if c.Params != nil {
			request.Params = wire.Value(*c.Params)
		}
		var err error
		switch c.Type {
		case "NoParams":
			_, err = UnmarshalParams[NoParams](request)
		case "InitializedParams":
			_, err = UnmarshalParams[InitializedParams](request)
		case "InitializeParams":
			_, err = UnmarshalParams[InitializeParams](request)
		case "HoverParams":
			_, err = UnmarshalParams[HoverParams](request)
		default:
			t.Fatalf("unknown params case type: %s", c.Type)
		}
		response := &ResponseMessage{ID: jsonrpc.NewIDInt(7)}
		if err != nil {
			code := ErrorCodeInternalError
			if errCode, ok := errors.AsType[ErrorCode](err); ok {
				code = errCode
			}
			response.Error = &jsonrpc.ResponseError{Code: int32(code), Message: err.Error()}
		} else {
			response.Result = Null{}
		}
		encoded, err := wire.Marshal(response.Message())
		if err != nil {
			t.Fatal(err)
		}
		results[c.Name] = encoded
	}
	output, err := json.MarshalIndent(results, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("TSR_PARAMS_RESULT"), append(output, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}

// The URL parser dependency has behavior beyond ordinary filesystem paths.
// Both runtimes consume these cases, including raw bytes and refusal paths.
func TestRustDocumentURI(t *testing.T) {
	data, err := os.ReadFile(os.Getenv("TSR_URI_CASES"))
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		URI     string  `json:"uri"`
		FileHex *string `json:"file_hex"`
	}
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatal(err)
	}
	for _, c := range cases {
		var result string
		panicked := true
		func() { defer func() { _ = recover() }(); result = DocumentUri(c.URI).FileName(); panicked = false }()
		if c.FileHex == nil {
			if !panicked {
				t.Errorf("%q: expected panic, got %q", c.URI, result)
			}
			continue
		}
		if panicked || hex.EncodeToString([]byte(result)) != *c.FileHex {
			t.Errorf("%q: got %x (panic=%v), expected %s", c.URI, result, panicked, *c.FileHex)
		}
	}
}
