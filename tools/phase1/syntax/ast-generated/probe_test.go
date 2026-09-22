package ast

// Access-only generated-AST operation probe. Expected values are never encoded
// here: factories, typed operations, callbacks and facts run at the pinned source.
import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"os"
	"runtime"
	"testing"
)

type phase1GeneratedRequest struct {
	Mode       string              `json:"mode"`
	Case       string              `json:"case"`
	Operation  string              `json:"operation"`
	Subject    string              `json:"subject"`
	Shape      string              `json:"shape"`
	List       string              `json:"list"`
	Absent     bool                `json:"absent"`
	NameFirst  bool                `json:"name_first"`
	Predicate  string              `json:"predicate"`
	FirstKind  int                 `json:"first_kind"`
	LastKind   int                 `json:"last_kind"`
	ExtraKinds []int               `json:"extra_kinds"`
	Actions    []map[string]string `json:"actions"`
}

func phase1GeneratedPos(node *Node) any {
	if node == nil {
		return nil
	}
	return node.Pos()
}
func phase1GeneratedChildren(node *Node, stop int) any {
	values := []any{}
	stopped := node.ForEachChild(func(n *Node) bool {
		values = append(values, phase1GeneratedPos(n))
		return stop != 0 && len(values) == stop
	})
	return []any{values, stopped}
}
func phase1GeneratedList(node *Node, shape string) *NodeList {
	switch shape {
	case "Block":
		return node.AsBlock().Statements
	case "JSDocParameterOrPropertyTag":
		return node.AsJSDocParameterOrPropertyTag().Comment
	}
	return nil
}
func phase1GeneratedSnapshot(node *Node, shape string) any {
	var list any
	if l := phase1GeneratedList(node, shape); l != nil {
		nodes := []any{}
		for _, n := range l.Nodes {
			nodes = append(nodes, phase1GeneratedPos(n))
		}
		list = []any{l.Pos(), l.End(), l.Nodes == nil, nodes}
	}
	var fields any
	switch shape {
	case "QualifiedName":
		d := node.AsQualifiedName()
		fields = []any{phase1GeneratedPos(d.Left), phase1GeneratedPos(d.Right)}
	case "Block":
		fields = []any{node.AsBlock().MultiLine}
	case "JSDocParameterOrPropertyTag":
		d := node.AsJSDocParameterOrPropertyTag()
		fields = []any{phase1GeneratedPos(d.TagName), phase1GeneratedPos(d.name), d.IsBracketed, phase1GeneratedPos(d.TypeExpression), d.IsNameFirst}
	}
	return []any{int(node.Kind), uint32(node.Flags), node.Pos(), node.End(), fields, list, phase1GeneratedChildren(node, 0)}
}
func phase1GeneratedUpdate(f *NodeFactory, n *Node, shape string, changed bool, replacement *Node) *Node {
	switch shape {
	case "QualifiedName":
		d := n.AsQualifiedName()
		left := d.Left
		if changed {
			left = replacement
		}
		return f.UpdateQualifiedName(d, left, d.Right)
	case "Block":
		d := n.AsBlock()
		multi := d.MultiLine
		if changed {
			multi = !multi
		}
		return f.UpdateBlock(d, d.Statements, multi)
	case "JSDocParameterOrPropertyTag":
		d := n.AsJSDocParameterOrPropertyTag()
		name := d.name
		if changed {
			name = replacement
		}
		return f.UpdateJSDocParameterOrPropertyTag(d, d.TagName, name, d.IsBracketed, d.TypeExpression, d.IsNameFirst, d.Comment)
	}
	panic("unhandled generated shape")
}
func phase1GeneratedValidateActions(r phase1GeneratedRequest) error {
	if r.Operation != "tsc/internal/ast/ast_generated.go:NodeFactory.New"+r.Shape {
		return fmt.Errorf("generated AST operation does not match selected factory")
	}
	expected := []string{"new", "children-stop", "update-same", "update-changed", "clone", "visit-same", "visit-replace"}
	if r.Shape == "QualifiedName" || r.Shape == "Block" {
		expected = append(expected, "facts")
	}
	expected = append(expected, "counts")
	if len(r.Actions) != len(expected) {
		return fmt.Errorf("generated AST action schedule differs from executable trace")
	}
	for i, name := range expected {
		if len(r.Actions[i]) != 1 || r.Actions[i]["op"] != name {
			return fmt.Errorf("generated AST action schedule differs from executable trace")
		}
	}
	return nil
}
func phase1GeneratedRun(r phase1GeneratedRequest) any {
	hooks := []any{}
	record := func(stage string, n *Node) {
		hooks = append(hooks, []any{stage, int(n.Kind), uint32(n.Flags), n.Pos(), n.End()})
	}
	take := func() []any { out := hooks; hooks = []any{}; return out }
	f := NewNodeFactory(NodeFactoryHooks{OnCreate: func(n *Node) { record("create", n) }, OnUpdate: func(n, _ *Node) { record("update", n) }, OnClone: func(n, _ *Node) { record("clone", n) }})
	sentinels := []*Node{}
	for pos := 1; pos <= 5; pos++ {
		var n *Node
		if pos == 2 {
			n = f.NewAwaitExpression(f.NewKeywordExpression(KindThisKeyword))
		} else {
			n = f.NewIdentifier(fmt.Sprintf("n%d", pos))
		}
		n.Loc = core.NewTextRange(pos, pos+1)
		sentinels = append(sentinels, n)
	}
	var selectedList *NodeList
	switch r.List {
	case "nil":
	case "empty":
		selectedList = f.NewNodeList([]*Node{})
	case "nodes":
		selectedList = f.NewNodeList([]*Node{sentinels[3], sentinels[1]})
	case "nil-element":
		selectedList = f.NewNodeList([]*Node{sentinels[3], nil})
	default:
		panic("unknown list mode")
	}
	if selectedList != nil {
		selectedList.Loc = core.NewTextRange(8, 12)
	}
	take()
	edge := func(i int) *Node {
		if r.Absent {
			return nil
		}
		return sentinels[i]
	}
	var node *Node
	switch r.Shape {
	case "QualifiedName":
		node = f.NewQualifiedName(edge(0), edge(1))
	case "Block":
		node = f.NewBlock(selectedList, true)
	case "JSDocParameterOrPropertyTag":
		node = f.NewJSDocParameterOrPropertyTag(KindJSDocParameterTag, edge(0), edge(1), true, edge(2), r.NameFirst, selectedList)
	default:
		panic("unknown generated shape")
	}
	out := []any{[]any{"new", phase1GeneratedSnapshot(node, r.Shape), take()}}
	node.Flags = 128
	node.Loc = core.NewTextRange(17, 29)
	out = append(out, []any{"children-stop", phase1GeneratedChildren(node, 1)})
	same := phase1GeneratedUpdate(f, node, r.Shape, false, sentinels[4])
	out = append(out, []any{"update-same", same == node, phase1GeneratedSnapshot(same, r.Shape), take()})
	changed := phase1GeneratedUpdate(f, node, r.Shape, true, sentinels[4])
	out = append(out, []any{"update-changed", changed == node, phase1GeneratedSnapshot(changed, r.Shape), take()})
	cloned := node.Clone(f)
	out = append(out, []any{"clone", cloned == node, phase1GeneratedList(cloned, r.Shape) == phase1GeneratedList(node, r.Shape), phase1GeneratedSnapshot(cloned, r.Shape), take()})
	for _, replace := range []bool{false, true} {
		calls := []any{}
		visitor := NewNodeVisitor(func(n *Node) *Node {
			calls = append(calls, phase1GeneratedPos(n))
			if replace && n == sentinels[1] {
				return sentinels[4]
			}
			return n
		}, f, NodeVisitorHooks{})
		visited := visitor.VisitEachChild(node)
		label := "visit-same"
		if replace {
			label = "visit-replace"
		}
		out = append(out, []any{label, visited == node, phase1GeneratedList(visited, r.Shape) == phase1GeneratedList(node, r.Shape), calls, phase1GeneratedSnapshot(visited, r.Shape), take()})
	}
	if r.Shape == "QualifiedName" || r.Shape == "Block" {
		out = append(out, []any{"facts", uint32(node.SubtreeFacts())})
	}
	out = append(out, []any{"counts", f.NodeCount(), f.TextCount()})
	return map[string]any{"ordered": out}
}

