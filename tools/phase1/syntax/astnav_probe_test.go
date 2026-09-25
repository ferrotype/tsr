package astnav

// Phase 1 F4a, `astnav` group (plan task 6): access only. Each request is one
// source file and an ordered list of navigation actions. The probe parses the
// file with the pinned parser and answers every action with the real astnav
// entry points -- never a second scanner -- and carries no expected value.
//
// A position sweep covers every byte offset from 0 to len(text) INCLUSIVE; the
// upstream JSON baselines stop one short, so the end-of-file position is new
// here. Consecutive positions with the same answer are run-length encoded as
// [first, last, answer].
//
// A node answer is [kind, pos, end, ordinal]. The ordinal is the order in
// which this request first saw that node object, so pointer identity is
// compared, not just kind and range: GetOrCreateToken must hand back the same
// token to a repeated question and to a different operation that reaches it,
// and two distinct nodes with the same range must stay distinct.
// `child_of_kind_payload` appends what the node carries beyond that (text,
// token flags, raw text, template flags), so the payload createToken builds is
// compared too. A recovered panic is ["panic", message], because a panic is the
// pinned outcome of some FindNextToken questions (tokens.go:682).
//
// Every name is prefixed `phase1` so it cannot collide with a pinned helper.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

type phase1NavRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	File      struct {
		Name       string `json:"name"`
		Text       string `json:"text"`
		ScriptKind int    `json:"script_kind"`
	} `json:"file"`
	Actions []struct {
		Op    string   `json:"op"`
		Kinds []string `json:"kinds"`
	} `json:"actions"`
}

type phase1Nav struct {
	file     *ast.SourceFile
	ordinals map[*ast.Node]int
	kinds    map[string]ast.Kind
}

func (n *phase1Nav) node(node *ast.Node) any {
	if node == nil {
		return nil
	}
	ordinal, ok := n.ordinals[node]
	if !ok {
		ordinal = len(n.ordinals)
		n.ordinals[node] = ordinal
	}
	return []any{int(node.Kind), node.Pos(), node.End(), ordinal}
}

// payload is a node answer followed by what the node carries beyond its kind
// and range: a name's text; a literal's text and token flags; for a template
// piece also its raw text and template flags; for JSX text also whether it is
// only whitespace. A token that GetOrCreateToken makes gets all of these from
// createToken (ast.go:2935), so `child_of_kind_payload` compares that
// constructor's output, which a plain node answer cannot see.
func (n *phase1Nav) payload(node *ast.Node) any {
	answer := n.node(node)
	if node == nil {
		return answer
	}
	fields := answer.([]any)
	switch node.Kind {
	case ast.KindIdentifier, ast.KindPrivateIdentifier:
		fields = append(fields, node.Text())
	case ast.KindNumericLiteral, ast.KindBigIntLiteral, ast.KindStringLiteral, ast.KindRegularExpressionLiteral:
		data := node.LiteralLikeData()
		fields = append(fields, data.Text, int(data.TokenFlags))
	case ast.KindJsxText:
		data := node.AsJsxText()
		fields = append(fields, data.Text, int(data.TokenFlags), data.ContainsOnlyTriviaWhiteSpaces)
	case ast.KindNoSubstitutionTemplateLiteral, ast.KindTemplateHead, ast.KindTemplateMiddle, ast.KindTemplateTail:
		data := node.TemplateLiteralLikeData()
		fields = append(fields, data.Text, int(data.TokenFlags), data.RawText, int(data.TemplateFlags))
	}
	return fields
}

// answer runs one question, turning a panic into its pinned message.
func phase1Answer(question func() any) (result any) {
	defer func() {
		if value := recover(); value != nil {
			result = []any{"panic", fmt.Sprint(value)}
		}
	}()
	return question()
}

func (n *phase1Nav) sweep(question func(position int) any) []any {
	runs := []any{}
	var previous any
	start := 0
	encode := func(value any) string {
		data, _ := json.Marshal(value)
		return string(data)
	}
	for position := 0; position <= len(n.file.Text()); position++ {
		value := phase1Answer(func() any { return question(position) })
		if position > 0 && encode(value) == encode(previous) {
			continue
		}
		if position > 0 {
			runs = append(runs, []any{start, position - 1, previous})
		}
		start, previous = position, value
	}
	return append(runs, []any{start, len(n.file.Text()), previous})
}

// preorder lists every node reached by ForEachChild from the file, parents
// first, so a containing node is identified by its index.
func (n *phase1Nav) preorder() []*ast.Node {
	nodes := []*ast.Node{}
	var visit func(node *ast.Node) bool
	visit = func(node *ast.Node) bool {
		nodes = append(nodes, node)
		node.ForEachChild(visit)
		return false
	}
	visit(n.file.AsNode())
	return nodes
}

