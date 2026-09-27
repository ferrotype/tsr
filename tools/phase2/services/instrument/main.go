// Command instrument writes the Phase 2 C5.5 services recorder overlay: copies
// of the pinned checker package's files in which every exported entry point
// the language service can call (the exported methods of Checker, NodeBuilder
// and EmitResolver, and the exported package functions that take a checker)
// reports its arguments and results to the recorder. Only text is inserted:
// the pinned formatting and comments are kept, so the overlay's difference is
// exactly the recording statements and the names given to unnamed parameters
// and results.
//
//	go run ./tools/phase2/services/instrument -src upstream/tsc/internal/checker -out DIR
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// The checker each receiver or package function reaches.
var receivers = map[string]string{
	"Checker":      "%s",
	"NodeBuilder":  "%s.impl.ch",
	"EmitResolver": "%s.checker",
}

var packageFunctions = map[string]string{
	"NewNodeBuilder":                       "ch",
	"NewNodeBuilderEx":                     "ch",
	"GetResolvedSignatureForSignatureHelp": "c",
	"SkipAlias":                            "checker",
}

// Accessors that reach no checker state.
var skipped = map[string]bool{
	"NodeBuilder.EmitContext": true,
}

// Entry points whose symbols the pin collects from a map (symbolsToArray,
// maps.Values): their order is unspecified and differs between runs, and the
// language service walks it. The recording build returns them in one fixed
// order, one the pin can produce, so the calls that follow are reproducible.
var unordered = map[string]bool{
	"Checker.GetSymbolsInScope":               true,
	"Checker.GetExportsOfModule":              true,
	"Checker.GetExportsAndPropertiesOfModule": true,
	"Checker.GetAllPossiblePropertiesOfTypes": true,
	"Checker.GetAmbientModules":               true,
}

type edit struct {
	offset int
	end    int
	text   string
}

type instrumented struct {
	File string `json:"file"`
	Op   string `json:"op"`
}

func main() {
	src := flag.String("src", "", "the pinned checker package directory")
	out := flag.String("out", "", "the overlay directory to write")
	flag.Parse()
	if *src == "" || *out == "" {
		fmt.Fprintln(os.Stderr, "usage: instrument -src DIR -out DIR")
		os.Exit(2)
	}
	names, err := filepath.Glob(filepath.Join(*src, "*.go"))
	if err != nil {
		panic(err)
	}
	sort.Strings(names)
	var report []instrumented
	for _, name := range names {
		if strings.HasSuffix(name, "_test.go") {
			continue
		}
		source, err := os.ReadFile(name)
		if err != nil {
			panic(err)
		}
		fset := token.NewFileSet()
		file, err := parser.ParseFile(fset, name, source, parser.ParseComments)
		if err != nil {
			panic(err)
		}
		var edits []edit
		for _, decl := range file.Decls {
			fn, ok := decl.(*ast.FuncDecl)
			if !ok || fn.Body == nil || !ast.IsExported(fn.Name.Name) {
				continue
			}
			op, checker, ok := target(fn)
			if !ok || skipped[op] {
				continue
			}
			edits = append(edits, instrument(fset, source, fn, op, checker)...)
			report = append(report, instrumented{File: filepath.Base(name), Op: op})
		}
		if len(edits) == 0 {
			continue
		}
		sort.SliceStable(edits, func(i, j int) bool { return edits[i].offset > edits[j].offset })
		text := string(source)
		for _, e := range edits {
			text = text[:e.offset] + e.text + text[e.end:]
		}
		if err := os.WriteFile(filepath.Join(*out, filepath.Base(name)), []byte(text), 0o644); err != nil {
			panic(err)
		}
	}
	encoded, err := json.MarshalIndent(report, "", " ")
	if err != nil {
		panic(err)
	}
	fmt.Println(string(encoded))
}

func target(fn *ast.FuncDecl) (op string, checker string, ok bool) {
	if fn.Recv == nil {
		expression, found := packageFunctions[fn.Name.Name]
		return fn.Name.Name, expression, found
	}
	if len(fn.Recv.List) != 1 {
		return "", "", false
	}
	star, isStar := fn.Recv.List[0].Type.(*ast.StarExpr)
	if !isStar {
		return "", "", false
	}
	ident, isIdent := star.X.(*ast.Ident)
	if !isIdent {
		return "", "", false
	}
	pattern, found := receivers[ident.Name]
	if !found || len(fn.Recv.List[0].Names) != 1 {
		return "", "", false
	}
	return ident.Name + "." + fn.Name.Name, fmt.Sprintf(pattern, fn.Recv.List[0].Names[0].Name), true
}

func offset(fset *token.FileSet, pos token.Pos) int {
	return fset.Position(pos).Offset
}

func instrument(fset *token.FileSet, source []byte, fn *ast.FuncDecl, op string, checker string) []edit {
	var edits []edit
	var params []string
	index := 0
	for _, field := range fn.Type.Params.List {
		if len(field.Names) == 0 {
			name := fmt.Sprintf("phase2P%d", index)
			edits = append(edits, edit{offset(fset, field.Type.Pos()), offset(fset, field.Type.Pos()), name + " "})
			params = append(params, name)
			index++
			continue
		}
		for _, ident := range field.Names {
			name := ident.Name
			if name == "_" {
				name = fmt.Sprintf("phase2P%d", index)
				edits = append(edits, edit{offset(fset, ident.Pos()), offset(fset, ident.End()), name})
			}
			params = append(params, name)
			index++
		}
	}
	var results []string
	if fn.Type.Results != nil {
		list := fn.Type.Results
		unnamed := len(list.List) != 0 && len(list.List[0].Names) == 0
		if unnamed && !list.Opening.IsValid() {
			start, end := offset(fset, list.Pos()), offset(fset, list.End())
			edits = append(edits, edit{start, end, "(phase2R0 " + string(source[start:end]) + ")"})
			results = append(results, "phase2R0")
		} else {
			index := 0
			for _, field := range list.List {
				if len(field.Names) == 0 {
					name := fmt.Sprintf("phase2R%d", index)
					edits = append(edits, edit{offset(fset, field.Type.Pos()), offset(fset, field.Type.Pos()), name + " "})
					results = append(results, name)
					index++
					continue
				}
				for _, ident := range field.Names {
					name := ident.Name
					if name == "_" {
						name = fmt.Sprintf("phase2R%d", index)
						edits = append(edits, edit{offset(fset, ident.Pos()), offset(fset, ident.End()), name})
					}
					results = append(results, name)
					index++
				}
			}
		}
	}
	args := "[]any{" + strings.Join(params, ", ") + "}"
	values := "[]any{" + strings.Join(results, ", ") + "}"
	order := ""
	if unordered[op] {
		order = fmt.Sprintf("\n\t\t\t%[1]s = phase2Order(%[1]s)", results[0])
	}
	statement := fmt.Sprintf(`
	if phase2Call := phase2Enter(%[1]s, %[2]q, %[3]s); phase2Call != nil {
		defer func() {
			if phase2Panic := recover(); phase2Panic != nil {
				phase2Call.exit(%[1]s, %[3]s, nil, phase2Panic)
				panic(phase2Panic)
			}%[5]s
			phase2Call.exit(%[1]s, %[3]s, %[4]s, nil)
		}()
	}`, checker, op, args, values, order)
	body := offset(fset, fn.Body.Lbrace) + 1
	edits = append(edits, edit{body, body, statement})
	return edits
}
