package moduletransforms

// Phase 3 module helper oracle (overlay only, never part of the pin). For each
// request it parses and binds /main.ts with the request's options, runs
// collectExternalModuleInfo and the helpers of utilities.go and
// externalmoduleinfo.go over it, and records a deterministic description of
// what they return. The Rust test of the same name rebuilds the description
// from the ported helpers.

import (
	"encoding/json"
	"fmt"
	"os"
	"sort"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/binder"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/printer"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

type phase3Request struct {
	ID                              string   `json:"id"`
	Source                          string   `json:"source"`
	Module                          string   `json:"module"`
	FileModuleKind                  string   `json:"file_module_kind"`
	ImportHelpers                   bool     `json:"import_helpers"`
	RewriteRelativeImportExtensions bool     `json:"rewrite_relative_import_extensions"`
	JsxPreserve                     bool     `json:"jsx_preserve"`
	ModuleDetectionForce            bool     `json:"module_detection_force"`
	Helpers                         []string `json:"helpers"`
	HasExportStars                  bool     `json:"has_export_stars"`
	HasImportStar                   bool     `json:"has_import_star"`
	HasImportDefault                bool     `json:"has_import_default"`
}

var phase3ModuleKinds = map[string]core.ModuleKind{
	"none":     core.ModuleKindNone,
	"commonjs": core.ModuleKindCommonJS,
	"system":   core.ModuleKindSystem,
	"es2015":   core.ModuleKindES2015,
	"esnext":   core.ModuleKindESNext,
	"node16":   core.ModuleKindNode16,
	"preserve": core.ModuleKindPreserve,
}

func phase3Tristate(value bool) core.Tristate {
	if value {
		return core.TSTrue
	}
	return core.TSUnknown
}

type phase3Describer struct {
	emitContext *printer.EmitContext
}

func (d *phase3Describer) node(n *ast.Node) string {
	if n == nil {
		return "nil"
	}
	if info := d.emitContext.GetAutoGenerateInfo(n); info != nil {
		if info.Node != nil {
			return fmt.Sprintf("gen(%d:%s)", int(info.Flags), d.node(info.Node))
		}
		return fmt.Sprintf("gen(%d:%q)", int(info.Flags), n.Text())
	}
	text := ""
	switch n.Kind {
	case ast.KindIdentifier, ast.KindStringLiteral, ast.KindNoSubstitutionTemplateLiteral, ast.KindNumericLiteral:
		text = fmt.Sprintf(":%q", n.Text())
	}
	return fmt.Sprintf("%s@%d%s", n.Kind.String(), n.Pos(), text)
}

func (d *phase3Describer) nodes(nodes []*ast.Node) string {
	parts := make([]string, 0, len(nodes))
	for _, n := range nodes {
		parts = append(parts, d.node(n))
	}
	return "[" + strings.Join(parts, " ") + "]"
}

func (d *phase3Describer) synthesized(n *ast.Node) string {
	if n == nil {
		return "nil"
	}
	var b strings.Builder
	fmt.Fprintf(&b, "%s{flags=%d", n.Kind.String(), int(d.emitContext.EmitFlags(n)))
	if d.emitContext.HasAutoGenerateInfo(n) {
		b.WriteString(" " + d.node(n))
	} else {
		switch n.Kind {
		case ast.KindIdentifier, ast.KindStringLiteral:
			fmt.Fprintf(&b, " %q", n.Text())
		}
	}
	n.ForEachChild(func(child *ast.Node) bool {
		b.WriteString(" ")
		b.WriteString(d.synthesized(child))
		return false
	})
	b.WriteString("}")
	return b.String()
}

func phase3Describe(t *testing.T, request phase3Request) string {
	options := &core.CompilerOptions{
		Module:                          phase3ModuleKinds[request.Module],
		ImportHelpers:                   phase3Tristate(request.ImportHelpers),
		RewriteRelativeImportExtensions: phase3Tristate(request.RewriteRelativeImportExtensions),
	}
	if request.JsxPreserve {
		options.Jsx = core.JsxEmitPreserve
	}
	if request.ModuleDetectionForce {
		options.ModuleDetection = core.ModuleDetectionKindForce
	}
	fileName := "/main.ts"
	file := parser.ParseSourceFile(ast.SourceFileParseOptions{
		FileName:                       fileName,
		Path:                           tspath.Path(fileName),
		ExternalModuleIndicatorOptions: ast.GetExternalModuleIndicatorOptions(fileName, options, ast.SourceFileMetaData{}),
	}, request.Source, core.ScriptKindTS)
	binder.BindSourceFile(file)
	resolver := binder.NewReferenceResolver(options, binder.ReferenceResolverHooks{})
	emitContext := printer.NewEmitContext()
	factory := emitContext.Factory
	d := &phase3Describer{emitContext: emitContext}
	var out strings.Builder
	line := func(format string, args ...any) {
		fmt.Fprintf(&out, format, args...)
		out.WriteString("\n")
	}

	// collectExternalModuleInfo
	info := collectExternalModuleInfo(file, options, emitContext, resolver)
	line("externalImports %s", d.nodes(info.externalImports))
	var specifierKeys []string
	for key := range info.exportSpecifiers.M {
		specifierKeys = append(specifierKeys, key)
	}
	sort.Strings(specifierKeys)
	for _, key := range specifierKeys {
		var specifiers []*ast.Node
		for _, specifier := range info.exportSpecifiers.Get(key) {
			specifiers = append(specifiers, specifier.AsNode())
		}
		line("exportSpecifiers %q %s", key, d.nodes(specifiers))
	}
	var bindingKeys []string
	bindings := map[string][]*ast.Node{}
	for key, values := range info.exportedBindings.M {
		described := d.node(key)
		bindingKeys = append(bindingKeys, described)
		bindings[described] = values
	}
	sort.Strings(bindingKeys)
	for _, key := range bindingKeys {
		line("exportedBindings %s %s", key, d.nodes(bindings[key]))
	}
	line("exportedNames %s", d.nodes(info.exportedNames))
	var functions []*ast.Node
	for function := range info.exportedFunctions.Values() {
		functions = append(functions, function)
	}
	line("exportedFunctions %s", d.nodes(functions))
	if info.exportEquals == nil {
		line("exportEquals nil")
	} else {
		line("exportEquals %s", d.node(info.exportEquals.AsNode()))
	}
	line("hasExportStarsToExportValues %t", info.hasExportStarsToExportValues)

	// Per statement helpers.
	for _, statement := range file.Statements.Nodes {
		switch statement.Kind {
		case ast.KindImportDeclaration:
			n := statement.AsImportDeclaration()
			line("import %s star=%t default=%t", d.node(statement), getImportNeedsImportStarHelper(n), getImportNeedsImportDefaultHelper(n))
		case ast.KindExportDeclaration:
			n := statement.AsExportDeclaration()
			line("export %s star=%t containsDefault=%t", d.node(statement), getExportNeedsImportStarHelper(n), containsDefaultReference(n.ExportClause))
		}
		switch statement.Kind {
		case ast.KindImportDeclaration, ast.KindExportDeclaration, ast.KindImportEqualsDeclaration:
			literal := getExternalModuleNameLiteral(factory, statement, file, nil, nil, options)
			line("moduleNameLiteral %s %s", d.node(statement), d.synthesized(literal))
			specifier := ast.GetExternalModuleName(statement)
			rewritten := rewriteModuleSpecifier(emitContext, specifier, options)
			line("rewriteModuleSpecifier %s %s same=%t", d.node(specifier), d.synthesized(rewritten), rewritten == specifier)
		case ast.KindExpressionStatement:
			expression := statement.Expression()
			line("simpleInlineable %s %t", d.node(expression), isSimpleInlineableExpression(expression))
		}
	}

	// Every identifier: the declaration name of an enum or namespace?
	var names []*ast.Node
	var walk func(n *ast.Node) bool
	walk = func(n *ast.Node) bool {
		if n.Kind == ast.KindIdentifier && isDeclarationNameOfEnumOrNamespace(emitContext, n) {
			names = append(names, n)
		}
		n.ForEachChild(walk)
		return false
	}
	file.AsNode().ForEachChild(walk)
	line("enumOrNamespaceNames %s", d.nodes(names))

	// Generated names.
	var reserved []string
	for _, flags := range []printer.GeneratedIdentifierFlags{
		0,
		printer.GeneratedIdentifierFlagsFileLevel,
		printer.GeneratedIdentifierFlagsFileLevel | printer.GeneratedIdentifierFlagsOptimistic,
		printer.GeneratedIdentifierFlagsFileLevel | printer.GeneratedIdentifierFlagsReservedInNestedScopes,
		printer.GeneratedIdentifierFlagsOptimistic | printer.GeneratedIdentifierFlagsReservedInNestedScopes,
		printer.GeneratedIdentifierFlagsFileLevel | printer.GeneratedIdentifierFlagsOptimistic | printer.GeneratedIdentifierFlagsReservedInNestedScopes,
	} {
		name := factory.NewUniqueNameEx("n", printer.AutoGenerateOptions{Flags: flags})
		reserved = append(reserved, fmt.Sprint(isFileLevelReservedGeneratedIdentifier(emitContext, name)))
	}
	reserved = append(reserved, fmt.Sprint(isFileLevelReservedGeneratedIdentifier(emitContext, factory.NewIdentifier("plain"))))
	line("fileLevelReserved [%s]", strings.Join(reserved, " "))
	line("emptyImports %s", d.synthesized(createEmptyImports(factory)))
	line("externalModuleNameFromPath %q", getExternalModuleNameFromPath(nil, "/a.ts", "/b.ts"))

	// The tslib import.
	for _, helper := range request.Helpers {
		expression := factory.NewIdentifier("e")
		switch helper {
		case "importStar":
			factory.NewImportStarHelper(expression)
		case "importDefault":
			factory.NewImportDefaultHelper(expression)
		case "exportStar":
			factory.NewExportStarHelper(expression, factory.NewIdentifier("exports"))
		case "await":
			factory.NewAwaitHelper(expression)
		case "asyncSuper":
			emitContext.AddEmitHelper(file.AsNode(), printer.AsyncSuperHelper)
			continue
		default:
			t.Fatalf("unknown helper %s", helper)
		}
		emitContext.AddEmitHelper(file.AsNode(), emitContext.ReadEmitHelpers()...)
	}
	var imported []string
	for _, helper := range getImportedHelpers(emitContext, file) {
		imported = append(imported, helper.Name)
	}
	line("importedHelpers [%s]", strings.Join(imported, " "))
	declaration := createExternalHelpersImportDeclarationIfNeeded(emitContext, file, options, phase3ModuleKinds[request.FileModuleKind], request.HasExportStars, request.HasImportStar, request.HasImportDefault)
	line("externalHelpersImport %s", d.synthesized(declaration))
	line("sourceFileFlags %d", int(emitContext.EmitFlags(file.AsNode())))
	again := getOrCreateExternalHelpersModuleNameIfNeeded(emitContext, file, options, nil, false, false, phase3ModuleKinds[request.FileModuleKind])
	line("externalHelpersModuleName %s", d.synthesized(again))
	return out.String()
}

func TestPhase3ModuleInfo(t *testing.T) {
	requestsPath := os.Getenv("PHASE3_MODULEINFO_REQUESTS")
	outputPath := os.Getenv("PHASE3_MODULEINFO_OUTPUT")
	if requestsPath == "" || outputPath == "" {
		t.Skip("PHASE3_MODULEINFO_REQUESTS and PHASE3_MODULEINFO_OUTPUT are not set")
	}
	data, err := os.ReadFile(requestsPath)
	if err != nil {
		t.Fatal(err)
	}
	var requests []phase3Request
	if err := json.Unmarshal(data, &requests); err != nil {
		t.Fatal(err)
	}
	type row struct {
		ID          string `json:"id"`
		Description string `json:"description,omitempty"`
		Panic       string `json:"panic,omitempty"`
	}
	var rows []row
	for _, request := range requests {
		func() {
			defer func() {
				if recovered := recover(); recovered != nil {
					rows = append(rows, row{ID: request.ID, Panic: fmt.Sprint(recovered)})
				}
			}()
			rows = append(rows, row{ID: request.ID, Description: phase3Describe(t, request)})
		}()
	}
	output, err := json.MarshalIndent(rows, "", " ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(outputPath, append(output, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
