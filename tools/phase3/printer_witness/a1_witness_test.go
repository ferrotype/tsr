package printer

// Native witness for the printer's generated names, emit helpers and the
// parentheses parenthesizeExpressionForNoAsi creates. Each case parses a
// source, rewrites marker identifiers into generated names with the emit
// context's node visitor, records helpers and flags, prints the file and
// records the text. Overlaid into the pinned package; not part of it.

import (
	"encoding/json"
	"os"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
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
	original := parsetestutil.ParseTypeScript(c.Source, false /*jsx*/)
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
				return pee
			}
		case ast.KindFunctionDeclaration:
			updated := node.VisitEachChild(visitor)
			if name := node.Name(); name != nil && strings.HasPrefix(name.Text(), "reuse") {
				ec.AddEmitFlags(updated, EFReuseTempVariableScope)
			}
			return updated
		}
		return node.VisitEachChild(visitor)
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
	p := NewPrinter(PrinterOptions{
		NewLine:        core.NewLineKindLF,
		NoEmitHelpers:  c.NoEmitHelpers,
		RemoveComments: c.RemoveComments,
	}, PrintHandlers{}, ec)
	writes := max(c.Writes, 1)
	var output []string
	for range writes {
		output = append(output, p.EmitSourceFile(file))
	}
	return output
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
