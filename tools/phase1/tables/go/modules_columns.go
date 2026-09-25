// Phase 1 operation tables: group modules (modules, imports, type-only, augmentations, symbol names).
// Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/modules.rs and the spec is
// data/phase1/tables/modules.json.
package main

import (
	"encoding/json"
	"maps"
	"slices"
	"strings"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
)

// moduleKinds are the module options IsEffectiveExternalModule is observed
// under, one bit each in this order.
var moduleKinds = []core.ModuleKind{core.ModuleKindNone, core.ModuleKindCommonJS, core.ModuleKindAMD,
	core.ModuleKindUMD, core.ModuleKindSystem, core.ModuleKindES2015, core.ModuleKindES2020,
	core.ModuleKindES2022, core.ModuleKindESNext, core.ModuleKindNode16, core.ModuleKindNode18,
	core.ModuleKindNode20, core.ModuleKindNodeNext, core.ModuleKindPreserve}

// stableSymbolName blanks the node id a pattern ambient module's name ends
// with (binder.go:314): the id is a process-global counter, so it is no
// contract and differs between runs.
func stableSymbolName(name string) string {
	if at := strings.LastIndex(name, "\"pattern@"); at >= 0 {
		return name[:at] + "\"pattern@#"
	}
	return name
}

// symbolOf is the symbol a bound node declares, as the checker reads it.
func symbolOf(node *ast.Node) *ast.Symbol { return node.Symbol() }

// exportClauseDomain excludes the one node IsExportNamespaceAsDefaultDeclaration
// panics on, an export declaration without a clause (it reads the clause's
// kind); its callers pass declarations with clauses.
func exportClauseDomain(node *ast.Node) bool {
	return !ast.IsExportDeclaration(node) || node.AsExportDeclaration().ExportClause != nil
}

// moduleSymbol admits the nodes whose symbol is a module, the argument of
// GetSourceFileOfModule.
func moduleSymbol(node *ast.Node) bool {
	symbol := node.Symbol()
	return symbol != nil && symbol.Flags&ast.SymbolFlagsModule != 0
}

