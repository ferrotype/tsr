package scanner

// Access-only fixture for scanner APIs whose arguments are source files or
// AST nodes. Text and ranges come from the request; no expected result is read.
import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
)

type phase1ScannerNode struct {
	Kind         string   `json:"kind"`
	Pos          int      `json:"pos"`
	End          int      `json:"end"`
	Text         []string `json:"text_hex"`
	InJSDoc      bool     `json:"in_jsdoc"`
	Doc          *[2]int  `json:"doc"`
	IncludeJSDoc bool     `json:"include_jsdoc"`
}
type phase1ScannerRequest struct {
	Case        string                `json:"case"`
	Operation   string                `json:"operation"`
	Subject     string                `json:"subject"`
	Call        string                `json:"call"`
	Source      string                `json:"source_hex"`
	Nodes       *[]*phase1ScannerNode `json:"nodes"`
	Positions   []int                 `json:"positions"`
	Coordinates [][2]int              `json:"coordinates"`
	Names       []string              `json:"names_hex"`
}

func phase1ScannerBytes(t *testing.T, value string) string {
	t.Helper()
	bytes, err := hex.DecodeString(value)
	if err != nil {
		t.Fatal(err)
	}
	return string(bytes)
}

// Only the named nil-element request admits a panic observation. Other panic
// types/messages are rethrown, and a normal return fails the native probe.
func phase1ScannerNilElementComment(t *testing.T, call func() string) (out any) {
	t.Helper()
	defer func() {
		value := recover()
		if value == nil {
			t.Fatal("nil-element comment did not produce the native nil-pointer panic")
		}
		err, ok := value.(runtime.Error)
		if !ok || err.Error() != "runtime error: invalid memory address or nil pointer dereference" {
			panic(value)
		}
		out = []any{"panic", err.Error()}
	}()
	call()
	return nil
}
func phase1ScannerCall(t *testing.T, r phase1ScannerRequest) any {
	t.Helper()
	operation := map[string]string{
		"default_state":       "tsc/internal/scanner/scanner.go:defaultScanner",
		"comment":             "tsc/internal/scanner/utilities.go:GetTextOfJSDocComment",
		"comment_nil_element": "tsc/internal/scanner/utilities.go:GetTextOfJSDocComment",
		"lines":               "tsc/internal/scanner/scanner.go:GetECMALineStarts",
		"token_position":      "tsc/internal/scanner/scanner.go:GetTokenPosOfNode",
		"scan_token":          "tsc/internal/scanner/scanner.go:ScanTokenAtPosition",
		"keyword":             "tsc/internal/scanner/utilities.go:IdentifierToKeywordKind",
	}[r.Call]
	if operation == "" || operation != r.Operation {
		t.Fatalf("%s: wrong scanner operation for call %q", r.Case, r.Call)
	}
	if r.Call == "comment_nil_element" && r.Case != "syntax/scanner-ast/comment-nil-element" {
		t.Fatal("nil-element panic observation requires its named request")
	}
	f := ast.NewNodeFactory(ast.NodeFactoryHooks{})
	text := phase1ScannerBytes(t, r.Source)
	file := f.NewSourceFile(ast.SourceFileParseOptions{FileName: "/scanner.ts"}, text, nil, nil).AsSourceFile()
	out := []any{}
	switch r.Call {
	case "default_state":
		s := NewScanner()
		snapshot := func() []any {
			return []any{int(s.Token()), s.TokenFullStart(), s.TokenStart(), s.TokenEnd(), int(s.TokenFlags()), hex.EncodeToString([]byte(s.TokenText()))}
		}
		out = append(out, snapshot())
		s.SetText(text)
		s.Scan()
		out = append(out, snapshot())
		s.SetSkipTrivia(false)
		s.Reset()
		out = append(out, snapshot())
		s.SetText(text)
		s.Scan()
		out = append(out, snapshot())
	case "lines":
		first := GetECMALineStarts(file)
		second := GetECMALineStarts(file)
		out = append(out, first, len(first) == len(second) && &first[0] == &second[0])
		for _, pos := range r.Positions {
			line, offset := GetECMALineAndByteOffsetOfPosition(file, pos)
			out = append(out, []any{GetECMALineOfPosition(file, pos), line, offset})
		}
		for _, at := range r.Coordinates {
			out = append(out, []any{GetECMAPositionOfLineAndByteOffset(file, at[0], at[1]), GetECMAPositionOfLineAndUTF16Character(file, at[0], core.UTF16Offset(at[1]))})
		}
	case "scan_token":
		for _, pos := range r.Positions {
			out = append(out, int(ScanTokenAtPosition(file, pos)))
		}
	case "keyword":
		for _, name := range r.Names {
			out = append(out, int(IdentifierToKeywordKind(f.NewIdentifier(phase1ScannerBytes(t, name)).AsIdentifier())))
		}
	case "comment", "comment_nil_element", "token_position":
		var list *ast.NodeList
		if r.Nodes != nil {
			nodes := []*ast.Node{}
			for _, spec := range *r.Nodes {
				if spec == nil && r.Call != "token_position" {
					nodes = append(nodes, nil)
					continue
				}
				parts := []string{}
				for _, part := range spec.Text {
					parts = append(parts, phase1ScannerBytes(t, part))
				}
				var node *ast.Node
				switch spec.Kind {
				case "JSDocText":
					node = f.NewJSDocText(parts)
				case "JSDocLink":
					node = f.NewJSDocLink(nil, parts)
				case "JSDocLinkCode":
					node = f.NewJSDocLinkCode(nil, parts)
				case "JSDocLinkPlain":
					node = f.NewJSDocLinkPlain(nil, parts)
				case "Identifier":
					node = f.NewIdentifier("constructed")
				case "JsxText":
					node = f.NewJsxText("constructed", false)
				case "JSDocComment":
					node = f.NewJSDoc(nil, nil)
				case "SemicolonToken":
					node = f.NewToken(ast.KindSemicolonToken)
				default:
					t.Fatalf("%s: unknown scanner node kind %q", r.Case, spec.Kind)
				}
				node.Loc = core.NewTextRange(spec.Pos, spec.End)
				node.Parent = file.AsNode()
				if spec.InJSDoc {
					node.Flags |= ast.NodeFlagsJSDoc
				}
				if spec.Doc != nil {
					doc := f.NewJSDoc(nil, nil)
					doc.Loc = core.NewTextRange(spec.Doc[0], spec.Doc[1])
					doc.Parent = node
					node.Flags |= ast.NodeFlagsHasJSDoc
					file.SetJSDocCache(map[*ast.Node][]*ast.Node{node: {doc}})
				}
				if r.Call == "token_position" {
					out = append(out, GetTokenPosOfNode(node, file, spec.IncludeJSDoc))
				}
				nodes = append(nodes, node)
			}
			list = &ast.NodeList{Nodes: nodes}
		}
		if r.Call == "comment" {
			out = append(out, hex.EncodeToString([]byte(GetTextOfJSDocComment(list))))
		} else if r.Call == "comment_nil_element" {
			out = append(out, phase1ScannerNilElementComment(t, func() string {
				return GetTextOfJSDocComment(list)
			}))
		}
	}
	return map[string]any{"ordered": out}
}
func TestPhase1SyntaxScannerAst(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var doc struct {
		Requests []json.RawMessage `json:"requests"`
	}
	if err = json.Unmarshal(input, &doc); err != nil {
		t.Fatal(err)
	}
	rows := []map[string]any{}
	for _, raw := range doc.Requests {
		var header struct {
			Case      string `json:"case"`
			Operation string `json:"operation"`
			Subject   string `json:"subject"`
		}
		if err := json.Unmarshal(raw, &header); err != nil {
			t.Fatal(err)
		}
		row := map[string]any{"case": header.Case, "operation": header.Operation}
		if header.Subject != "scannerAst" {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by scanner AST probe"
		} else {
			var r phase1ScannerRequest
			if err := json.Unmarshal(raw, &r); err != nil {
				t.Fatal(err)
			}
			row["result"] = "observed"
			row["observation"] = phase1ScannerCall(t, r)
		}
		rows = append(rows, row)
	}
	hash := sha256.Sum256(input)
	output := map[string]any{"version": 1, "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "request_sha256": hex.EncodeToString(hash[:]), "observations": rows}
	bytes, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("S08_OUTPUT"), append(bytes, '\n'), 0644); err != nil {
		t.Fatal(err)
	}
}
