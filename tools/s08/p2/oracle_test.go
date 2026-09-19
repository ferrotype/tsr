package checker_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"runtime"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnosticwriter"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/baseline"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/harnessutil"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/tsbaseline"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

type p2Query struct {
	ID          string `json:"id"`
	File        string `json:"file"`
	Declaration string `json:"declaration"`
	Target      string `json:"target"`
	Operation   string `json:"operation"`
}
type p2Program struct {
	ID      string            `json:"id"`
	Files   map[string]string `json:"files"`
	Roots   []string          `json:"roots"`
	Queries []p2Query         `json:"queries"`
}
type p2Merge struct {
	ID               string              `json:"id"`
	Files            map[string]string   `json:"files"`
	Shared           string              `json:"shared"`
	Symbol           string              `json:"symbol"`
	Modes            []string            `json:"modes"`
	SingleRoots      []string            `json:"single_roots"`
	IndependentRoots map[string][]string `json:"independent_roots"`
}
type p2Spec struct {
	Version int    `json:"version"`
	Scope   string `json:"scope"`
    DiagnosticMode string `json:"diagnostic_mode"`
	Options struct {
		Target string `json:"target"`
		Module string `json:"module"`
		Strict bool   `json:"strict"`
		NoLib  bool   `json:"noLib"`
		SkipLibCheck bool `json:"skipLibCheck"`
        NoUncheckedIndexedAccess bool `json:"noUncheckedIndexedAccess"`
        IsolatedModules bool `json:"isolatedModules"`
        VerbatimModuleSyntax bool `json:"verbatimModuleSyntax"`
        AllowUnreachableCode *bool `json:"allowUnreachableCode"`
        NoImplicitOverride bool `json:"noImplicitOverride"`
        AllowSyntheticDefaultImports *bool `json:"allowSyntheticDefaultImports"`
        EsModuleInterop *bool `json:"esModuleInterop"`
        ResolveJsonModule *bool `json:"resolveJsonModule"`
        Declaration bool `json:"declaration"`
        IsolatedDeclarations bool `json:"isolatedDeclarations"`
        StripInternal bool `json:"stripInternal"`
        UseDefineForClassFields *bool `json:"useDefineForClassFields"`
        ImportHelpers *bool `json:"importHelpers"`
        NoEmit *bool `json:"noEmit"`
        AllowJs bool `json:"allowJs"`
        CheckJs bool `json:"checkJs"`
	} `json:"options"`
	Programs []p2Program `json:"programs"`
	Merge    p2Merge     `json:"merge"`
}

// The host caches original parsed SourceFiles only. Each real Program and
// Checker retains its own source list, globals, merges and semantic caches.
type p2SharedHost struct {
	compiler.CompilerHost
	mu    sync.Mutex
	files map[ast.SourceFileParseOptions]*ast.SourceFile
}

