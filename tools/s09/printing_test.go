package printer_test

// Access-only overlay: captures the real protocol bytes and the upstream
// decode/print path. It does not model Rust ownership or implement a formatter.
import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/api/encoder"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/printer"
)

type s09PrintOptions struct {
	NeverAsciiEscape              bool `json:"never_ascii_escape"`
	PreserveSourceNewlines        bool `json:"preserve_source_newlines"`
	TerminateUnterminatedLiterals bool `json:"terminate_unterminated_literals"`
}

type s09PrintCase struct {
	Name            string          `json:"name"`
	Tree            string          `json:"tree,omitempty"`
	Text            string          `json:"text,omitempty"`
	WireHex         *string         `json:"wire_hex,omitempty"`
	Mutation        string          `json:"mutation,omitempty"`
	Options         s09PrintOptions `json:"options"`
	RustExpected    string          `json:"rust_expected"`
	RustUnsupported string          `json:"rust_unsupported,omitempty"`
	ExpectedPanic   string          `json:"expected_panic,omitempty"`
	PanicClass      string          `json:"panic_class,omitempty"`
}

func s09Tree(f *ast.NodeFactory, request s09PrintCase) (*ast.Node, error) {
	switch request.Tree {
	case "identifier":
		return f.NewIdentifier(request.Text), nil
	case "string":
		return f.NewStringLiteral(request.Text, 0), nil
	case "unterminated-string":
		return f.NewStringLiteral(request.Text, ast.TokenFlagsUnterminated), nil
	case "union":
		return f.NewUnionTypeNode(f.NewNodeList([]*ast.Node{
			f.NewKeywordTypeNode(ast.KindStringKeyword),
			f.NewLiteralTypeNode(f.NewStringLiteral("choice", 0)),
		})), nil
	case "type-literal":
		return f.NewTypeLiteralNode(f.NewNodeList([]*ast.Node{
			f.NewPropertySignatureDeclaration(nil, f.NewIdentifier("label"), nil, f.NewKeywordTypeNode(ast.KindStringKeyword), nil),
			f.NewPropertySignatureDeclaration(nil, f.NewIdentifier("count"), nil, f.NewKeywordTypeNode(ast.KindNumberKeyword), nil),
		})), nil
	case "expression-statement":
		return f.NewExpressionStatement(f.NewIdentifier(request.Text)), nil
	default:
		return nil, fmt.Errorf("unrecognized fixture tree %q", request.Tree)
	}
}

func s09Print(request s09PrintCase, wire []byte) (row map[string]any) {
	row = map[string]any{"name": request.Name, "encoded_hex": hex.EncodeToString(wire), "options": request.Options}
	if request.RustUnsupported != "" {
		row["rust_unsupported"] = request.RustUnsupported
	}
	if request.PanicClass != "" {
		row["panic_class"] = request.PanicClass
	}
	defer func() {
		if value := recover(); value != nil {
			row["panic"] = fmt.Sprint(value)
		}
	}()
	node, err := encoder.DecodeNodes(wire)
	if err != nil {
		row["decode_error"] = err.Error()
		return row
	}
	p := printer.NewPrinter(printer.PrinterOptions{
		NeverAsciiEscape:              request.Options.NeverAsciiEscape,
		PreserveSourceNewlines:        request.Options.PreserveSourceNewlines,
		TerminateUnterminatedLiterals: request.Options.TerminateUnterminatedLiterals,
	}, printer.PrintHandlers{}, nil)
	row["text_hex"] = hex.EncodeToString([]byte(p.Emit(node, nil)))
	return row
}

func TestS09DecodeAndPrint(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var request struct {
		Cases []s09PrintCase `json:"cases"`
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&request); err != nil {
		t.Fatal(err)
	}
	if err := decoder.Decode(new(any)); err != io.EOF {
		t.Fatal("trailing request data")
	}
	rows := make([]map[string]any, 0, len(request.Cases))
	for _, item := range request.Cases {
		var wire []byte
		if item.WireHex != nil {
			wire, err = hex.DecodeString(*item.WireHex)
		} else {
			factory := ast.NewNodeFactory(ast.NodeFactoryHooks{})
			var root *ast.Node
			root, err = s09Tree(factory, item)
			if err == nil {
				wire, _, err = encoder.EncodeNode(root, nil)
			}
		}
		if err != nil {
			t.Fatalf("prepare %s: %v", item.Name, err)
		}
		if item.Mutation != "" {
			rootOffset := binary.LittleEndian.Uint32(wire[encoder.HeaderOffsetNodes:]) + encoder.NodeSize
			switch item.Mutation {
			case "root-list-sentinel":
				binary.LittleEndian.PutUint32(wire[rootOffset:], encoder.SyntaxKindNodeList)
			case "root-synthetic-expression":
				binary.LittleEndian.PutUint32(wire[rootOffset:], uint32(ast.KindSyntheticExpression))
				binary.LittleEndian.PutUint32(wire[rootOffset+encoder.NodeOffsetData:], encoder.NodeDataTypeChildren)
			default:
				t.Fatalf("prepare %s: unknown mutation %q", item.Name, item.Mutation)
			}
		}
		rows = append(rows, s09Print(item, wire))
	}
	sum := sha256.Sum256(raw)
	output, err := json.Marshal(map[string]any{
		"request_sha256": hex.EncodeToString(sum[:]),
		"go":             runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH,
		"scope": "pinned EncodeNode/DecodeNodes/Printer.Emit observations; printing only, not insertion formatting or Rust ownership evidence",
		"rows":  rows,
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), output, 0o600); err != nil {
		t.Fatal(err)
	}
}