// Decode only the common header until the subject belongs to this probe.
// Other syntax groups have heterogeneous action payloads (arrays and numbers).
func phase1GeneratedDecodeRequest(raw json.RawMessage) (phase1GeneratedRequest, bool, error) {
	var header struct {
		Case      string `json:"case"`
		Operation string `json:"operation"`
		Subject   string `json:"subject"`
	}
	if err := json.Unmarshal(raw, &header); err != nil {
		return phase1GeneratedRequest{}, false, err
	}
	r := phase1GeneratedRequest{Case: header.Case, Operation: header.Operation, Subject: header.Subject}
	served := header.Subject == "generatedAst" || header.Subject == "generatedPredicate" || header.Subject == "generatedShape" || header.Subject == "generatedSpecial"
	if !served {
		return r, false, nil
	}
	err := json.Unmarshal(raw, &r)
	return r, true, err
}

func testPhase1GeneratedMixedRequestDecoding(t *testing.T) {
	foreign := json.RawMessage(`{"case":"foreign","operation":"foreign-op","subject":"astnav","actions":[{"op":"child_of_kind","kinds":[1,2],"position":7}]}`)
	r, served, err := phase1GeneratedDecodeRequest(foreign)
	if err != nil || served || r.Case != "foreign" || r.Operation != "foreign-op" || r.Subject != "astnav" {
		t.Fatalf("foreign request was not declined by its header: %+v, %v, %v", r, served, err)
	}
	valid := json.RawMessage(`{"case":"generated","operation":"generated-op","subject":"generatedAst","actions":[{"op":"counts"}]}`)
	r, served, err = phase1GeneratedDecodeRequest(valid)
	if err != nil || !served || len(r.Actions) != 1 || r.Actions[0]["op"] != "counts" {
		t.Fatalf("served request did not decode its typed payload: %+v, %v, %v", r, served, err)
	}
	for _, subject := range []string{"generatedAst", "generatedPredicate", "generatedShape", "generatedSpecial"} {
		malformed := json.RawMessage(fmt.Sprintf(`{"subject":%q,"actions":[{"op":"child_of_kind","kinds":[1,2]}]}`, subject))
		if _, served, err := phase1GeneratedDecodeRequest(malformed); !served || err == nil {
			t.Fatalf("malformed served payload was silently declined for %s", subject)
		}
	}
}

