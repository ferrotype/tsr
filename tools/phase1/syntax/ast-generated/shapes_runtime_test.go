package ast

import (
	"fmt"
	"github.com/microsoft/TypeScript/tsc/internal/core"
)

type phase1ShapeInputs struct {
	absent                  bool
	allNodes, replacements  []*Node
	lists, changedLists     []*NodeList
	mods, changedMods       []*ModifierList
	raw, changedRaw         [][]*Node
	textValues, changedText [][]string
}

func phase1ShapeNewInputs(f *NodeFactory, mode string) *phase1ShapeInputs {
	b := &phase1ShapeInputs{absent: mode == "nil"}
	for i := 0; i < 16; i++ {
		var node *Node
		if i%2 == 0 {
			node = f.NewAwaitExpression(f.NewKeywordExpression(KindThisKeyword))
		} else {
			node = f.NewIdentifier(fmt.Sprintf("node%d", i))
		}
		replacement := f.NewIdentifier(fmt.Sprintf("replacement%d", i))
		modifier := f.NewToken(KindExportKeyword)
		changedModifier := f.NewToken(KindAsyncKeyword)
		for _, pair := range []struct {
			node *Node
			pos  int
		}{{node, 10*i + 1}, {replacement, 1000 + 10*i + 1}, {modifier, 10*i + 2}, {changedModifier, 1000 + 10*i + 2}} {
			pair.node.Loc = core.NewTextRange(pair.pos, pair.pos+1)
		}
		b.allNodes = append(b.allNodes, node)
		b.replacements = append(b.replacements, replacement)
		var raw []*Node
		switch mode {
		case "nil":
		case "empty":
			raw = []*Node{}
		case "nodes":
			raw = []*Node{node, replacement}
		case "nil-element":
			raw = []*Node{node, nil}
		default:
			panic("unknown shape mode")
		}
		changedRaw := []*Node{replacement}
		var list *NodeList
		if mode != "nil" {
			list = f.NewNodeList(raw)
			list.Loc = core.NewTextRange(10*i, 10*i+9)
		}
		changedList := f.NewNodeList(changedRaw)
		changedList.Loc = core.NewTextRange(1000+10*i, 1000+10*i+9)
		var mods *ModifierList
		if mode != "nil" {
			nodes := []*Node{modifier}
			if mode == "empty" {
				nodes = []*Node{}
			}
			mods = f.NewModifierList(nodes)
			mods.Loc = core.NewTextRange(10*i, 10*i+9)
		}
		changedMods := f.NewModifierList([]*Node{changedModifier})
		changedMods.Loc = core.NewTextRange(1000+10*i, 1000+10*i+9)
		var text []string
		if mode != "nil" {
			text = []string{}
			if mode != "empty" {
				text = []string{fmt.Sprintf("text%d", i)}
			}
		}
		b.raw = append(b.raw, raw)
		b.changedRaw = append(b.changedRaw, changedRaw)
		b.lists = append(b.lists, list)
		b.changedLists = append(b.changedLists, changedList)
		b.mods = append(b.mods, mods)
		b.changedMods = append(b.changedMods, changedMods)
		b.textValues = append(b.textValues, text)
		b.changedText = append(b.changedText, []string{fmt.Sprintf("changed%d", i)})
	}
	return b
}
func (b *phase1ShapeInputs) node(i int, changed bool) *Node {
	if changed {
		return b.replacements[i]
	}
	if b.absent {
		return nil
	}
	return b.allNodes[i]
}
func (b *phase1ShapeInputs) list(i int, changed bool) *NodeList {
	if changed {
		return b.changedLists[i]
	}
	return b.lists[i]
}
func (b *phase1ShapeInputs) modifiers(i int, changed bool) *ModifierList {
	if changed {
		return b.changedMods[i]
	}
	return b.mods[i]
}
func (b *phase1ShapeInputs) nodes(i int, changed bool) []*Node {
	if changed {
		return b.changedRaw[i]
	}
	return b.raw[i]
}
func (b *phase1ShapeInputs) texts(i int, changed bool) []string {
	if changed {
		return b.changedText[i]
	}
	return b.textValues[i]
}
func (b *phase1ShapeInputs) text(i int, changed bool) string {
	return fmt.Sprintf("field%d-%t", i, changed)
}
func (b *phase1ShapeInputs) boolean(i int, changed bool) bool { return (i%2 == 0) != changed }
func (b *phase1ShapeInputs) number(i int, changed bool) uint32 {
	v := uint32(1) << (i%5 + 1)
	if changed {
		v++
	}
	return v
}
func (b *phase1ShapeInputs) kind(kind Kind, changed bool) Kind {
	if changed {
		return KindMinusToken
	}
	return kind
}
func phase1ShapeNodesSnapshot(nodes []*Node) any {
	values := []any{}
	for _, node := range nodes {
		values = append(values, phase1GeneratedPos(node))
	}
	return []any{nodes == nil, values}
}
func phase1ShapeListSnapshot(list *NodeList) any {
	if list == nil {
		return nil
	}
	return []any{list.Pos(), list.End(), phase1ShapeNodesSnapshot(list.Nodes)}
}
func phase1ShapeModifiersSnapshot(list *ModifierList) any {
	if list == nil {
		return nil
	}
	return []any{list.Pos(), list.End(), phase1ShapeNodesSnapshot(list.Nodes)}
}
func phase1ShapeTextsSnapshot(text []string) any {
	values := []string{}
	values = append(values, text...)
	return []any{text == nil, values}
}
func phase1ShapeSnapshot(node *Node, shape string) any {
	return []any{node.Kind, uint32(node.Flags), node.Pos(), node.End(), phase1ShapeFields(node, shape)}
}
func phase1ShapeRun(r phase1GeneratedRequest) (any, error) {
	if r.Operation != "tsc/internal/ast/ast_generated.go:NodeFactory.New"+r.Shape {
		return nil, fmt.Errorf("shape operation identity mismatch")
	}
	labels := phase1ShapeActions(r.Shape)
	if len(r.Actions) != len(labels) {
		return nil, fmt.Errorf("shape action schedule mismatch")
	}
	for i, label := range labels {
		if len(r.Actions[i]) != 1 || r.Actions[i]["op"] != label.name {
			return nil, fmt.Errorf("shape action schedule mismatch")
		}
	}
	hooks := []any{}
	record := func(stage string, n *Node) {
		hooks = append(hooks, []any{stage, n.Kind, uint32(n.Flags), n.Pos(), n.End()})
	}
	take := func() []any { out := hooks; hooks = []any{}; return out }
	f := NewNodeFactory(NodeFactoryHooks{OnCreate: func(n *Node) { record("create", n) }, OnUpdate: func(n, _ *Node) { record("update", n) }, OnClone: func(n, _ *Node) { record("clone", n) }})
	inputs := phase1ShapeNewInputs(f, r.Mode)
	take()
	root := phase1ShapeMake(f, inputs, r.Shape)
	out := []any{[]any{"new", phase1ShapeSnapshot(root, r.Shape), take()}}
	root.Loc = core.NewTextRange(333, 444)
	root.Flags |= 128
	for _, action := range labels[1:] {
		label := action.name
		var row any
		switch label {
		case "cast":
			row = []any{label, phase1ShapeFields(root, r.Shape)}
		case "name":
			row = []any{label, phase1GeneratedPos(root.Name())}
		case "children-stop":
			row = []any{label, phase1GeneratedChildren(root, 0), phase1GeneratedChildren(root, 1)}
		case "clone":
			cloned := root.Clone(f)
			row = []any{label, cloned == root, phase1ShapeSnapshot(cloned, r.Shape), take()}
		case "visit-same", "visit-replace":
			calls := []any{}
			visitor := NewNodeVisitor(func(node *Node) *Node {
				calls = append(calls, phase1GeneratedPos(node))
				if label == "visit-replace" {
					for i, original := range inputs.allNodes {
						if node == original {
							return inputs.replacements[i]
						}
					}
				}
				return node
			}, f, NodeVisitorHooks{})
			visited := visitor.VisitEachChild(root)
			row = []any{label, visited == root, calls, phase1ShapeSnapshot(visited, r.Shape), take()}
		case "facts":
			row = []any{label, uint32(root.SubtreeFacts())}
		case "counts":
			row = []any{label, f.NodeCount(), f.TextCount()}
		default:
			updated := phase1ShapeUpdate(f, inputs, root, r.Shape, action.field)
			row = []any{label, updated == root, phase1ShapeSnapshot(updated, r.Shape), take()}
		}
		out = append(out, row)
	}
	return map[string]any{"ordered": out}, nil
}

