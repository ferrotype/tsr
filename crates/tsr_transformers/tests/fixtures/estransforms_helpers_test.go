package estransforms

// Phase 3 (A5) oracle, overlay only: runs the estransforms shared helpers
// (utilities.go, namedevaluation.go, classthis.go) over small parsed inputs and
// prints each result with printer.NewPrinter, as upstream's printer tests do.
// The Rust port (crates/tsr_transformers/src/estransforms/helpers_tests.rs)
// runs the same scenarios over the same inputs and compares the text.
//
// A5_REQUESTS names the requests file ({"cases": [...]}); A5_OUTPUT receives
// the cases with their "expected" text. A panic is recorded as "panic: <value>".
// estransforms_helpers.json is that output; its cases without "expected" are
// the requests. Run from upstream/tsc, with an overlay.json whose "Replace"
// maps internal/transformers/estransforms/phase3_a5_helpers_test.go to this file:
//
//	A5_REQUESTS=requests.json A5_OUTPUT=out.json go test -mod=readonly \
//	    -overlay overlay.json -run TestPhase3A5Helpers ./internal/transformers/estransforms/

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/printer"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/parsetestutil"
	"github.com/microsoft/TypeScript/tsc/internal/transformers"
)

type a5Case struct {
	ID       string `json:"id"`
	Scenario string `json:"scenario"`
	Source   string `json:"source"`
	Name     string `json:"name"`
	Flag     bool   `json:"flag"`
	This     string `json:"this"`
	Mode     string `json:"mode"`
	Expected string `json:"expected"`
}

type a5Run struct {
	ec   *printer.EmitContext
	f    *printer.NodeFactory
	file *ast.SourceFile
	out  []string
}

func (r *a5Run) print(node *ast.Node) string {
	p := printer.NewPrinter(printer.PrinterOptions{NewLine: core.NewLineKindLF}, printer.PrintHandlers{}, r.ec)
	return p.Emit(node, r.file)
}

func (r *a5Run) line(format string, args ...any) {
	r.out = append(r.out, fmt.Sprintf(format, args...))
}

func a5Bool(b bool) string {
	if b {
		return "true"
	}
	return "false"
}

func a5Preorder(node *ast.Node, f func(*ast.Node)) {
	f(node)
	node.ForEachChild(func(child *ast.Node) bool {
		a5Preorder(child, f)
		return false
	})
}

func (r *a5Run) all() []*ast.Node {
	var nodes []*ast.Node
	a5Preorder(r.file.AsNode(), func(n *ast.Node) { nodes = append(nodes, n) })
	return nodes
}

func (r *a5Run) topLevelExpressions() []*ast.Node {
	var expressions []*ast.Node
	for _, statement := range r.file.Statements.Nodes {
		if ast.IsExpressionStatement(statement) {
			expressions = append(expressions, statement.Expression())
		}
	}
	return expressions
}

// prepare marks the parsed static blocks the way the class transforms would
// have produced them (see the Rust twin for the same rules).
func (r *a5Run) prepare() {
	for _, node := range r.all() {
		if !ast.IsClassStaticBlockDeclaration(node) {
			continue
		}
		statements := node.AsClassStaticBlockDeclaration().Body.Statements()
		if len(statements) != 1 || !ast.IsExpressionStatement(statements[0]) {
			continue
		}
		class := node.Parent
		expression := statements[0].Expression()
		if ast.IsCallExpression(expression) && ast.IsIdentifier(expression.Expression()) {
			callee := expression.Expression()
			args := expression.Arguments()
			switch callee.Text() {
			case "__setFunctionName":
				if !(len(args) >= 3 && ast.IsIdentifier(args[2]) && args[2].Text() == "noflag") {
					r.ec.AddEmitFlags(callee, printer.EFHelperName)
				}
				var target *ast.Node
				if len(args) >= 2 {
					target = args[1]
					if ast.IsIdentifier(args[1]) && args[1].Text() == "wrong" {
						target = args[0]
					}
				} else {
					target = r.f.NewStringLiteral("x", ast.TokenFlagsNone)
				}
				r.ec.SetAssignedName(node, target)
				r.ec.SetAssignedName(class, target)
			case "__assigned":
				r.ec.SetAssignedName(class, r.f.NewStringLiteral("x", ast.TokenFlagsNone))
			}
			continue
		}
		if ast.IsBinaryExpression(expression) && ast.IsIdentifier(expression.AsBinaryExpression().Left) {
			left := expression.AsBinaryExpression().Left
			if strings.HasPrefix(left.Text(), "_classThis") {
				r.ec.SetClassThis(node, left)
				r.ec.SetClassThis(class, left)
			} else if strings.HasPrefix(left.Text(), "_other") {
				r.ec.SetClassThis(node, r.f.NewIdentifier("_other"))
			}
		}
	}
}