func (h *p2SharedHost) GetSourceFile(opts ast.SourceFileParseOptions) *ast.SourceFile {
	h.mu.Lock()
	defer h.mu.Unlock()
	if f := h.files[opts]; f != nil {
		return f
	}
	f := h.CompilerHost.GetSourceFile(opts)
	h.files[opts] = f
	return f
}
func p2Host(files map[string]string) *p2SharedHost {
	return &p2SharedHost{CompilerHost: compiler.NewCompilerHost("/", vfstest.FromMap(files, true), "/no-default-lib", nil, nil, nil), files: map[ast.SourceFileParseOptions]*ast.SourceFile{}}
}
func p2NewProgram(host compiler.CompilerHost, roots []string) *compiler.Program {
	options := &core.CompilerOptions{Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext, Strict: core.TSTrue, NoLib: core.TSTrue}
	config := tsoptions.NewParsedCommandLine(options, roots, nil, tspath.ComparePathsOptions{})
	p := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
	p.BindSourceFiles()
	return p
}
func p2Hex(s string) string { return hex.EncodeToString([]byte(s)) }
func p2Node(n *ast.Node) any {
	if n == nil {
		return nil
	}
	file := ast.GetSourceFileOfNode(n)
	return map[string]any{"file": file.FileName(), "kind": n.Kind, "pos": n.Pos(), "end": n.End()}
}
func p2Diagnostics(values []*ast.Diagnostic) []map[string]any {
	result := make([]map[string]any, 0, len(values))
	for _, d := range values {
		var file any
		if d.File() != nil {
			file = d.File().FileName()
		}
		result = append(result, map[string]any{"file": file, "pos": d.Pos(), "end": d.End(), "code": d.Code(), "category": d.Category(),
			"key_hex": p2Hex(string(d.MessageKey())), "text_hex": p2Hex(d.MessageText()), "source_hex": p2Hex(d.Source()), "args": d.MessageArgs(),
			"chain": p2Diagnostics(d.MessageChain()), "related": p2Diagnostics(d.RelatedInformation()), "unnecessary": d.ReportsUnnecessary(),
			"deprecated": d.ReportsDeprecated(), "skipped_on_no_emit": d.SkippedOnNoEmit()})
	}
	return result
}
// Private symbol names embed a process-local class ID. Freeze their lexical
// class identity instead; retaining the class location distinguishes shadowed #x.
func (g *p2Symbols) portableName(name string, symbol *ast.Symbol) string {
	if strings.HasPrefix(name, "\xfe@") {
		last := strings.LastIndexByte(name, '@')
		if last > 1 {
			if last == len(name)-1 { panic("P2 unique symbol name lacks runtime identity") }
			for _, b := range []byte(name[last+1:]) {
				if b < '0' || b > '9' { panic("P2 unique symbol name has invalid runtime identity") }
			}
			typ := checker.S08ExistingNameType(g.checker, symbol)
			if typ == nil || typ.Flags()&checker.TypeFlagsUniqueESSymbol == 0 || typ.Symbol() == nil {
				panic("P2 unique symbol name has no existing nameType identity")
			}
			key := typ.Symbol()
			decl := key.ValueDeclaration
			if decl == nil && len(key.Declarations) > 0 { decl = key.Declarations[0] }
			if decl == nil { panic("P2 unique symbol name has no defining declaration") }
			file := ast.GetSourceFileOfNode(decl)
			return fmt.Sprintf("%s@symbol:%s:%d:%d:%d", name[:last], p2Hex(file.FileName()), decl.Kind, decl.Pos(), decl.End())
		}
	}
	if !strings.HasPrefix(name, "\xfe#") {
		return name
	}
	i := 2
	for i < len(name) && name[i] >= '0' && name[i] <= '9' {
		i++
	}
	if i == 2 || i >= len(name) || name[i] != '@' || symbol == nil {
		panic("P2 private symbol name has no runtime class identity")
	}
	decl := symbol.ValueDeclaration
	if decl == nil && len(symbol.Declarations) > 0 {
		decl = symbol.Declarations[0]
	}
	for decl != nil && !ast.IsClassLike(decl) {
		decl = decl.Parent
	}
	if decl == nil {
		panic("P2 private symbol name has no declaring class")
	}
	file := ast.GetSourceFileOfNode(decl)
	return fmt.Sprintf("\xfe#class:%s:%d:%d:%d%s", p2Hex(file.FileName()), decl.Kind, decl.Pos(), decl.End(), name[i:])
}
func (g *p2Symbols) sortedNames(table ast.SymbolTable) []string {
	keys := make([]string, 0, len(table))
	for k := range table {
		keys = append(keys, k)
	}
	slices.SortFunc(keys, func(a, b string) int {
		return strings.Compare(g.portableName(a, table[a]), g.portableName(b, table[b]))
	})
	return keys
}

type p2Symbols struct {
	checker *checker.Checker
	ids   map[*ast.Symbol]string
	owner string
	next  int
}

