package printer

// Native witness for the printer's generated names, emit helpers and the
// parentheses parenthesizeExpressionForNoAsi creates. Each case parses a
// source, rewrites marker identifiers into generated names with the emit
// context's node visitor, records helpers and flags, prints the file and
// records the text. Overlaid into the pinned package; not part of it.
//
// Run from upstream/tsc with an overlay that adds this file to
// internal/printer: A1_CASES=<requests.json> A1_OUTPUT=<results.json>
// go test -overlay <overlay.json> ./internal/printer -run 'TestA1Witness$'.

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/binder"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/parsetestutil"
)

type a1Case struct {
	ID                  string   `json:"id"`
	Source              string   `json:"source"`
	Helpers             []string `json:"helpers"`
	BodyHelpers         []string `json:"body_helpers"`
	NoEmitHelpers       bool     `json:"no_emit_helpers"`
	RemoveComments      bool     `json:"remove_comments"`
	ExternalHelpers     bool     `json:"external_helpers"`
	HelpersModule       bool     `json:"helpers_module"`
	Pee                 string   `json:"pee"`
	Writes              int      `json:"writes"`
	Bind                bool     `json:"bind"`
	Jsx                 bool     `json:"jsx"`
	TabStops            bool     `json:"tab_stops"`
	NeverAsciiEscape    bool     `json:"never_ascii_escape"`
	Listener            bool     `json:"listener"`
}

type a1Result struct {
	ID     string   `json:"id"`
	Output []string `json:"output"`
}

var a1Helpers = map[string]*EmitHelper{
	"decorate":           decorateHelper,
	"metadata":           metadataHelper,
	"param":              paramHelper,
	"awaiter":            awaiterHelper,
	"await":              awaitHelper,
	"rest":               restHelper,
	"asyncSuper":         AsyncSuperHelper,
	"advancedAsyncSuper": AdvancedAsyncSuperHelper,
	"esDecorate":         esDecorateHelper,
	"runInitializers":    runInitializersHelper,
	"propKey":            propKeyHelper,
	"setFunctionName":    setFunctionNameHelper,
	"importStar":         importStarHelper,
	"importDefault":      importDefaultHelper,
	"createBinding":      createBindingHelper,
	"setModuleDefault":   setModuleDefaultHelper,
	"exportStar":         exportStarHelper,
	"makeTemplateObject": makeTemplateObjectHelper,
}

func a1Lookup(names []string) []*EmitHelper {
	var helpers []*EmitHelper
	for _, name := range names {
		helper := a1Helpers[name]
		if helper == nil {
			panic("unknown helper " + name)
		}
		helpers = append(helpers, helper)
	}
	return helpers
}