func (n *phase1Nav) action(op string, kinds []string) any {
	file := n.file
	token := func(position int) *ast.Node { return GetTokenAtPosition(file, position) }
	switch op {
	case "token_at", "token_at_repeat":
		return n.sweep(func(p int) any { return n.node(token(p)) })
	case "touching_property_name":
		return n.sweep(func(p int) any { return n.node(GetTouchingPropertyName(file, p)) })
	case "touching_token":
		return n.sweep(func(p int) any { return n.node(GetTouchingToken(file, p)) })
	case "preceding":
		return n.sweep(func(p int) any { return n.node(FindPrecedingToken(file, p)) })
	case "preceding_exclude_jsdoc":
		return n.sweep(func(p int) any { return n.node(FindPrecedingTokenEx(file, p, nil, true)) })
	case "next_in_file":
		return n.sweep(func(p int) any { return n.node(FindNextToken(token(p), file.AsNode(), file)) })
	case "next_in_parent":
		return n.sweep(func(p int) any {
			previous := token(p)
			return n.node(FindNextToken(previous, previous.Parent, file))
		})
	case "start_of_token":
		return n.sweep(func(p int) any { return GetStartOfNode(token(p), file, false) })
	case "start_of_token_with_jsdoc":
		return n.sweep(func(p int) any { return GetStartOfNode(token(p), file, true) })
	case "child_of_kind", "child_of_kind_payload":
		answer := n.node
		if op == "child_of_kind_payload" {
			answer = n.payload
		}
		rows := []any{}
		for index, container := range n.preorder() {
			for _, name := range kinds {
				kind, ok := n.kinds[name]
				if !ok {
					panic("unknown kind " + name)
				}
				rows = append(rows, []any{index, name, phase1Answer(func() any {
					return answer(FindChildOfKind(container, kind, file))
				})})
			}
		}
		return rows
	case "visit_nodes", "visit_lists":
		// The pinned visitor calls both hooks for EVERY child slot, including
		// absent ones (ast/visitor.go visitNode/visitNodes pass nil through).
		// Every pinned consumer nil-checks first, so `visit_nodes` records the
		// calls a consumer acts on; `visit_lists` records all of them, nil
		// calls included, together with each list's own range.
		full := op == "visit_lists"
		rows := []any{}
		for index, node := range n.preorder() {
			visits := []any{}
			VisitEachChildAndJSDoc(node, file,
				func(child *ast.Node, _ *ast.NodeVisitor) *ast.Node {
					if child != nil || full {
						visits = append(visits, []any{"node", n.node(child)})
					}
					return child
				},
				func(list *ast.NodeList, _ *ast.NodeVisitor) *ast.NodeList {
					if list == nil {
						if full {
							visits = append(visits, []any{"list", nil})
						}
						return list
					}
					members := []any{}
					for _, child := range list.Nodes {
						members = append(members, n.node(child))
					}
					if op == "visit_lists" {
						visits = append(visits, []any{"list", list.Pos(), list.End(), members})
					} else {
						visits = append(visits, []any{"list", members})
					}
					return list
				})
			rows = append(rows, []any{index, visits})
		}
		return rows
	}
	panic("unknown astnav action " + op)
}

func TestPhase1SyntaxAstnav(t *testing.T) {
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
	kinds := map[string]ast.Kind{}
	for kind := ast.Kind(0); !strings.HasPrefix(kind.String(), "Kind("); kind++ {
		kinds[kind.String()] = kind
	}
	observations := make([]map[string]any, 0, len(document.Requests))
	for _, raw := range document.Requests {
		var request phase1NavRequest
		if err := json.Unmarshal(raw, &request); err != nil {
			t.Fatal(err)
		}
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject != "astnav" {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the astnav probe"
			observations = append(observations, row)
			continue
		}
		options := ast.SourceFileParseOptions{FileName: request.File.Name, Path: tspath.Path(request.File.Name)}
		file := parser.ParseSourceFile(options, request.File.Text, core.ScriptKind(request.File.ScriptKind))
		nav := &phase1Nav{file: file, ordinals: map[*ast.Node]int{}, kinds: kinds}
		ordered := []any{}
		for _, action := range request.Actions {
			ordered = append(ordered, nav.action(action.Op, action.Kinds))
		}
		row["result"] = "observed"
		row["observation"] = map[string]any{"ordered": ordered, "nodes_seen": len(nav.ordinals)}
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