func p2SymbolIDs(files []*ast.SourceFile, owner string, c *checker.Checker) *p2Symbols {
	g := &p2Symbols{ids: map[*ast.Symbol]string{}, owner: owner, checker: c}
	files = slices.Clone(files)
	slices.SortFunc(files, func(a, b *ast.SourceFile) int { return bytes.Compare([]byte(a.FileName()), []byte(b.FileName())) })
	for _, f := range files {
		next := 0
		var seed func(*ast.Symbol)
		seed = func(s *ast.Symbol) {
			if s == nil || g.ids[s] != "" {
				return
			}
			g.ids[s] = fmt.Sprintf("source:%s:%d", f.FileName(), next)
			next++
			for _, table := range []ast.SymbolTable{s.Members, s.Exports} {
				for _, name := range g.sortedNames(table) {
					seed(table[name])
				}
			}
			seed(s.Parent)
			seed(s.ExportSymbol)
		}
		var visit func(*ast.Node) bool
		visit = func(n *ast.Node) bool {
			seed(n.Symbol())
			seed(n.LocalSymbol())
			for _, name := range g.sortedNames(n.Locals()) {
				seed(n.Locals()[name])
			}
			n.ForEachChild(visit)
			return false
		}
		visit(f.AsNode())
	}
	return g
}
func (g *p2Symbols) id(s *ast.Symbol) any {
	if s == nil {
		return nil
	}
	if g.ids[s] == "" {
		g.ids[s] = fmt.Sprintf("checker:%s:%d", g.owner, g.next)
		g.next++
	}
	return g.ids[s]
}
func (g *p2Symbols) snapshot(root *ast.Symbol) any {
	if root == nil {
		return nil
	}
	queue := []*ast.Symbol{root}
	seen := map[*ast.Symbol]bool{}
	records := []map[string]any{}
	rootID := g.id(root)
	for len(queue) > 0 {
		s := queue[0]
		queue = queue[1:]
		if seen[s] {
			continue
		}
		seen[s] = true
		ref := func(value *ast.Symbol) any {
			if value != nil {
				queue = append(queue, value)
			}
			return g.id(value)
		}
		table := func(t ast.SymbolTable) any {
			if t == nil {
				return nil
			}
			entries := []map[string]any{}
			for _, name := range g.sortedNames(t) {
				entries = append(entries, map[string]any{"name_hex": p2Hex(g.portableName(name, t[name])), "symbol": ref(t[name])})
			}
			return entries
		}
		var declarations any
		if s.Declarations != nil {
			ds := []any{}
			for _, d := range s.Declarations {
				ds = append(ds, p2Node(d))
			}
			declarations = ds
		}
		records = append(records, map[string]any{"id": g.id(s), "name_hex": p2Hex(g.portableName(s.Name, s)), "flags": uint32(s.Flags), "check_flags": uint32(s.CheckFlags), "declarations": declarations,
			"value_declaration": p2Node(s.ValueDeclaration), "members": table(s.Members), "exports": table(s.Exports), "parent": ref(s.Parent), "export_symbol": ref(s.ExportSymbol)})
	}
	return map[string]any{"root": rootID, "records": records}
}
func p2BoundSnapshot(file *ast.SourceFile) map[string]any {
	g := p2SymbolIDs([]*ast.SourceFile{file}, "bound", nil)
	nodes := []map[string]any{}
	var visit func(*ast.Node) bool
	visit = func(n *ast.Node) bool {
		children := []any{}
		n.ForEachChild(func(child *ast.Node) bool { children = append(children, p2Node(child)); return false })
		nodes = append(nodes, map[string]any{"node": p2Node(n), "flags": uint32(n.Flags), "symbol": g.snapshot(n.Symbol()), "local_symbol": g.snapshot(n.LocalSymbol()), "children": children})
		n.ForEachChild(visit)
		return false
	}
	visit(file.AsNode())
	locals := []map[string]any{}
	for _, name := range g.sortedNames(file.AsNode().Locals()) {
		locals = append(locals, map[string]any{"name_hex": p2Hex(g.portableName(name, file.AsNode().Locals()[name])), "symbol": g.snapshot(file.AsNode().Locals()[name])})
	}
	return map[string]any{"file": file.FileName(), "text_hex": p2Hex(file.Text()), "nodes": nodes, "locals": locals}
}
func p2Declaration(file *ast.SourceFile, name string) *ast.Node {
	var found *ast.Node
	var visit func(*ast.Node) bool
	visit = func(n *ast.Node) bool {
		if (n.Kind == ast.KindTypeAliasDeclaration || n.Kind == ast.KindInterfaceDeclaration || n.Kind == ast.KindVariableDeclaration) && n.Name() != nil && ast.IsIdentifier(n.Name()) && n.Name().Text() == name {
			if found != nil {
				panic("ambiguous declaration: " + name)
			}
			found = n
		}
		n.ForEachChild(visit)
		return false
	}
	visit(file.AsNode())
	if found == nil {
		panic("missing declaration: " + name)
	}
	return found
}
func p2BasicType(c *checker.Checker, typ *checker.Type) map[string]any {
	// P2 exercises context-free display on both sides. Enclosing-declaration
	// name qualification and annotation reuse belong to the later display slice.
	return map[string]any{"flags": uint32(typ.Flags()), "object_flags": uint32(typ.ObjectFlags()), "display_hex": p2Hex(c.TypeToString(typ)), "in_alias_display_hex": p2Hex(c.TypeToStringEx(typ, nil, checker.TypeFormatFlagsInTypeAlias, nil))}
}
func p2Type(c *checker.Checker, typ *checker.Type, g *p2Symbols) map[string]any {
	result := p2BasicType(c, typ)
	properties := []map[string]any{}
	for _, symbol := range c.GetPropertiesOfType(typ) {
		properties = append(properties, map[string]any{"symbol": g.snapshot(symbol), "type": p2BasicType(c, c.GetTypeOfSymbol(symbol))})
	}
	result["properties"] = properties
	return result
}
func p2QueryResult(c *checker.Checker, p *compiler.Program, q p2Query) map[string]any {
	file := p.GetSourceFile(q.File)
	if file == nil {
		panic("query file missing")
	}
	decl := p2Declaration(file, q.Declaration)
	var node *ast.Node
	switch q.Target {
	case "name":
		node = decl.Name()
	case "annotation":
		node = decl.Type()
	case "annotation_name":
		node = decl.Type().AsTypeReferenceNode().TypeName
	case "initializer":
		node = decl.Initializer()
	default:
		panic("unknown query target")
	}
	if node == nil {
		panic("query target absent")
	}
	symbol := c.GetSymbolAtLocation(node)
	var typ *checker.Type
	switch q.Operation {
	case "type_at_location":
		typ = c.GetTypeAtLocation(node)
	case "declared_type", "declared_type_summary":
		if symbol == nil {
			panic("declared type lacks symbol")
		}
		typ = c.GetDeclaredTypeOfSymbol(symbol)
	default:
		panic("unknown query operation")
	}
	if typ == nil {
		panic("native query returned no type")
	}
	g := p2SymbolIDs(p.SourceFiles(), "query", c)
	if q.Operation == "declared_type_summary" {
		return map[string]any{"id": q.ID, "state": "executed", "node": p2Node(node), "symbol": g.snapshot(symbol), "type": p2BasicType(c, typ)}
	}
	return map[string]any{"id": q.ID, "state": "executed", "node": p2Node(node), "symbol": g.snapshot(symbol), "type": p2Type(c, typ, g)}
}
func p2AllDiagnostics(t *testing.T, p *compiler.Program, c *checker.Checker) map[string]any {return p2AllDiagnosticsMode(t,p,c,false)}
func p2AllDiagnosticsMode(t *testing.T, p *compiler.Program, c *checker.Checker, programMode bool) map[string]any {
	ctx := context.Background()
	phases := map[string][]*ast.Diagnostic{"config": p.GetConfigFileParsingDiagnostics(), "program": p.GetProgramDiagnostics(), "syntactic": p.GetSyntacticDiagnostics(ctx, nil), "bind": p.GetBindDiagnostics(ctx, nil), "semantic": {}, "global": {}}
	for _, file := range p.SourceFiles() {
		if p.SkipTypeChecking(file, false) { continue }
		phases["semantic"] = append(phases["semantic"], c.GetDiagnostics(ctx, file)...)
	}
    if programMode {phases["semantic"]=p.GetSemanticDiagnostics(ctx,nil)}
	phases["global"] = c.GetGlobalDiagnostics()
	observed := map[string]any{"state": "executed"}
	all := []*ast.Diagnostic{}
	phaseOrder := []string{"config", "program", "syntactic", "bind", "semantic", "global"}
    if p.Options().Declaration.IsTrue() || p.Options().Composite.IsTrue() { phases["declaration"] = p.GetDeclarationDiagnostics(ctx, nil); phaseOrder = append(phaseOrder,"declaration") }
	for _, phase := range phaseOrder {
		ds := compiler.SortAndDeduplicateDiagnostics(phases[phase])
		observed[phase] = p2Diagnostics(ds)
        if !programMode || phase != "bind" {all = append(all,ds...)}
	}
	all = compiler.SortAndDeduplicateDiagnostics(all)
	observed["combined"] = p2Diagnostics(all)
	files := []*harnessutil.TestFile{}
	for _, f := range p.SourceFiles() {
		files = append(files, &harnessutil.TestFile{UnitName: f.FileName(), Content: f.Text()})
	}
	value := baseline.NoContent
	if len(all) > 0 {
		value = tsbaseline.GetErrorBaseline(t, files, diagnosticwriter.WrapASTDiagnostics(all), diagnosticwriter.CompareASTDiagnostics, false)
	}
	if value == baseline.NoContent {
		observed["baseline"] = map[string]any{"state": "no_content"}
	} else {
		observed["baseline"] = map[string]any{"state": "content", "text_hex": p2Hex(value)}
	}
	return observed
}
func p2OptionalBool(value *bool) core.Tristate {
    if value == nil { return core.TSUnknown }
    return core.IfElse(*value, core.TSTrue, core.TSFalse)
}
func p2ObserveProgram(t *testing.T, request p2Program, libraries bool, unchecked bool, isolated bool, verbatim bool, unreachable *bool, allowJs bool, checkJs bool, implicitOverride bool, syntheticDefaults *bool, moduleInterop *bool, resolveJson *bool, programMode bool, declaration bool, isolatedDeclarations bool, stripInternal bool, target core.ScriptTarget, module core.ModuleKind, useDefine *bool, importHelpers *bool, noEmit *bool) map[string]any {
    allowUnreachable := core.TSUnknown
    if unreachable != nil { allowUnreachable = core.IfElse(*unreachable, core.TSTrue, core.TSFalse) }
	var p *compiler.Program
	if libraries {
		fs := bundled.WrapFS(vfstest.FromMap(request.Files, true))
		host := compiler.NewCompilerHost("/", fs, bundled.LibPath(), nil, nil, nil)
		options := &core.CompilerOptions{Declaration: core.IfElse(declaration,core.TSTrue,core.TSUnknown), IsolatedDeclarations: core.IfElse(isolatedDeclarations,core.TSTrue,core.TSUnknown), StripInternal: core.IfElse(stripInternal,core.TSTrue,core.TSUnknown), AllowSyntheticDefaultImports: p2OptionalBool(syntheticDefaults), ESModuleInterop: p2OptionalBool(moduleInterop), ResolveJsonModule: p2OptionalBool(resolveJson), Target: target, Module: module, UseDefineForClassFields: p2OptionalBool(useDefine), ImportHelpers: p2OptionalBool(importHelpers), NoEmit: p2OptionalBool(noEmit), Strict: core.TSTrue, SkipLibCheck: core.TSTrue, AllowUnreachableCode: allowUnreachable, NoImplicitOverride: core.IfElse(implicitOverride,core.TSTrue,core.TSUnknown), AllowJs: core.IfElse(allowJs,core.TSTrue,core.TSUnknown), CheckJs: core.IfElse(checkJs,core.TSTrue,core.TSUnknown), NoUncheckedIndexedAccess: core.IfElse(unchecked, core.TSTrue, core.TSUnknown), IsolatedModules: core.IfElse(isolated, core.TSTrue, core.TSUnknown), VerbatimModuleSyntax: core.IfElse(verbatim, core.TSTrue, core.TSUnknown)}
		config := tsoptions.NewParsedCommandLine(options, request.Roots, nil, tspath.ComparePathsOptions{})
		p = compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
		p.BindSourceFiles()
	} else {
		host := p2Host(request.Files)
        options := &core.CompilerOptions{Declaration: core.IfElse(declaration,core.TSTrue,core.TSUnknown), IsolatedDeclarations: core.IfElse(isolatedDeclarations,core.TSTrue,core.TSUnknown), StripInternal: core.IfElse(stripInternal,core.TSTrue,core.TSUnknown), AllowSyntheticDefaultImports: p2OptionalBool(syntheticDefaults), ESModuleInterop: p2OptionalBool(moduleInterop), ResolveJsonModule: p2OptionalBool(resolveJson), Target: target, Module: module, UseDefineForClassFields: p2OptionalBool(useDefine), ImportHelpers: p2OptionalBool(importHelpers), NoEmit: p2OptionalBool(noEmit), Strict: core.TSTrue, NoLib: core.TSTrue, AllowUnreachableCode: allowUnreachable, NoImplicitOverride: core.IfElse(implicitOverride,core.TSTrue,core.TSUnknown), AllowJs: core.IfElse(allowJs,core.TSTrue,core.TSUnknown), CheckJs: core.IfElse(checkJs,core.TSTrue,core.TSUnknown), NoUncheckedIndexedAccess: core.IfElse(unchecked, core.TSTrue, core.TSUnknown), IsolatedModules: core.IfElse(isolated, core.TSTrue, core.TSUnknown), VerbatimModuleSyntax: core.IfElse(verbatim, core.TSTrue, core.TSUnknown)}
        config := tsoptions.NewParsedCommandLine(options, request.Roots, nil, tspath.ComparePathsOptions{})
        p = compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
        p.BindSourceFiles()
	}
	c, _ := checker.NewChecker(p, nil)
	queries := []map[string]any{}
	for _, q := range request.Queries {
		queries = append(queries, p2QueryResult(c, p, q))
	}
	return map[string]any{"id": request.ID, "state": "executed", "queries": queries, "diagnostics": p2AllDiagnosticsMode(t, p, c, programMode)}
}