func a1Run(c a1Case) []string {
	ec := NewEmitContext()
	original := parsetestutil.ParseTypeScript(c.Source, c.Jsx)
	if c.Bind {
		binder.BindSourceFile(original)
	}
	// The most recent partially emitted expression and property access the
	// visitor produced, which `$lastpee` and `$lastpae` share.
	var lastPee, lastPae *ast.Node
	tabStop := 0
	var visitor *ast.NodeVisitor
	visitor = ec.NewNodeVisitor(func(node *ast.Node) *ast.Node {
		switch node.Kind {
		case ast.KindIdentifier:
			text := node.Text()
			switch {
			case text == "$temp":
				return ec.Factory.NewTempVariable()
			case text == "$loop":
				return ec.Factory.NewLoopVariable()
			case text == "$affixed":
				return ec.Factory.NewTempVariableEx(AutoGenerateOptions{Prefix: "pre", Suffix: "suf"})
			case strings.HasPrefix(text, "$unique_"):
				return ec.Factory.NewUniqueName(strings.TrimPrefix(text, "$unique_"))
			case strings.HasPrefix(text, "$optimistic_"):
				return ec.Factory.NewUniqueNameEx(strings.TrimPrefix(text, "$optimistic_"), AutoGenerateOptions{Flags: GeneratedIdentifierFlagsOptimistic})
			case strings.HasPrefix(text, "$filelevel_"):
				return ec.Factory.NewUniqueNameEx(strings.TrimPrefix(text, "$filelevel_"), AutoGenerateOptions{Flags: GeneratedIdentifierFlagsFileLevel | GeneratedIdentifierFlagsOptimistic})
			case strings.HasPrefix(text, "$helper_"):
				return ec.Factory.NewUnscopedHelperName(strings.TrimPrefix(text, "$helper_"))
			case strings.HasPrefix(text, "$node_"):
				return ec.Factory.NewGeneratedNameForNode(node)
			case text == "$strtemp":
				return ec.Factory.NewStringLiteralFromNode(ec.Factory.NewTempVariable())
			case strings.HasPrefix(text, "$strunique_"):
				return ec.Factory.NewStringLiteralFromNode(ec.Factory.NewUniqueName(strings.TrimPrefix(text, "$strunique_")))
			case strings.HasPrefix(text, "$decl_"):
				var index int
				fmt.Sscan(strings.TrimPrefix(text, "$decl_"), &index)
				return ec.Factory.NewGeneratedNameForNode(original.Statements.Nodes[index])
			case strings.HasPrefix(text, "$nlhelper_"):
				name := ec.Factory.NewUnscopedHelperName(strings.TrimPrefix(text, "$nlhelper_"))
				ec.AddEmitFlags(name, EFStartOnNewLine)
				return name
			case strings.HasPrefix(text, "$strtext_"):
				return ec.Factory.NewStringLiteralFromNode(ec.Factory.NewIdentifier(strings.TrimPrefix(text, "$strtext_")))
			case strings.HasPrefix(text, "$strascii_"):
				literal := ec.Factory.NewStringLiteralFromNode(ec.Factory.NewIdentifier(strings.TrimPrefix(text, "$strascii_")))
				ec.AddEmitFlags(literal, EFNoAsciiEscaping)
				return literal
			case text == "$lastpee":
				return lastPee
			case text == "$lastpae":
				return lastPae
			}
			return node
		case ast.KindStringLiteral:
			text := node.Text()
			if strings.HasPrefix(text, "$strjsx_") {
				return ec.Factory.NewStringLiteralFromNode(ec.Factory.NewIdentifier(strings.TrimPrefix(text, "$strjsx_")))
			}
			if strings.HasPrefix(text, "$strasciijsx_") {
				literal := ec.Factory.NewStringLiteralFromNode(ec.Factory.NewIdentifier(strings.TrimPrefix(text, "$strasciijsx_")))
				ec.AddEmitFlags(literal, EFNoAsciiEscaping)
				return literal
			}
			return node
		case ast.KindJsxAttribute:
			if initializer := node.Initializer(); initializer != nil && initializer.Kind == ast.KindStringLiteral && initializer.Text() == "$strns" {
				attribute := node.AsJsxAttribute()
				return ec.Factory.UpdateJsxAttribute(attribute, attribute.Name(), ec.Factory.NewStringLiteralFromNode(attribute.Name()))
			}
		case ast.KindEmptyStatement:
			if c.TabStops {
				ec.SetSnippetElement(node, SnippetElement{Kind: SnippetKindTabStop, Order: tabStop})
				tabStop++
			}
			return node
		case ast.KindPrivateIdentifier:
			text := node.Text()
			if strings.HasPrefix(text, "#unique_") {
				return ec.Factory.NewUniquePrivateName("#" + strings.TrimPrefix(text, "#unique_"))
			}
			return node
		case ast.KindParenthesizedExpression:
			if c.Pee != "" {
				expression := visitor.VisitNode(node.Expression())
				pee := ec.Factory.NewPartiallyEmittedExpression(expression)
				if c.Pee == "original" {
					ec.SetOriginal(pee, node)
				} else {
					ec.AddSyntheticLeadingComment(pee, ast.KindSingleLineCommentTrivia, " c", true)
				}
				pee.Loc = node.Loc
				lastPee = pee
				return pee
			}
		case ast.KindFunctionDeclaration:
			updated := node.VisitEachChild(visitor)
			if name := node.Name(); name != nil && strings.HasPrefix(name.Text(), "reuse") {
				ec.AddEmitFlags(updated, EFReuseTempVariableScope)
			}
			return updated
		}
		visited := node.VisitEachChild(visitor)
		if visited.Kind == ast.KindPropertyAccessExpression {
			lastPae = visited
		}
		return visited
	})
	file := visitor.VisitSourceFile(original)
	if c.ExternalHelpers {
		ec.AddEmitFlags(original.AsNode(), EFExternalHelpers)
	}
	if c.HelpersModule {
		ec.SetExternalHelpersModuleName(original, ec.Factory.NewUniqueName("tslib"))
	}
	if len(c.Helpers) > 0 {
		ec.AddEmitHelper(file.AsNode(), a1Lookup(c.Helpers)...)
	}
	if len(c.BodyHelpers) > 0 {
		for _, statement := range file.Statements.Nodes {
			if statement.Kind == ast.KindFunctionDeclaration {
				ec.AddEmitHelper(statement.Body(), a1Lookup(c.BodyHelpers)...)
				break
			}
		}
	}
	var p *Printer
	before := map[*ast.Node]int{}
	after := map[*ast.Node]int{}
	handlers := PrintHandlers{}
	if c.Listener {
		handlers.OnBeforeEmitNode = func(node *ast.Node) { before[node] = p.writer.GetTextPos() }
		handlers.OnAfterEmitNode = func(node *ast.Node) { after[node] = p.writer.GetTextPos() }
	}
	p = NewPrinter(PrinterOptions{
		NewLine:          core.NewLineKindLF,
		NoEmitHelpers:    c.NoEmitHelpers,
		RemoveComments:   c.RemoveComments,
		NeverAsciiEscape: c.NeverAsciiEscape,
	}, handlers, ec)
	writes := max(c.Writes, 1)
	var output []string
	for range writes {
		output = append(output, p.EmitSourceFile(file))
	}
	if c.Listener {
		output = append(output, a1Positions(file.AsNode(), before, after))
	}
	return output
}

// a1Positions lists, for each node of the printed tree in pre-order, the
// writer positions the emit notifications recorded for it.
func a1Positions(root *ast.Node, before map[*ast.Node]int, after map[*ast.Node]int) string {
	var lines []string
	var walk func(node *ast.Node) bool
	walk = func(node *ast.Node) bool {
		line := strings.TrimPrefix(node.Kind.String(), "Kind")
		if pos, ok := before[node]; ok {
			line += fmt.Sprintf(" %d", pos)
		} else {
			line += " -"
		}
		if end, ok := after[node]; ok {
			line += fmt.Sprintf(" %d", end)
		} else {
			line += " -"
		}
		lines = append(lines, line)
		node.ForEachChild(walk)
		return false
	}
	walk(root)
	return strings.Join(lines, "\n")
}

func TestA1Witness(t *testing.T) {
	requests := os.Getenv("A1_CASES")
	if requests == "" {
		t.Skip("A1_CASES is not set")
	}
	data, err := os.ReadFile(requests)
	if err != nil {
		t.Fatal(err)
	}
	var cases []a1Case
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatal(err)
	}
	var results []a1Result
	for _, c := range cases {
		results = append(results, a1Result{ID: c.ID, Output: a1Run(c)})
	}
	out, err := json.MarshalIndent(results, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("A1_OUTPUT"), out, 0o644); err != nil {
		t.Fatal(err)
	}
}
