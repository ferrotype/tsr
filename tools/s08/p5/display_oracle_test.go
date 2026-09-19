package checker_test

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/nodebuilder"
	"github.com/microsoft/TypeScript/tsc/internal/printer"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

func TestS08P5Display(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var request struct {
		Version  int `json:"version"`
		Programs []struct {
			ID      string            `json:"id"`
			Files   map[string]string `json:"files"`
            FileBytes map[string]string `json:"file_bytes"`
			Roots   []string          `json:"roots"`
			Module  string            `json:"module"`
			Queries []struct {
				ID            string  `json:"id"`
				Declaration   string  `json:"declaration"`
				Context       *string `json:"context"`
                EnclosingDeclaration string `json:"enclosing_declaration"`
				Operation     string  `json:"operation"`
				Flags         uint32  `json:"flags"`
				InternalFlags int32   `json:"internal_flags"`
				Meaning       uint32  `json:"meaning"`
			} `json:"queries"`
		} `json:"programs"`
	}
	if err := json.Unmarshal(raw, &request); err != nil {
		t.Fatal(err)
	}
	if request.Version != 1 {
		t.Fatal("unknown request version")
	}
	programs := []any{}
	for _, r := range request.Programs {
        for name, data := range r.FileBytes {
            if _, exists := r.Files[name]; exists { t.Fatal("duplicate source encoding") }
            decoded, err := hex.DecodeString(data)
            if err != nil { t.Fatal(err) }
            r.Files[name] = string(decoded)
        }
		host := compiler.NewCompilerHost("/", vfstest.FromMap(r.Files, true), "/no-default-lib", nil, nil, nil)
		opts := &core.CompilerOptions{Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext, Strict: core.TSTrue, NoLib: core.TSTrue}
		switch r.Module {
		case "", "esnext":
		case "node16": opts.Module = core.ModuleKindNode16
		case "nodenext": opts.Module = core.ModuleKindNodeNext
		default: t.Fatal("unknown display module option")
		}
		config := tsoptions.NewParsedCommandLine(opts, r.Roots, nil, tspath.ComparePathsOptions{})
		program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
		program.BindSourceFiles()
		c, _ := checker.NewChecker(program, nil)
		for _, file := range program.SourceFiles() {
			c.GetDiagnostics(context.Background(), file)
		}
		c.GetGlobalDiagnostics()
		queries := []any{}
		for _, q := range r.Queries {
            findDeclaration := func(name string) *ast.Node {
                var found *ast.Node
                var visit func(*ast.Node) bool
                visit = func(n *ast.Node) bool {
                    if (n.Kind == ast.KindTypeAliasDeclaration || n.Kind == ast.KindVariableDeclaration || n.Kind == ast.KindInterfaceDeclaration || n.Kind == ast.KindNamespaceExport) && n.Name() != nil && n.Name().Text() == name {
                        if found != nil { t.Fatal("ambiguous declaration", name) }
                        found = n
                    }
                    n.ForEachChild(visit)
                    return false
                }
                for _, f := range program.SourceFiles() { visit(f.AsNode()) }
                if found == nil { t.Fatal("missing declaration", name) }
                return found
            }
            declaration := findDeclaration(q.Declaration)
            contextDeclaration := declaration
            if q.EnclosingDeclaration != "" { contextDeclaration = findDeclaration(q.EnclosingDeclaration) }
			var enclosing *ast.Node
			if q.Context != nil {
				switch *q.Context {
				case "declaration":
					enclosing = contextDeclaration
				case "source":
					enclosing = ast.GetSourceFileOfNode(contextDeclaration).AsNode()
				default:
					t.Fatal("unknown context")
				}
			}
			result := map[string]any{"id": q.ID, "state": "content"}
			if q.Operation == "symbol_string" {
				symbol := c.GetSymbolAtLocation(declaration.Name())
				if symbol == nil {
					result["state"] = "absent"
				} else {
					result["text_hex"] = hex.EncodeToString([]byte(c.SymbolToStringEx(symbol, enclosing, ast.SymbolFlags(q.Meaning), checker.SymbolFormatFlags(q.Flags))))
				}
				queries = append(queries, result)
				continue
			}
			typ := c.GetTypeAtLocation(declaration.Name())
			switch q.Operation {
			case "type_string":
				result["text_hex"] = hex.EncodeToString([]byte(c.TypeToStringEx(typ, enclosing, checker.TypeFormatFlags(q.Flags), nil)))
			case "type_node":
				emit := printer.NewEmitContext()
				b := checker.NewNodeBuilder(c, emit)
				node := b.TypeToTypeNode(typ, enclosing, nodebuilder.Flags(q.Flags), nodebuilder.InternalFlags(q.InternalFlags), nil)
				if node == nil {
					result["state"] = "absent"
				} else {
					result["kind"] = node.Kind
					writer := printer.NewTextWriter("", 0)
					p := printer.NewPrinter(printer.PrinterOptions{RemoveComments: true}, printer.PrintHandlers{}, emit)
					p.Write(node, ast.GetSourceFileOfNode(enclosing), writer, nil)
					result["text_hex"] = hex.EncodeToString([]byte(writer.String()))
				}
			default:
				t.Fatal("unknown operation")
			}
			queries = append(queries, result)
		}
		programs = append(programs, map[string]any{"id": r.ID, "queries": queries})
	}
	hash := sha256.Sum256(raw)
	output := map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "programs": programs}
	encoded, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), encoded, 0600); err != nil {
		t.Fatal(err)
	}
}