func isNamedEvaluationKind(node *ast.Node) bool {
	switch node.Kind {
	case ast.KindPropertyAssignment, ast.KindShorthandPropertyAssignment, ast.KindVariableDeclaration,
		ast.KindParameter, ast.KindBindingElement, ast.KindPropertyDeclaration, ast.KindBinaryExpression,
		ast.KindExportAssignment:
		return true
	}
	return false
}

func a5IsClassExpression(node *anonymousFunctionDefinition) bool {
	return ast.IsClassExpression(node)
}

func (r *a5Run) printHoisted() {
	for _, statement := range r.ec.EndVariableEnvironment() {
		r.line("hoisted: %s", r.print(statement))
	}
}

func (r *a5Run) run(c *a5Case) {
	r.prepare()
	switch c.Scenario {
	case "classThisBlock":
		for _, node := range r.all() {
			if ast.IsClassLike(node) {
				var flags []string
				for _, member := range node.Members() {
					flags = append(flags, a5Bool(isClassThisAssignmentBlock(r.ec, member)))
				}
				r.line("%s", strings.Join(flags, ","))
			}
		}
	case "helperBlock":
		for _, node := range r.all() {
			if ast.IsClassLike(node) {
				var flags []string
				for _, member := range node.Members() {
					flags = append(flags, a5Bool(isClassNamedEvaluationHelperBlock(r.ec, member)))
				}
				r.line("members=%s explicit=%s declared=%s", strings.Join(flags, ","),
					a5Bool(classHasExplicitlyAssignedName(r.ec, node)),
					a5Bool(classHasDeclaredOrExplicitlyAssignedName(r.ec, node)))
			}
		}
	case "anonymous":
		for _, expression := range r.topLevelExpressions() {
			r.line("%s %s", a5Bool(isAnonymousFunctionDefinition(r.ec, expression, nil)),
				a5Bool(isAnonymousFunctionDefinition(r.ec, expression, a5IsClassExpression)))
		}
	case "namedEvaluation":
		for _, node := range r.all() {
			if isNamedEvaluationKind(node) {
				r.line("%s %s %s", node.Kind.String(), a5Bool(isNamedEvaluation(r.ec, node)),
					a5Bool(isNamedEvaluationAnd(r.ec, node, a5IsClassExpression)))
			}
		}
	case "transform":
		r.ec.StartVariableEnvironment()
		if c.Mode == "unhandled" {
			r.line("%s", r.print(transformNamedEvaluation(r.ec, r.file.Statements.Nodes[0], c.Flag, c.Name)))
		}
		for _, node := range r.all() {
			if isNamedEvaluationKind(node) && isNamedEvaluation(r.ec, node) {
				r.line("%s", r.print(transformNamedEvaluation(r.ec, node, c.Flag, c.Name)))
			}
		}
		r.printHoisted()
	case "finish":
		for _, expression := range r.topLevelExpressions() {
			name := r.f.NewStringLiteral(c.Name, ast.TokenFlagsNone)
			r.line("%s", r.print(finishTransformNamedEvaluation(r.ec, expression, name, c.Flag)))
		}
	case "convert":
		for _, node := range r.all() {
			if ast.IsClassDeclaration(node) {
				expression := convertClassDeclarationToClassExpression(r.ec, node.AsClassDeclaration())
				r.line("%s", r.print(expression))
				name := getAssignedNameOfIdentifier(r.ec, r.f.NewIdentifier("x"), expression)
				r.line("name: %s", r.print(name))
				r.line("%s", r.print(finishTransformNamedEvaluation(r.ec, expression, name, false)))
			}
			if ast.IsFunctionDeclaration(node) {
				r.line("function name: %s", r.print(getAssignedNameOfIdentifier(r.ec, r.f.NewIdentifier("x"), node)))
			}
		}
	case "propertyName":
		r.ec.StartVariableEnvironment()
		for _, node := range r.all() {
			if ast.IsPropertyAssignment(node) || ast.IsPropertyDeclaration(node) {
				assigned, updated := getAssignedNameOfPropertyName(r.ec, node.Name(), c.Name)
				r.line("%s %s same=%s", r.print(assigned), r.print(updated), a5Bool(updated == node.Name()))
			}
		}
		r.printHoisted()
	case "notNull":
		expressions := r.topLevelExpressions()
		r.line("%s", r.print(createNotNullCondition(r.ec, expressions[0], expressions[1], c.Flag)))
	case "createBlock":
		var this *ast.Node
		if c.This != "" {
			this = r.f.NewIdentifier(c.This)
		}
		block := createClassNamedEvaluationHelperBlock(r.ec, r.f.NewStringLiteral(c.Name, ast.TokenFlagsNone), this)
		r.line("%s", r.print(block))
		r.line("helper=%s", a5Bool(isClassNamedEvaluationHelperBlock(r.ec, block)))
	case "inject":
		for _, node := range r.all() {
			if ast.IsClassLike(node) {
				var this *ast.Node
				if c.This != "" {
					this = r.f.NewIdentifier(c.This)
				}
				name := r.f.NewStringLiteral(c.Name, ast.TokenFlagsNone)
				result := injectClassNamedEvaluationHelperBlockIfMissing(r.ec, node, name, this)
				r.line("%s", r.print(result))
				r.line("same=%s classThis=%s assigned=%s", a5Bool(result == node),
					a5Bool(r.ec.ClassThis(result) != nil), a5Bool(r.ec.AssignedName(result) == name))
			}
		}
	case "super":
		s := &superAccessState{}
		s.initSuperAccessVisitor(r.ec, r.f)
		if c.Mode != "nil" && c.Mode != "nilStatement" {
			s.capturedSuperProperties = &collections.OrderedSet[string]{}
		}
		// Plain identifiers stand in for the transformers' unique names, so
		// the output needs no name generator.
		s.superBinding = r.f.NewIdentifier("_super")
		s.superIndexBinding = r.f.NewIdentifier("_superIndex")
		var body *ast.Node
		for _, node := range r.all() {
			if ast.IsMethodDeclaration(node) {
				body = node.Body()
				break
			}
		}
		a5Preorder(body, s.trackSuperAccess)
		if s.capturedSuperProperties != nil {
			r.line("captured=%s", strings.Join(collectionsValues(s.capturedSuperProperties), ","))
		}
		r.line("element=%s assignment=%s", a5Bool(s.hasSuperElementAccess), a5Bool(s.hasSuperPropertyAssignment))
		if c.Mode == "nilStatement" {
			r.line("%s", r.print(s.createSuperAccessVariableStatement()))
		}
		// The block itself is not printable when it is the unchanged body
		// (its parent makes it a non-statement), so print its statements.
		substituted := s.substituteSuperAccessesInBody(body)
		r.line("same=%s", a5Bool(substituted == body))
		for _, statement := range substituted.Statements() {
			r.line("%s", r.print(statement))
		}
		if s.capturedSuperProperties != nil {
			r.line("%s", r.print(s.createSuperAccessVariableStatement()))
		}
	case "backingField":
		for _, node := range r.all() {
			if ast.IsPropertyDeclaration(node) {
				modifiers := transformers.ExtractModifiers(r.ec, node.Modifiers(), ^ast.ModifierFlagsAccessor)
				r.line("%s", r.print(createAccessorPropertyBackingField(r.f, node.AsPropertyDeclaration(), modifiers, node.Initializer())))
			}
		}
	case "backingFieldParts":
		// The backing field without printing its generated name.
		for _, node := range r.all() {
			if ast.IsPropertyDeclaration(node) {
				modifiers := transformers.ExtractModifiers(r.ec, node.Modifiers(), ^ast.ModifierFlagsAccessor)
				field := createAccessorPropertyBackingField(r.f, node.AsPropertyDeclaration(), modifiers, node.Initializer())
				var parts []string
				for _, modifier := range field.ModifierNodes() {
					parts = append(parts, r.print(modifier))
				}
				initializer := "none"
				if field.Initializer() != nil {
					initializer = r.print(field.Initializer())
				}
				r.line("text=%s generated=%s modifiers=[%s] initializer=%s same=%s", field.Name().Text(),
					a5Bool(r.ec.HasAutoGenerateInfo(field.Name())), strings.Join(parts, " "), initializer, a5Bool(field == node))
			}
		}
	default:
		panic("unknown scenario " + c.Scenario)
	}
}

