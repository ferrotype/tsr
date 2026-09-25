package binder

import (
	"bytes"
	"fmt"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"os"
	"strings"
	"testing"
)

// Resolver fixtures deliberately supply small symbol graphs. They exercise the
// original resolver and parser, independently of either binder implementation.
func TestS07Resolvers(t *testing.T) {
	var output bytes.Buffer
	emit := func(label string, values ...any) {
		fmt.Fprint(&output, label)
		for _, v := range values {
			fmt.Fprintf(&output, "\t%v", v)
		}
		fmt.Fprintln(&output)
	}
	parse := func(text string, js bool) *ast.Node {
		kind := core.ScriptKindTS
		name := "/resolver.ts"
		if js {
			kind = core.ScriptKindJS
			name = "/resolver.js"
		}
		return parser.ParseSourceFile(ast.SourceFileParseOptions{FileName: name}, text, kind).AsNode()
	}
	find := func(root *ast.Node, kind ast.Kind, nth int) *ast.Node {
		var found *ast.Node
		var visit func(*ast.Node) bool
		visit = func(n *ast.Node) bool {
			if n.Kind == kind {
				if nth == 0 {
					found = n
					return true
				}
				nth--
			}
			return n.ForEachChild(visit)
		}
		visit(root)
		if found == nil {
			panic(fmt.Sprintf("missing kind %d", kind))
		}
		return found
	}
	sym := func(name string, flags ast.SymbolFlags, declarations ...*ast.Node) *ast.Symbol {
		s := &ast.Symbol{Name: name, Flags: flags, Declarations: declarations}
		if len(declarations) > 0 {
			s.ValueDeclaration = declarations[0]
		}
		return s
	}
	options := &core.CompilerOptions{Target: core.ScriptTargetES2022}
	message := diagnostics.Cannot_find_name_0
	for i, text := range []string{"function f(p=x?.y) {}", "function f(p=x??y) {}", "function f({...p}) {}", "function f(p=class {static x=1}) {}", "function f(p=()=>x?.y) {}", "function f(p=class {x=x?.y}) {}", "function f(p={ [x?.y]: 0}) {}", "function f(p={ m() {x?.y} }) {}", "function f(p: typeof x) {}"} {
		root := parse(text, false)
		parameter := find(root, ast.KindParameter, 0)
		for _, target := range []core.ScriptTarget{core.ScriptTargetES2016, core.ScriptTargetES2017, core.ScriptTargetES2020, core.ScriptTargetES2022} {
			r := NameResolver{CompilerOptions: &core.CompilerOptions{Target: target}}
			emit(fmt.Sprintf("scope/%d/%d", i, target), r.requiresScopeChange(parameter))
		}
	}
	for i, text := range []string{"const v=()=>x;", "const v=(()=>x)();", "const v=(async()=>x)();", "const v=(function*(){yield x})();", "function f(){x}", "class C {p=x}", "class C {static p=x}", "type T=typeof x;"} {
		root := parse(text, false)
		var selected *ast.Node
		for _, kind := range []ast.Kind{ast.KindArrowFunction, ast.KindFunctionExpression, ast.KindFunctionDeclaration, ast.KindPropertyDeclaration, ast.KindTypeQuery} {
			var walk func(*ast.Node) bool
			walk = func(n *ast.Node) bool {
				if n.Kind == kind {
					selected = n
					return true
				}
				return n.ForEachChild(walk)
			}
			walk(root)
			if selected != nil {
				break
			}
		}
		emit(fmt.Sprintf("deferred/%d", i), getIsDeferredContext(selected, nil), getIsDeferredContext(selected, selected.Name()))
	}
	for mode := 0; mode < 4; mode++ {
		var trace []string
		s := sym("g", ast.SymbolFlagsFunctionScopedVariable)
		r := NameResolver{CompilerOptions: options, Globals: ast.SymbolTable{"g": s}}
		r.SymbolReferenced = func(*ast.Symbol, ast.SymbolFlags) { trace = append(trace, "referenced") }
		r.OnFailedToResolveSymbol = func(*ast.Node, string, ast.SymbolFlags, *diagnostics.Message) { trace = append(trace, "failed") }
		r.OnSuccessfullyResolvedSymbol = func(*ast.Node, *ast.Symbol, ast.SymbolFlags, *ast.Node, *ast.Node, bool) {
			trace = append(trace, "success")
		}
		if mode > 0 {
			r.Lookup = func(table ast.SymbolTable, name string, meaning ast.SymbolFlags) *ast.Symbol {
				trace = append(trace, fmt.Sprintf("lookup:%d:%t", meaning, table == nil))
				if mode == 1 {
					return nil
				}
				return s
			}
		}
		r.Globals = ast.SymbolTable{"g": s}
		if mode == 3 {
			r.Globals = nil
		}
		got := r.Resolve(nil, "g", 0, message, true, false)
		emit(fmt.Sprintf("global/%d", mode), got != nil, strings.Join(trace, ","))
	}
	root := parse("function f(){arguments;}", false)
	function := find(root, ast.KindFunctionDeclaration, 0)
	r := NameResolver{CompilerOptions: options}
	a := r.Resolve(function, "arguments", ast.SymbolFlagsVariable, nil, false, true)
	b := r.Resolve(function, "arguments", ast.SymbolFlagsVariable, nil, false, true)
	emit("arguments", a == b, a.Flags, a.Name)
	root = parse("require(dynamic)", true)
	reference := find(root, ast.KindIdentifier, 0)
	var failed bool
	r = NameResolver{CompilerOptions: options, OnFailedToResolveSymbol: func(*ast.Node, string, ast.SymbolFlags, *diagnostics.Message) { failed = true }}
	emit("require/nil", r.Resolve(reference, "require", ast.SymbolFlagsValue, message, true, true) == nil, failed)
	r.RequireSymbol = sym("require", ast.SymbolFlagsFunction)
	emit("require/symbol", r.Resolve(reference, "require", ast.SymbolFlagsValue, message, true, true) == r.RequireSymbol, failed)
	root = parse("0 as const", false)
	reference = find(root, ast.KindIdentifier, 0)
	failed = false
	r = NameResolver{CompilerOptions: options, OnFailedToResolveSymbol: func(*ast.Node, string, ast.SymbolFlags, *diagnostics.Message) { failed = true }}
	emit("const", r.Resolve(reference, "const", ast.SymbolFlagsType, message, false, false) == nil, failed)
	root = parse("export default function local() {}", false)
	function = find(root, ast.KindFunctionDeclaration, 0)
	local := sym("local", ast.SymbolFlagsFunction, function)
	exported := sym("default", ast.SymbolFlagsFunction, function)
	function.ExportableData().LocalSymbol = local
	emit("default", GetLocalSymbolForExportDefault(exported) == local, GetLocalSymbolForExportDefault(nil) == nil)
	root = parse("class C<T> {static p:T}", false)
	class := find(root, ast.KindClassDeclaration, 0)
	parameter := find(root, ast.KindTypeParameter, 0)
	reference = find(root, ast.KindTypeReference, 0)
	tp := sym("T", ast.SymbolFlagsTypeParameter, parameter)
	cs := sym("C", ast.SymbolFlagsClass, class)
	cs.Members = ast.SymbolTable{"T": tp}
	class.DeclarationData().Symbol = cs
	var codes []string
	r = NameResolver{CompilerOptions: options, Error: func(_ *ast.Node, m *diagnostics.Message, _ ...any) *ast.Diagnostic {
		codes = append(codes, fmt.Sprint(m.Code()))
		return nil
	}}
	emit("static/type", r.Resolve(reference, "T", ast.SymbolFlagsType, message, false, true) == nil, strings.Join(codes, ","))
	emit("type/container", isTypeParameterSymbolDeclaredInContainer(tp, class), isTypeParameterSymbolDeclaredInContainer(tp, root))
	root = parse("function f(p=x){var x;}", false)
	function = find(root, ast.KindFunctionDeclaration, 0)
	parameter = find(root, ast.KindParameter, 0)
	value := find(root, ast.KindVariableDeclaration, 0)
	vs := sym("x", ast.SymbolFlagsFunctionScopedVariable, value)
	for _, state := range []core.Tristate{core.TSUnknown, core.TSTrue, core.TSFalse} {
		var trace []string
		r = NameResolver{CompilerOptions: options, GetRequiresScopeChangeCache: func(*ast.Node) core.Tristate { trace = append(trace, "get"); return state }, SetRequiresScopeChangeCache: func(_ *ast.Node, v core.Tristate) { trace = append(trace, fmt.Sprintf("set:%d", v)) }}
		emit(fmt.Sprintf("parameter/cache/%d", state), r.useOuterVariableScopeInParameter(vs, function, parameter), strings.Join(trace, ","))
	}
	for i, text := range []string{"import {A} from 'm'; A;", "import {type A} from 'm'; A;", "import type {A} from 'm'; A;", "import A = require('m'); A;", "import type A = require('m'); A;"} {
		root = parse(text, false)
		var declaration *ast.Node
		if i < 3 {
			declaration = find(root, ast.KindImportSpecifier, 0)
		} else {
			declaration = find(root, ast.KindImportEqualsDeclaration, 0)
		}
		alias := sym("A", ast.SymbolFlagsAlias, declaration)
		reference = declaration.Name()
		rr := NewReferenceResolver(options, ReferenceResolverHooks{GetResolvedSymbol: func(*ast.Node) *ast.Symbol { return alias }})
		emit(fmt.Sprintf("import/%d", i), rr.GetReferencedImportDeclaration(reference) == declaration)
	}
	root = parse("let v=1; function f(){} type T=number;", false)
	value = find(root, ast.KindVariableDeclaration, 0)
	function = find(root, ast.KindFunctionDeclaration, 0)
	alias := find(root, ast.KindTypeAliasDeclaration, 0)
	reference = value.Name()
	s := sym("v", ast.SymbolFlagsFunctionScopedVariable, value, alias, function)
	for mode := 0; mode < 4; mode++ {
		var trace []string
		hooks := ReferenceResolverHooks{GetResolvedSymbol: func(*ast.Node) *ast.Symbol {
			trace = append(trace, "resolved")
			if mode == 0 {
				return s
			}
			return nil
		}, ResolveName: func(*ast.Node, string, ast.SymbolFlags, *diagnostics.Message, bool, bool) *ast.Symbol {
			trace = append(trace, "resolve")
			if mode == 2 {
				return nil
			}
			return s
		}, GetMergedSymbol: func(s *ast.Symbol) *ast.Symbol { trace = append(trace, "merged"); return s }}
		rr := NewReferenceResolver(options, hooks)
		got := rr.GetReferencedValueDeclarations(reference)
		emit(fmt.Sprintf("reference/%d", mode), len(got), got == nil, strings.Join(trace, ","))
	}
	element := find(parse("this[x]", false), ast.KindElementAccessExpression, 0).AsElementAccessExpression()
	for mode := 0; mode < 3; mode++ {
		hooks := ReferenceResolverHooks{}
		if mode > 0 {
			hooks.GetElementAccessExpressionName = func(*ast.ElementAccessExpression) (string, bool) { return "member", mode == 2 }
		}
		rr := NewReferenceResolver(options, hooks)
		emit(fmt.Sprintf("element/%d", mode), rr.GetElementAccessExpressionName(element), rr.GetElementAccessExpressionName(nil))
	}
	// Local callback order and self-reference suppression are observable checker hooks.
	for i, text := range []string{"export {}; function f(){f;}", "export {}; let x; function f(p=x){x;}"} {
		root = parse(text, false)
		function = find(root, ast.KindFunctionDeclaration, 0)
		fs := sym("f", ast.SymbolFlagsFunction, function)
		function.DeclarationData().Symbol = fs
		name := "f"
		s = fs
		if i == 1 {
			name = "x"
			s = sym("x", ast.SymbolFlagsBlockScopedVariable, find(root, ast.KindVariableDeclaration, 0))
		}
		root.LocalsContainerData().Locals = ast.SymbolTable{name: s}
		reference = find(root, ast.KindExpressionStatement, 0).Expression()
		var trace []string
		r = NameResolver{CompilerOptions: options, SymbolReferenced: func(*ast.Symbol, ast.SymbolFlags) { trace = append(trace, "referenced") }, OnSuccessfullyResolvedSymbol: func(_ *ast.Node, _ *ast.Symbol, _ ast.SymbolFlags, _ *ast.Node, associated *ast.Node, deferred bool) {
			trace = append(trace, fmt.Sprintf("success:%t:%t", associated != nil, deferred))
		}}
		emit(fmt.Sprintf("lexical/%d", i), r.Resolve(reference, name, ast.SymbolFlagsValue, message, true, true) == s, strings.Join(trace, ","))
	}
	// Absence and returned nil must not collapse at optional reference hooks.
	root = parse("namespace N { export function f(){} f; }", false)
	module := find(root, ast.KindModuleDeclaration, 0)
	function = find(root, ast.KindFunctionDeclaration, 0)
	reference = find(root, ast.KindExpressionStatement, 0).Expression()
	moduleSymbol := sym("N", ast.SymbolFlagsValueModule, module)
	functionSymbol := sym("f", ast.SymbolFlagsFunction, function)
	functionSymbol.Parent = moduleSymbol
	localSymbol := sym("f", ast.SymbolFlagsExportValue, function)
	localSymbol.ExportSymbol = functionSymbol
	module.DeclarationData().Symbol = moduleSymbol
	function.DeclarationData().Symbol = localSymbol
	panics := func(callback func()) (result string) {
		defer func() {
			if reason := recover(); reason != nil {
				result = fmt.Sprint(reason)
			}
		}()
		callback()
		return
	}
	for mode := 0; mode < 6; mode++ {
		hooks := ReferenceResolverHooks{GetResolvedSymbol: func(*ast.Node) *ast.Symbol { return localSymbol }}
		if mode == 2 {
			hooks.GetParentOfSymbol = func(*ast.Symbol) *ast.Symbol { return nil }
		}
		if mode == 3 {
			hooks.GetSymbolOfDeclaration = func(*ast.Node) *ast.Symbol { return nil }
		}
		if mode >= 4 {
			hooks.GetMergedSymbol = func(*ast.Symbol) *ast.Symbol { return nil }
		}
		rr := NewReferenceResolver(options, hooks)
		var got *ast.Node
		panic := panics(func() { got = rr.GetReferencedExportContainer(reference, mode != 0 && mode != 5) })
		emit(fmt.Sprintf("export/container/%d", mode), got == module, panic)
	}
	for mode := 0; mode < 3; mode++ {
		hooks := ReferenceResolverHooks{}
		if mode > 0 {
			hooks.GetResolvedSymbol = func(*ast.Node) *ast.Symbol { return localSymbol }
		}
		if mode == 2 {
			hooks.GetExportSymbolOfValueSymbolIfExported = func(*ast.Symbol) *ast.Symbol { return nil }
		}
		rr := NewReferenceResolver(options, hooks)
		var got *ast.Node
		panic := panics(func() { got = rr.GetReferencedMemberValueDeclaration(function) })
		emit(fmt.Sprintf("member/value/%d", mode), got == function, panic)
	}
	root = parse("import type {A} from 'm'; A;", false)
	declaration := find(root, ast.KindImportSpecifier, 0)
	reference = declaration.Name()
	aliasSymbol := sym("A", ast.SymbolFlagsAlias, declaration)
	rr := NewReferenceResolver(options, ReferenceResolverHooks{GetResolvedSymbol: func(*ast.Node) *ast.Symbol { return aliasSymbol }, GetTypeOnlyAliasDeclaration: func(*ast.Symbol, ast.SymbolFlags) *ast.Node { return nil }})
	emit("import/hook-nil", rr.GetReferencedImportDeclaration(reference) == declaration)
	// Fallback NameResolver is lazy and resolves against existing file binding state.
	root = parse("export {}; let x; x;", false)
	value = find(root, ast.KindVariableDeclaration, 0)
	reference = find(root, ast.KindExpressionStatement, 0).Expression()
	s = sym("x", ast.SymbolFlagsBlockScopedVariable, value)
	root.LocalsContainerData().Locals = ast.SymbolTable{"x": s}
	rr = NewReferenceResolver(options, ReferenceResolverHooks{})
	emit("reference/fallback", rr.GetReferencedValueDeclaration(reference) == value)

	root = parse("class C extends Base {}", false)
	detached := find(root, ast.KindExpressionWithTypeArguments, 0)
	detached.Parent = nil
	r = NameResolver{CompilerOptions: options}
	var detachedResult *ast.Symbol
	detachedPanic := panics(func() { detachedResult = r.Resolve(detached, "x", ast.SymbolFlagsValue, nil, false, true) })
	emit("heritage/detached", detachedResult == nil, detachedPanic)

	// A module export that is purely an alias is not in scope (nameresolver.go
	// 131-144): GetDeclarationOfKind finds an ExportSpecifier or NamespaceExport
	// declaration, also when it is not the first; any other alias declaration
	// leaves the export resolvable.
	for i, text := range []string{"let x; export { x as y }; y;", "export * as y from 'm'; y;", "export import y = require('m'); y;", "export import w = require('m'); let x; export { x as y }; y;"} {
		root = parse(text, false)
		var declarations []*ast.Node
		switch i {
		case 0:
			declarations = []*ast.Node{find(root, ast.KindExportSpecifier, 0)}
		case 1:
			declarations = []*ast.Node{find(root, ast.KindNamespaceExport, 0)}
		case 2:
			declarations = []*ast.Node{find(root, ast.KindImportEqualsDeclaration, 0)}
		default:
			declarations = []*ast.Node{find(root, ast.KindImportEqualsDeclaration, 0), find(root, ast.KindExportSpecifier, 0)}
		}
		alias := sym("y", ast.SymbolFlagsAlias, declarations...)
		module := sym("\"/resolver\"", ast.SymbolFlagsValueModule, root)
		module.Exports = ast.SymbolTable{"y": alias}
		root.DeclarationData().Symbol = module
		reference = find(root, ast.KindExpressionStatement, 0).Expression()
		r = NameResolver{CompilerOptions: options}
		got := r.Resolve(reference, "y", ast.SymbolFlagsAlias, nil, false, true)
		emit(fmt.Sprintf("alias/%d", i), got == nil, got == alias)
	}
	// The locals of a global script are not in scope (IsGlobalSourceFile).
	root = parse("let x; x;", false)
	reference = find(root, ast.KindExpressionStatement, 0).Expression()
	s = sym("x", ast.SymbolFlagsBlockScopedVariable, find(root, ast.KindVariableDeclaration, 0))
	root.LocalsContainerData().Locals = ast.SymbolTable{"x": s}
	r = NameResolver{CompilerOptions: options}
	emit("script/locals", r.Resolve(reference, "x", ast.SymbolFlagsValue, nil, false, true) == nil)
	// Export-default local symbols: a function declaration without a local
	// (ExportableBase.ExportableData with a nil LocalSymbol), then a payload that
	// has no exportable data at all (NodeDefault.ExportableData), then a class
	// with a local.
	root = parse("export default function() {} export default 0; export default class C {}", false)
	function = find(root, ast.KindFunctionDeclaration, 0)
	assignment := find(root, ast.KindExportAssignment, 0)
	class = find(root, ast.KindClassDeclaration, 0)
	classLocal := sym("C", ast.SymbolFlagsClass, class)
	class.ExportableData().LocalSymbol = classLocal
	emit("default/nil", GetLocalSymbolForExportDefault(sym("default", ast.SymbolFlagsFunction, function, assignment)) == nil)
	emit("default/scan", GetLocalSymbolForExportDefault(sym("default", ast.SymbolFlagsFunction, function, assignment, class)) == classLocal)
	// GetReferencedImportDeclaration: an alias merged with a local value is not a
	// non-local alias (IsNonLocalAlias); otherwise the declaration is the last
	// IsAliasSymbolDeclaration match, not the last declaration.
	root = parse("import {A} from 'm'; let v = 1; A;", false)
	declaration = find(root, ast.KindImportSpecifier, 0)
	value = find(root, ast.KindVariableDeclaration, 0)
	reference = declaration.Name()
	for i, flags := range []ast.SymbolFlags{ast.SymbolFlagsAlias | ast.SymbolFlagsFunctionScopedVariable, ast.SymbolFlagsAlias} {
		merged := sym("A", flags, declaration, value)
		rr := NewReferenceResolver(options, ReferenceResolverHooks{GetResolvedSymbol: func(*ast.Node) *ast.Symbol { return merged }})
		emit(fmt.Sprintf("import/alias/%d", i), rr.GetReferencedImportDeclaration(reference) == declaration)
	}
	// BindSourceFile binds a file once under BindOnce and marks it bound.
	root = parse("export {}; let x;", false)
	file := root.AsSourceFile()
	before := file.IsBound()
	BindSourceFile(file)
	first := root.Symbol()
	BindSourceFile(file)
	emit("bind/state", before, file.IsBound(), first != nil, len(root.Locals()), root.Symbol() == first, file.SymbolCount)
	// BindSourceFile's IsBound guard keeps the second call above away from
	// BindOnce, so enter it directly: on a bound file neither a new callback
	// nor bindSourceFile runs again, and the module symbol is kept.
	ran := false
	file.BindOnce(func() { ran = true })
	bindSourceFile(file)
	emit("bind/once", ran, root.Symbol() == first)
	if err := os.WriteFile(os.Getenv("S07_RESOLVER_OUTPUT"), output.Bytes(), 0600); err != nil {
		t.Fatal(err)
	}
}
