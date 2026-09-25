// Phase 1 operation tables: group diagnostics (AST diagnostics).
// Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/diagnostics.rs and the spec is
// data/phase1/tables/diagnostics.json.
package main

import (
	"encoding/hex"
	"encoding/json"
	"fmt"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/locale"
)

// diagnosticCase is one constructed diagnostic of a diagnostics column; each
// column reads the fields its constructor takes.
type diagnosticCase struct {
	Code        int32    `json:"code"`
	Category    int32    `json:"category"`
	TextHex     string   `json:"text_hex"`
	Key         string   `json:"key"`
	ArgsHex     []string `json:"args_hex"`
	SourceHex   string   `json:"source_hex"`
	Unnecessary bool     `json:"unnecessary"`
	Deprecated  bool     `json:"deprecated"`
	Skipped     bool     `json:"skipped"`
	Chain       bool     `json:"chain"`
	Related     bool     `json:"related"`
	Pos         int      `json:"pos"`
	End         int      `json:"end"`
}

type diagnosticCases struct {
	Cases   []diagnosticCase `json:"cases"`
	Locales []string         `json:"locales"`
}

func unhexText(text string) string {
	bytes, err := hex.DecodeString(text)
	if err != nil {
		fatal("diagnostics: bad hex %q", text)
	}
	return string(bytes)
}

func unhexTexts(texts []string) []string {
	out := make([]string, len(texts))
	for i, text := range texts {
		out[i] = unhexText(text)
	}
	return out
}

// parts are the chain and related information a case asks for: one ad hoc
// diagnostic each.
func (c diagnosticCase) parts() (chain []*ast.Diagnostic, related []*ast.Diagnostic) {
	if c.Chain {
		chain = []*ast.Diagnostic{ast.NewDiagnosticFromText(nil, core.NewTextRange(1, 2), 7001, diagnostics.CategoryMessage, "chained", nil, nil, false, false)}
	}
	if c.Related {
		related = []*ast.Diagnostic{ast.NewDiagnosticFromText(nil, core.NewTextRange(3, 4), 7002, diagnostics.CategoryMessage, "related", nil, nil, false, false)}
	}
	return chain, related
}

// diagnosticValue projects a diagnostic: [code, category, key hex, [args hex],
// message text hex, reportsUnnecessary, reportsDeprecated, skippedOnNoEmit,
// chain length, related length, pos, end, String hex, [Localize hex per
// locale]].
func diagnosticValue(d *ast.Diagnostic, locales []locale.Locale) any {
	args := []any{}
	for _, arg := range d.MessageArgs() {
		args = append(args, Hex(arg))
	}
	localized := []any{}
	for _, l := range locales {
		localized = append(localized, Hex(d.Localize(l)))
	}
	return []any{Scalar(int(d.Code())), Scalar(int(d.Category())), Hex(string(d.MessageKey())), args, Hex(d.MessageText()),
		d.ReportsUnnecessary(), d.ReportsDeprecated(), d.SkippedOnNoEmit(), Scalar(len(d.MessageChain())),
		Scalar(len(d.RelatedInformation())), Scalar(d.Pos()), Scalar(d.End()), Hex(d.String()), localized}
}

// diagnosticValuesColumn is a values column over diagnosticCases: [each case's
// diagnostic, projected by diagnosticValue].
func diagnosticValuesColumn(id string, build func(c diagnosticCase) *ast.Diagnostic) Column {
	return Column{
		ID:    id,
		Input: "values",
		Build: func(raw json.RawMessage) (func() any, error) {
			var in diagnosticCases
			if err := DecodeInput(raw, &in); err != nil {
				return nil, err
			}
			locales := make([]locale.Locale, len(in.Locales))
			for i, name := range in.Locales {
				parsed, ok := locale.Parse(name)
				if !ok {
					return nil, fmt.Errorf("bad locale %q", name)
				}
				locales[i] = parsed
			}
			return func() any {
				out := []any{}
				for _, c := range in.Cases {
					out = append(out, diagnosticValue(build(c), locales))
				}
				return out
			}, nil
		},
	}
}