type p2Merged struct {
	row    map[string]any
	symbol *ast.Symbol
	typ    *checker.Type
}

func p2ObserveMerged(c *checker.Checker, p *compiler.Program, request p2Merge, label, path string) p2Merged {
	decl := p2Declaration(p.GetSourceFile(path), request.Symbol)
	symbol := c.GetSymbolAtLocation(decl.Name())
	if symbol == nil {
		panic("merged symbol absent")
	}
	typ := c.GetDeclaredTypeOfSymbol(symbol)
	g := p2SymbolIDs(p.SourceFiles(), label, c)
	base := p.GetSourceFile(request.Shared).AsNode().Locals()[request.Symbol]
	return p2Merged{row: map[string]any{"checker": label, "via": path, "symbol": g.snapshot(symbol), "type": p2Type(c, typ, g), "merged_is_bound_base": symbol == base}, symbol: symbol, typ: typ}
}
func p2ObserveMerge(t *testing.T, request p2Merge, mode string) map[string]any {
	host := p2Host(request.Files)
	row := map[string]any{"mode": mode, "state": "executed"}
	a := p2NewProgram(host, request.IndependentRoots["A"])
	base := a.GetSourceFile(request.Shared)
	row["base_before"] = p2BoundSnapshot(base)
	observations := []map[string]any{}
	if mode == "single-checker" || mode == "repeated" {
		p := p2NewProgram(host, request.SingleRoots)
		c, _ := checker.NewChecker(p, nil)
		row["shared_bound_file_identity"] = p.GetSourceFile(request.Shared) == base
		paths := []string{request.Shared}
		if mode == "repeated" {
			paths = []string{request.Shared, "/a.ts", "/b.ts", "/b.ts", "/a.ts", request.Shared}
		}
		var previous p2Merged
		for _, path := range paths {
			value := p2ObserveMerged(c, p, request, "single", path)
			value.row["same_symbol_as_previous"] = previous.symbol != nil && previous.symbol == value.symbol
			value.row["same_type_as_previous"] = previous.typ != nil && previous.typ == value.typ
			observations = append(observations, value.row)
			previous = value
		}
		row["diagnostics"] = map[string]any{"single": p2AllDiagnostics(t, p, c)}
	} else {
		b := p2NewProgram(host, request.IndependentRoots["B"])
		row["shared_bound_file_identity"] = b.GetSourceFile(request.Shared) == base
		var ca, cb *checker.Checker
		var va, vb p2Merged
		if mode == "concurrent" {
			start := make(chan struct{})
			ready := sync.WaitGroup{}
			done := sync.WaitGroup{}
			ready.Add(2)
			done.Add(2)
			go func() {
				defer done.Done()
				ready.Done()
				<-start
				ca, _ = checker.NewChecker(a, nil)
				va = p2ObserveMerged(ca, a, request, "A", request.Shared)
			}()
			go func() {
				defer done.Done()
				ready.Done()
				<-start
				cb, _ = checker.NewChecker(b, nil)
				vb = p2ObserveMerged(cb, b, request, "B", request.Shared)
			}()
			ready.Wait()
			close(start)
			done.Wait()
		} else if mode == "separate-checker" {
			ca, _ = checker.NewChecker(a, nil)
			cb, _ = checker.NewChecker(b, nil)
			va = p2ObserveMerged(ca, a, request, "A", request.Shared)
			vb = p2ObserveMerged(cb, b, request, "B", request.Shared)
		} else {
			panic("unknown merge mode")
		}
		observations = append(observations, va.row, vb.row)
		row["different_merged_symbols"] = va.symbol != vb.symbol
		row["different_types"] = va.typ != vb.typ
		for _, item := range []struct {
			c        *checker.Checker
			p        *compiler.Program
			label    string
			previous p2Merged
		}{{cb, b, "B", vb}, {ca, a, "A", va}} {
			value := p2ObserveMerged(item.c, item.p, request, item.label, request.Shared)
			value.row["same_symbol_as_previous"] = value.symbol == item.previous.symbol
			value.row["same_type_as_previous"] = value.typ == item.previous.typ
			observations = append(observations, value.row)
		}
		row["diagnostics"] = map[string]any{"A": p2AllDiagnostics(t, a, ca), "B": p2AllDiagnostics(t, b, cb)}
		// Retain only B's checker/program. The observed A payload has no native
		// pointers; clearing them permits collection before the next B query.
		ca = nil
		a = nil
		va = p2Merged{}
		runtime.GC()
		last := p2ObserveMerged(cb, b, request, "B", request.Shared)
		last.row["after_release_A"] = true
		last.row["same_symbol_as_previous"] = last.symbol == vb.symbol
		last.row["same_type_as_previous"] = last.typ == vb.typ
		observations = append(observations, last.row)
	}
	row["observations"] = observations
	row["base_after"] = p2BoundSnapshot(base)
	before, _ := json.Marshal(row["base_before"])
	after, _ := json.Marshal(row["base_after"])
	row["base_unchanged"] = bytes.Equal(before, after)
	return row
}
func TestS08P2(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("S08_P2_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	var spec p2Spec
	if err := decoder.Decode(&spec); err != nil {
		t.Fatal(err)
	}
	if decoder.Decode(new(any)) != io.EOF {
		t.Fatal("trailing request")
	}
    target, targetOK := map[string]core.ScriptTarget{
        "ES5":core.ScriptTargetES5,
        "ES2015":core.ScriptTargetES2015,
        "ES2016":core.ScriptTargetES2016,
        "ES2017":core.ScriptTargetES2017,
        "ES2018":core.ScriptTargetES2018,
        "ES2019":core.ScriptTargetES2019,
        "ES2020":core.ScriptTargetES2020,
        "ES2021":core.ScriptTargetES2021,
        "ES2022":core.ScriptTargetES2022,
        "ES2023":core.ScriptTargetES2023,
        "ES2024":core.ScriptTargetES2024,
        "ES2025":core.ScriptTargetES2025,
        "ESNext":core.ScriptTargetESNext,
    }[spec.Options.Target]
    module, moduleOK := map[string]core.ModuleKind{
        "None":core.ModuleKindNone,
        "CommonJS":core.ModuleKindCommonJS,
        "AMD":core.ModuleKindAMD,
        "UMD":core.ModuleKindUMD,
        "System":core.ModuleKindSystem,
        "ES2015":core.ModuleKindES2015,
        "ES2020":core.ModuleKindES2020,
        "ES2022":core.ModuleKindES2022,
        "ESNext":core.ModuleKindESNext,
        "Node16":core.ModuleKindNode16,
        "Node18":core.ModuleKindNode18,
        "Node20":core.ModuleKindNode20,
        "NodeNext":core.ModuleKindNodeNext,
        "Preserve":core.ModuleKindPreserve,
    }[spec.Options.Module]
	if spec.Version != 1 || !targetOK || !moduleOK || !spec.Options.Strict || spec.Options.NoLib == spec.Options.SkipLibCheck {
		t.Fatal("unsupported explicit P2 options")
	}
	programs := []map[string]any{}
	for _, request := range spec.Programs {
		programs = append(programs, p2ObserveProgram(t, request, !spec.Options.NoLib, spec.Options.NoUncheckedIndexedAccess, spec.Options.IsolatedModules, spec.Options.VerbatimModuleSyntax, spec.Options.AllowUnreachableCode, spec.Options.AllowJs, spec.Options.CheckJs, spec.Options.NoImplicitOverride, spec.Options.AllowSyntheticDefaultImports, spec.Options.EsModuleInterop, spec.Options.ResolveJsonModule, spec.DiagnosticMode=="program", spec.Options.Declaration, spec.Options.IsolatedDeclarations, spec.Options.StripInternal, target, module, spec.Options.UseDefineForClassFields, spec.Options.ImportHelpers, spec.Options.NoEmit))
	}
	merges := []map[string]any{}
	for _, mode := range spec.Merge.Modes {
		merges = append(merges, p2ObserveMerge(t, spec.Merge, mode))
	}
	hash := sha256.Sum256(raw)
	out, err := json.Marshal(map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "programs": programs, "merges": merges})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_P2_OUTPUT"), out, 0600); err != nil {
		t.Fatal(err)
	}
}