func init() {
	Register("modules",
		NodePredicate("ast.IsAnyExportAssignment", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsAnyExportAssignment(node) }),
		NodePredicate("ast.IsImportDeclarationOrJSImportDeclaration", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsImportDeclarationOrJSImportDeclaration(node) }),
		ValuesMap("ast.EscapeAllInternalSymbolNames",
			func(name string) any { return Hex(ast.EscapeAllInternalSymbolNames(name)) }),
		ValuesMap("ast.EscapeInternalSymbolName",
			func(name string) any { return Hex(ast.EscapeInternalSymbolName(name)) }),
		ValuesMap("ast.EscapeSymbolName",
			func(name string) any { return Hex(ast.EscapeSymbolName(name)) }),
		ValuesMap("ast.TryGetAmbientModuleNameFromSymbolName",
			func(name string) any {
				if module, ok := ast.TryGetAmbientModuleNameFromSymbolName(name); ok {
					return Hex(module)
				}
				return nil
			}),
		ValuesMap("ast.IsAmbientModuleSymbolName",
			func(name string) any { return ast.IsAmbientModuleSymbolName(name) }),
		NodeMap("ast.SymbolName", "bound", All,
			func(_ *Parsed, node *ast.Node) any {
				if symbol := symbolOf(node); symbol != nil {
					return Hex(stableSymbolName(ast.SymbolName(symbol)))
				}
				return nil
			}),
		NodeMap("ast.Symbol.CombinedLocalAndExportSymbolFlags", "bound", All,
			func(_ *Parsed, node *ast.Node) any {
				if symbol := symbolOf(node); symbol != nil {
					return []any{int(symbol.CombinedLocalAndExportSymbolFlags()), symbol.ExportSymbol != nil}
				}
				return nil
			}),
		NodeMap("ast.Symbol.IsExternalModule", "bound", All,
			func(_ *Parsed, node *ast.Node) any {
				if symbol := symbolOf(node); symbol != nil {
					return symbol.IsExternalModule()
				}
				return nil
			}),
		NodeMap("ast.GetNonAugmentationDeclaration", "bound", All,
			func(p *Parsed, node *ast.Node) any {
				if symbol := symbolOf(node); symbol != nil {
					return RefOf(p, ast.GetNonAugmentationDeclaration(symbol))
				}
				return nil
			}),
		NodeMap("ast.GetSourceFileOfModule", "bound", moduleSymbol,
			func(p *Parsed, node *ast.Node) any {
				if file := ast.GetSourceFileOfModule(node.Symbol()); file != nil {
					return p.Ref(file.AsNode())
				}
				return nil
			}),
		NodeMap("ast.GetExternalModuleImportEqualsDeclarationExpression", "source",
			ast.IsExternalModuleImportEqualsDeclaration,
			func(p *Parsed, node *ast.Node) any {
				return RefOf(p, ast.GetExternalModuleImportEqualsDeclarationExpression(node))
			}),
		NodeMap("ast.GetExternalModuleName", "source",
			Kinds(ast.KindImportDeclaration, ast.KindJSImportDeclaration, ast.KindExportDeclaration,
				ast.KindImportEqualsDeclaration, ast.KindImportType, ast.KindCallExpression, ast.KindModuleDeclaration),
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetExternalModuleName(node)) }),
		NodeMap("ast.GetImportAttributes", "source",
			Kinds(ast.KindImportDeclaration, ast.KindJSImportDeclaration, ast.KindExportDeclaration, ast.KindImportType),
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetImportAttributes(node)) }),
		NodeMap("ast.GetModuleSpecifierOfBareOrAccessedRequire", "source", All,
			func(p *Parsed, node *ast.Node) any {
				return RefOf(p, ast.GetModuleSpecifierOfBareOrAccessedRequire(node))
			}),
		NodePredicate("ast.HasImportAttributes", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.HasImportAttributes(node) }),
		NodePredicate("ast.HasResolutionModeOverride", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.HasResolutionModeOverride(node) }),
		// Over the string literals whose import TryGetImportFromModuleSpecifier
		// finds: ImportFromModuleSpecifier fails an assertion on the others.
		NodeMap("ast.ImportFromModuleSpecifier", "source",
			func(node *ast.Node) bool {
				return ast.IsStringLiteralLike(node) && ast.TryGetImportFromModuleSpecifier(node) != nil
			},
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.ImportFromModuleSpecifier(node)) }),
		NodeMap("ast.TryGetImportFromModuleSpecifier", "source", Kinds(ast.KindStringLiteral, ast.KindNoSubstitutionTemplateLiteral),
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.TryGetImportFromModuleSpecifier(node)) }),
		NodePredicate("ast.IsDefaultImport", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsDefaultImport(node) }),
		NodeMap("ast.IsEffectiveExternalModule", "bound", Kinds(ast.KindSourceFile),
			func(_ *Parsed, node *ast.Node) any {
				bits := 0
				for bit, kind := range moduleKinds {
					if ast.IsEffectiveExternalModule(node.AsSourceFile(), &core.CompilerOptions{Module: kind}) {
						bits |= 1 << bit
					}
				}
				return Int(bits)
			}),
		NodePredicate("ast.IsEmittableImport", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsEmittableImport(node) }),
		NodePredicate("ast.IsExportNamespaceAsDefaultDeclaration", "source", exportClauseDomain,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsExportNamespaceAsDefaultDeclaration(node) }),
		NodePredicate("ast.IsExternalModuleAugmentation", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsExternalModuleAugmentation(node) }),
		NodePredicate("ast.IsExternalModuleImportEqualsDeclaration", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsExternalModuleImportEqualsDeclaration(node) }),
		NodePredicate("ast.IsExternalModuleIndicator", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsExternalModuleIndicator(node) }),
		NodePredicate("ast.IsInternalModuleImportEqualsDeclaration", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsInternalModuleImportEqualsDeclaration(node) }),
		NodePredicate("ast.IsModuleWithStringLiteralName", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsModuleWithStringLiteralName(node) }),
		NodePredicate("ast.IsPartOfTypeOnlyImportOrExportDeclaration", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsPartOfTypeOnlyImportOrExportDeclaration(node) }),
		NodePredicate("ast.IsTypeOnlyImportOrExportDeclaration", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsTypeOnlyImportOrExportDeclaration(node) }),
		NodePredicate("ast.IsTypeOnlyImportDeclaration", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsTypeOnlyImportDeclaration(node) }),
		// Bit i is the result under checkJs Unknown, False, True.
		NodeMap("ast.IsPlainJSFile", "source", Kinds(ast.KindSourceFile),
			func(_ *Parsed, node *ast.Node) any {
				bits := 0
				for bit, checkJs := range []core.Tristate{core.TSUnknown, core.TSFalse, core.TSTrue} {
					if ast.IsPlainJSFile(node.AsSourceFile(), checkJs) {
						bits |= 1 << bit
					}
				}
				return Int(bits)
			}),
		NodePredicate("ast.IsRequireVariableStatement", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsRequireVariableStatement(node) }),
		// Over the JSDoc walk: a use site in JSDoc is valid.
		NodePredicate("ast.IsValidTypeOnlyAliasUseSite", "source_jsdoc", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsValidTypeOnlyAliasUseSite(node) }),
		NodePredicate("ast.IsVariableDeclarationInitializedToBareOrAccessedRequire", "source", All,
			func(_ *Parsed, node *ast.Node) bool {
				return ast.IsVariableDeclarationInitializedToBareOrAccessedRequire(node)
			}),
		// The last pragma of each name the parser records, in pragmaNames
		// order: its arguments as [[name hex, value hex], ...] sorted by name,
		// or null when the file has no pragma of that name.
		Column{
			ID:    "ast.GetPragmaFromSourceFile",
			Input: "source",
			Build: func(raw json.RawMessage) (func() any, error) {
				p, err := ParseSource(raw)
				if err != nil {
					return nil, err
				}
				return func() any {
					out := []any{}
					for _, name := range pragmaNames {
						out = append(out, pragmaArgs(ast.GetPragmaFromSourceFile(p.File, name)))
					}
					return out
				}, nil
			},
			Survey: func(p *Parsed) []string { return pragmaClasses(p, false) },
		},
		// For each pragma of the file in order, [name hex, [the argument value
		// hex for each name in pragmaArguments order]], then the same values
		// for a nil pragma.
		Column{
			ID:    "ast.GetPragmaArgument",
			Input: "source",
			Build: func(raw json.RawMessage) (func() any, error) {
				p, err := ParseSource(raw)
				if err != nil {
					return nil, err
				}
				return func() any {
					out := []any{}
					for i := range p.File.Pragmas {
						pragma := &p.File.Pragmas[i]
						out = append(out, []any{Hex(pragma.Name), pragmaValues(pragma)})
					}
					out = append(out, []any{nil, pragmaValues(nil)})
					return out
				}, nil
			},
			Survey: func(p *Parsed) []string { return pragmaClasses(p, true) },
		},
		// The emit module format of constructed file names, module options and
		// metadata: the module kind per case.
		typedValuesColumn("ast.GetEmitModuleFormatOfFileWorker", func(in struct {
			Cases []struct {
				FileName          string `json:"file_name"`
				Module            int    `json:"module"`
				ImpliedNodeFormat int    `json:"implied_node_format"`
				PackageJsonType   string `json:"package_json_type"`
			} `json:"cases"`
		}) any {
			out := []any{}
			for _, c := range in.Cases {
				options := &core.CompilerOptions{Module: core.ModuleKind(c.Module)}
				meta := ast.SourceFileMetaData{PackageJsonType: c.PackageJsonType, ImpliedNodeFormat: core.ResolutionMode(c.ImpliedNodeFormat)}
				out = append(out, Scalar(int(ast.GetEmitModuleFormatOfFileWorker(c.FileName, options, meta))))
			}
			return out
		}),
	)
}