func phase1ShapeSpecial(r phase1GeneratedRequest) (any, error) {
	var labels []string
	var operation string
	switch r.Shape {
	case "SourceFile":
		labels = []string{"new", "cast"}
		operation = "tsc/internal/ast/ast.go:NodeFactory.NewSourceFile"
	case "SyntheticExpression":
		labels = []string{"cast", "children-stop"}
		operation = "tsc/internal/ast/ast_generated.go:Node.AsSyntheticExpression"
	default:
		return nil, fmt.Errorf("unknown special shape")
	}
	if r.Operation != operation || len(r.Actions) != len(labels) {
		return nil, fmt.Errorf("special shape identity or actions changed")
	}
	for i, label := range labels {
		if len(r.Actions[i]) != 1 || r.Actions[i]["op"] != label {
			return nil, fmt.Errorf("special shape actions changed")
		}
	}
	f := NewNodeFactory(NodeFactoryHooks{})
	inputs := phase1ShapeNewInputs(f, r.Mode)
	if r.Shape == "SourceFile" {
		root := f.NewSourceFile(SourceFileParseOptions{FileName: "/generated.ts", Path: "/canonical/generated.ts", ExternalModuleIndicatorOptions: ExternalModuleIndicatorOptions{JSX: true}}, "let text = 'source';\n", inputs.list(0, false), inputs.node(1, false))
		d := root.AsSourceFile()
		opts := d.ParseOptions()
		fields := []any{d.FileName(), string(opts.Path), opts.ExternalModuleIndicatorOptions.JSX, opts.ExternalModuleIndicatorOptions.Force, d.Text(), phase1ShapeListSnapshot(d.Statements), phase1GeneratedPos(d.EndOfFileToken)}
		return map[string]any{"ordered": []any{[]any{"new", root.Kind, fields}, []any{"cast", fields}}}, nil
	}
	root := f.NewSyntheticExpression(nil, r.Mode != "nil", inputs.node(0, false))
	d := root.AsSyntheticExpression()
	return map[string]any{"ordered": []any{[]any{"cast", d.IsSpread, phase1GeneratedPos(d.TupleNameSource)}, []any{"children-stop", phase1GeneratedChildren(root, 0), phase1GeneratedChildren(root, 1)}}}, nil
}