func collectionsValues(set *collections.OrderedSet[string]) []string {
	var values []string
	for value := range set.Values() {
		values = append(values, value)
	}
	return values
}

func a5Execute(c *a5Case) (result string) {
	r := &a5Run{ec: printer.NewEmitContext()}
	r.f = r.ec.Factory
	r.file = parsetestutil.ParseTypeScript(c.Source, false)
	defer func() {
		if recovered := recover(); recovered != nil {
			r.out = append(r.out, fmt.Sprintf("panic: %v", recovered))
			result = strings.Join(r.out, "\n")
		}
	}()
	r.run(c)
	return strings.Join(r.out, "\n")
}

func TestPhase3A5Helpers(t *testing.T) {
	requests := os.Getenv("A5_REQUESTS")
	if requests == "" {
		t.Skip("A5_REQUESTS not set")
	}
	data, err := os.ReadFile(requests)
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Cases []*a5Case `json:"cases"`
	}
	if err := json.Unmarshal(data, &document); err != nil {
		t.Fatal(err)
	}
	for _, c := range document.Cases {
		c.Expected = a5Execute(c)
	}
	var b strings.Builder
	encoder := json.NewEncoder(&b)
	encoder.SetEscapeHTML(false)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(document); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("A5_OUTPUT"), []byte(b.String()), 0o644); err != nil {
		t.Fatal(err)
	}
}
