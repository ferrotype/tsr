package checker

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/nodebuilder"
	"github.com/microsoft/TypeScript/tsc/internal/printer"
)

func TestC2ElisionBranches(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var request struct {
		Cases []struct {
			ID           string `json:"id"`
			Operation    string `json:"operation"`
			NoTruncation bool   `json:"no_truncation"`
			Length       int    `json:"length"`
			Count        int    `json:"count"`
			Bare         bool   `json:"bare"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(raw, &request); err != nil {
		t.Fatal(err)
	}
	cases := []any{}
	for _, q := range request.Cases {
		emit := printer.NewEmitContext()
		b := newNodeBuilderImpl(&Checker{}, emit, nil)
		var flags nodebuilder.Flags
		if q.NoTruncation {
			flags = nodebuilder.FlagsNoTruncation
		}
		b.ctx = &NodeBuilderContext{flags: flags, approximateLength: q.Length, maxExpansionDepth: -1}
		var node *ast.Node
		var commentNodes []*ast.Node
		switch q.Operation {
		case "placeholder":
			node = b.createElidedInformationPlaceholder()
		case "conditional":
			node = b.conditionalTypeToTypeNode(nil)
		case "list":
			types := make([]*Type, q.Count)
			for i := range types {
				types[i] = &Type{flags: TypeFlagsNumber}
			}
			list := b.mapToTypeNodes(types, q.Bare)
			commentNodes = list.Nodes
			node = b.f.NewTupleTypeNode(list)
		default:
			t.Fatal("unknown operation")
		}
		if commentNodes == nil {
			commentNodes = []*ast.Node{node}
		}
		comments := []any{}
		for i, n := range commentNodes {
			for _, comment := range emit.GetSyntheticLeadingComments(n) {
				comments = append(comments, map[string]any{"index": i, "text": comment.Text, "kind": comment.Kind, "trailing_newline": comment.HasTrailingNewLine})
			}
		}
		writer := printer.NewTextWriter("", 0)
		printer.NewPrinter(printer.PrinterOptions{}, printer.PrintHandlers{}, emit).Write(node, nil, writer, nil)
		cases = append(cases, map[string]any{"id": q.ID, "text": writer.String(), "added_length": b.ctx.approximateLength - q.Length, "comments": comments})
	}
	hash := sha256.Sum256(raw)
	output := map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "cases": cases}
	encoded, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), encoded, 0600); err != nil {
		t.Fatal(err)
	}
}