// The pragma names the pinned parser records (parser.go, processPragmasIntoFields)
// and the argument names those pragmas carry.
var pragmaNames = []string{"reference", "amd-dependency", "amd-module", "ts-check", "ts-nocheck", "jsx", "jsxfrag", "jsximportsource", "jsxruntime"}

var pragmaArguments = []string{"path", "types", "lib", "no-default-lib", "resolution-mode", "preserve", "name", "factory"}

// pragmaArgs projects a pragma as its arguments sorted by name, or nil.
func pragmaArgs(pragma *ast.Pragma) any {
	if pragma == nil {
		return nil
	}
	out := []any{}
	for _, name := range slices.Sorted(maps.Keys(pragma.Args)) {
		out = append(out, []any{Hex(name), Hex(pragma.Args[name].Value)})
	}
	return out
}

// pragmaValues is GetPragmaArgument over every argument name, in order.
func pragmaValues(pragma *ast.Pragma) any {
	out := []any{}
	for _, name := range pragmaArguments {
		out = append(out, Hex(ast.GetPragmaArgument(pragma, name)))
	}
	return out
}

// pragmaClasses are the survey classes of the pragma columns: the pragma names
// present (n:<name>) or, with arguments, the (name, argument) pairs present
// (a:<name>/<argument>); "0" for a file without pragmas.
func pragmaClasses(p *Parsed, arguments bool) []string {
	classes := []string{}
	for i := range p.File.Pragmas {
		pragma := &p.File.Pragmas[i]
		if !arguments {
			classes = append(classes, "n:"+pragma.Name)
			continue
		}
		for _, name := range slices.Sorted(maps.Keys(pragma.Args)) {
			classes = append(classes, "a:"+pragma.Name+"/"+name)
		}
	}
	if len(classes) == 0 {
		classes = append(classes, "0")
	}
	return classes
}
