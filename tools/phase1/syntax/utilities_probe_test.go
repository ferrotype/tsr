package parser

// Phase 1 F4a, `parseOutputs` group (plan task 5, the missing witnesses):
// access only. The E1 encoder never observes several things the parser
// produces: the comment directives the scanner collects, the pragma list, the
// check-js directive chosen from it, UsesUriStyleNodeCoreModules and the
// reparsed clones (parser.go:467-485, references.go:35-42), and it reads JSDoc
// only eagerly. This probe records exactly those, and the lazy JSDoc contract
// for non-JS files (parser.go:481-483): EagerJSDoc is empty before first use,
// JSDoc parses and caches, a repeated JSDoc returns the identical nodes, and
// EagerJSDoc afterwards returns them too.
//
// A node is [kind, pos, end, ordinal], the ordinal being first-seen order in
// the request, so identity is compared. Every name is prefixed `phase1`.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"slices"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

type phase1ParseOutputsRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	File      struct {
		Name       string `json:"name"`
		Text       string `json:"text"`
		ScriptKind int    `json:"script_kind"`
	} `json:"file"`
	Actions []struct {
		Op string `json:"op"`
	} `json:"actions"`
}

type phase1Outputs struct {
	file     *ast.SourceFile
	ordinals map[*ast.Node]int
}

func (o *phase1Outputs) node(node *ast.Node) any {
	ordinal, ok := o.ordinals[node]
	if !ok {
		ordinal = len(o.ordinals)
		o.ordinals[node] = ordinal
	}
	return []any{int(node.Kind), node.Pos(), node.End(), ordinal}
}

func (o *phase1Outputs) nodes(nodes []*ast.Node) []any {
	out := []any{}
	for _, node := range nodes {
		out = append(out, o.node(node))
	}
	return out
}

func phase1Preorder(root *ast.Node) []*ast.Node {
	nodes := []*ast.Node{}
	var visit func(node *ast.Node) bool
	visit = func(node *ast.Node) bool {
		nodes = append(nodes, node)
		node.ForEachChild(visit)
		return false
	}
	visit(root)
	return nodes
}

func (o *phase1Outputs) sideFields() any {
	file := o.file
	directives := []any{}
	for _, d := range file.CommentDirectives {
		directives = append(directives, []any{int(d.Kind), d.Loc.Pos(), d.Loc.End()})
	}
	pragmas := []any{}
	for _, p := range file.Pragmas {
		names := make([]string, 0, len(p.Args))
		for name := range p.Args {
			names = append(names, name)
		}
		slices.Sort(names)
		args := []any{}
		for _, name := range names {
			arg := p.Args[name]
			args = append(args, []any{name, arg.Name, arg.Value, arg.Pos(), arg.End()})
		}
		pragmas = append(pragmas, []any{p.Name, int(p.Kind), p.Pos(), p.End(), p.HasTrailingNewLine, args})
	}
	var checkJs any
	if d := file.CheckJsDirective; d != nil {
		checkJs = []any{d.Enabled, int(d.Range.Kind), d.Range.Pos(), d.Range.End(), d.Range.HasTrailingNewLine}
	}
	inTree := map[*ast.Node]bool{}
	for _, node := range phase1Preorder(file.AsNode()) {
		inTree[node] = true
	}
	clones := []any{}
	for _, clone := range file.ReparsedClones {
		parent := -1
		if clone.Parent != nil {
			parent = int(clone.Parent.Kind)
		}
		clones = append(clones, []any{int(clone.Kind), clone.Pos(), clone.End(), int(clone.Flags), parent, inTree[clone]})
	}
	return []any{directives, pragmas, checkJs, int(file.UsesUriStyleNodeCoreModules), clones}
}

func (o *phase1Outputs) lazyJSDoc() any {
	file := o.file
	rows := []any{}
	for index, node := range phase1Preorder(file.AsNode()) {
		if node.Flags&ast.NodeFlagsHasJSDoc == 0 {
			continue
		}
		eagerBefore := o.nodes(node.EagerJSDoc(file))
		first := node.JSDoc(file)
		lazyFirst := o.nodes(first)
		lazySecond := o.nodes(node.JSDoc(file))
		eagerAfter := o.nodes(node.EagerJSDoc(file))
		trees := []any{}
		for _, jsdoc := range first {
			tree := []any{}
			for _, child := range phase1Preorder(jsdoc) {
				tree = append(tree, o.node(child))
			}
			trees = append(trees, tree)
		}
		rows = append(rows, []any{index, eagerBefore, lazyFirst, lazySecond, eagerAfter, trees})
	}
	return rows
}

func TestPhase1SyntaxParseOutputs(t *testing.T) {
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
		var request phase1ParseOutputsRequest
		if err := json.Unmarshal(raw, &request); err != nil {
			t.Fatal(err)
		}
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject != "parseOutputs" {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the parseOutputs probe"
			observations = append(observations, row)
			continue
		}
		options := ast.SourceFileParseOptions{FileName: request.File.Name, Path: tspath.Path(request.File.Name)}
		file := ParseSourceFile(options, request.File.Text, core.ScriptKind(request.File.ScriptKind))
		outputs := &phase1Outputs{file: file, ordinals: map[*ast.Node]int{}}
		ordered := []any{}
		for _, action := range request.Actions {
			switch action.Op {
			case "side_fields":
				ordered = append(ordered, outputs.sideFields())
			case "lazy_jsdoc":
				ordered = append(ordered, outputs.lazyJSDoc())
			default:
				t.Fatalf("%s: unknown action %s", request.Case, action.Op)
			}
		}
		row["result"] = "observed"
		row["observation"] = map[string]any{"ordered": ordered, "nodes_seen": len(outputs.ordinals)}
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