func TestPhase1GeneratedAST(t *testing.T) {
	t.Run("mixed-request-decoding", testPhase1GeneratedMixedRequestDecoding)
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []json.RawMessage `json:"requests"`
	}
	if err = json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}
	rows := []any{}
	for _, raw := range document.Requests {
		r, served, err := phase1GeneratedDecodeRequest(raw)
		if err != nil {
			t.Fatal(err)
		}
		row := map[string]any{"case": r.Case, "operation": r.Operation}
		if !served {
			row["result"] = "native_unavailable"
			row["reason"] = "subject not served by generated AST probe"
		} else {
			row["result"] = "observed"
			if r.Subject == "generatedSpecial" {
				observation, err := phase1ShapeSpecial(r)
				if err != nil {
					t.Fatal(err)
				}
				row["observation"] = observation
			} else if r.Subject == "generatedShape" {
				observation, err := phase1ShapeRun(r)
				if err != nil {
					t.Fatal(err)
				}
				row["observation"] = observation
			} else if r.Subject == "generatedPredicate" {
				if r.Operation != "tsc/internal/ast/ast_generated.go:"+r.Predicate {
					t.Fatal("generated predicate operation does not match selected predicate")
				}
				kinds := []int{}
				for kind := r.FirstKind; kind <= r.LastKind; kind++ {
					kinds = append(kinds, kind)
				}
				kinds = append(kinds, r.ExtraKinds...)
				answers := []any{}
				f := NewNodeFactory(NodeFactoryHooks{})
				for _, kind := range kinds {
					answers = append(answers, []any{kind, phase1GeneratedPredicate(r.Predicate, f.NewToken(Kind(kind)))})
				}
				row["observation"] = map[string]any{"ordered": answers}
			} else {
				if err := phase1GeneratedValidateActions(r); err != nil {
					t.Fatal(err)
				}
				row["observation"] = phase1GeneratedRun(r)
			}
		}
		rows = append(rows, row)
	}
	hash := sha256.Sum256(input)
	output := map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "observations": rows}
	data, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0644); err != nil {
		t.Fatal(err)
	}
}