func init() {
	Register("diagnostics",
		diagnosticValuesColumn("ast.NewDiagnosticFromText", func(c diagnosticCase) *ast.Diagnostic {
			chain, related := c.parts()
			return ast.NewDiagnosticFromText(nil, core.NewTextRange(c.Pos, c.End), c.Code,
				diagnostics.Category(c.Category), unhexText(c.TextHex), chain, related, c.Unnecessary, c.Deprecated)
		}),
		diagnosticValuesColumn("ast.NewDiagnosticFromSerialized", func(c diagnosticCase) *ast.Diagnostic {
			chain, related := c.parts()
			return ast.NewDiagnosticFromSerialized(nil, core.NewTextRange(c.Pos, c.End), c.Code,
				diagnostics.Category(c.Category), diagnostics.Key(c.Key), unhexTexts(c.ArgsHex), chain, related,
				c.Unnecessary, c.Deprecated, c.Skipped)
		}),
		diagnosticValuesColumn("ast.NewExternalDiagnostic", func(c diagnosticCase) *ast.Diagnostic {
			return ast.NewExternalDiagnostic(nil, core.NewTextRange(c.Pos, c.End), unhexText(c.SourceHex),
				diagnostics.Category(c.Category), c.Code, unhexText(c.TextHex))
		}),
		// With Cannot_find_name_0 and the case's arguments, chained to the
		// case's ad hoc diagnostic when it asks for a chain.
		diagnosticValuesColumn("ast.NewDiagnosticChain", func(c diagnosticCase) *ast.Diagnostic {
			var chain *ast.Diagnostic
			if c.Chain {
				_, related := c.parts()
				chain = ast.NewDiagnosticFromText(nil, core.NewTextRange(c.Pos, c.End), c.Code,
					diagnostics.Category(c.Category), unhexText(c.TextHex), nil, related, false, false)
			}
			args := []any{}
			for _, arg := range unhexTexts(c.ArgsHex) {
				args = append(args, arg)
			}
			return ast.NewDiagnosticChain(chain, diagnostics.Cannot_find_name_0, args...)
		}),
		// The info before and after SetRepopulateInfo on an ad hoc diagnostic:
		// [whether it was nil, [kind, module reference hex, mode, package hex]].
		Column{
			ID:    "ast.Diagnostic.RepopulateInfo",
			Input: "values",
			Build: func(raw json.RawMessage) (func() any, error) {
				var in diagnosticCases
				if err := DecodeInput(raw, &in); err != nil {
					return nil, err
				}
				return func() any {
					out := []any{}
					for _, c := range in.Cases {
						d := ast.NewDiagnosticFromText(nil, core.NewTextRange(c.Pos, c.End), c.Code,
							diagnostics.Category(c.Category), unhexText(c.TextHex), nil, nil, false, false)
						before := d.RepopulateInfo() == nil
						d.SetRepopulateInfo(&ast.RepopulateDiagnosticInfo{
							Kind: ast.RepopulateDiagnosticKind(c.Code % 3), ModuleReference: unhexText(c.SourceHex),
							Mode: core.ResolutionMode(c.Category), PackageName: unhexText(c.TextHex),
						})
						var after any
						if info := d.RepopulateInfo(); info != nil {
							after = []any{Scalar(int(info.Kind)), Hex(info.ModuleReference), Scalar(int(info.Mode)),
								Hex(info.PackageName)}
						}
						out = append(out, []any{before, after})
					}
					return out
				}, nil
			},
		},
		// Over the source file: a collection of one Cannot_find_name_0
		// diagnostic per top-level statement (the first 20, added last to
		// first, the argument the statement index modulo 3) and two global
		// ones; the value is GetDiagnostics' order as [in the file, pos, end,
		// code, args hex].
		NodeMap("ast.DiagnosticsCollection.GetDiagnostics", "source", Kinds(ast.KindSourceFile),
			func(p *Parsed, node *ast.Node) any {
				var collection ast.DiagnosticsCollection
				statements := node.Statements()
				if len(statements) > 20 {
					statements = statements[:20]
				}
				for i := len(statements) - 1; i >= 0; i-- {
					collection.Add(ast.NewDiagnostic(p.File, statements[i].Loc, diagnostics.Cannot_find_name_0, fmt.Sprint(i%3)))
				}
				collection.Add(ast.NewCompilerDiagnostic(diagnostics.Cannot_find_name_0, "g1"))
				collection.Add(ast.NewCompilerDiagnostic(diagnostics.Cannot_find_name_0, "g0"))
				out := []any{}
				for _, d := range collection.GetDiagnostics() {
					args := []any{}
					for _, arg := range d.MessageArgs() {
						args = append(args, Hex(arg))
					}
					out = append(out, []any{d.File() != nil, Scalar(d.Pos()), Scalar(d.End()), Scalar(int(d.Code())), args})
				}
				return out
			}),
	)
}
