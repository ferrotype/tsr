package tsoptions

import (
	"encoding/json"
	"os"
	"reflect"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/core"
)

// phase1Field is one field of core.CompilerOptions in declaration order: its
// index among all fields (reflect's), and the flags of the option declaration
// CommandLineCompilerOptionsMap finds for its name.
type phase1Field struct {
	Index                      int    `json:"index"`
	Field                      string `json:"field"`
	Exported                   bool   `json:"exported"`
	Declaration                string `json:"declaration"`
	AffectsEmit                bool   `json:"affects_emit"`
	AffectsDeclarationPath     bool   `json:"affects_declaration_path"`
	AffectsSemanticDiagnostics bool   `json:"affects_semantic_diagnostics"`
	StrictFlag                 bool   `json:"strict_flag"`
	AllowJsFlag                bool   `json:"allow_js_flag"`
	Category                   string `json:"category"`
}

func TestPhase1OptionFields(t *testing.T) {
	fields := []phase1Field{}
	options := reflect.TypeFor[core.CompilerOptions]()
	for i := range options.NumField() {
		field := options.Field(i)
		row := phase1Field{Index: i, Field: field.Name, Exported: field.IsExported()}
		if declaration := CommandLineCompilerOptionsMap.Get(field.Name); field.IsExported() && declaration != nil {
			row.Declaration = declaration.Name
			row.AffectsEmit = declaration.AffectsEmit
			row.AffectsDeclarationPath = declaration.AffectsDeclarationPath
			row.AffectsSemanticDiagnostics = declaration.AffectsSemanticDiagnostics
			row.StrictFlag = declaration.strictFlag
			row.AllowJsFlag = declaration.allowJsFlag
			if declaration.Category != nil {
				row.Category = string(declaration.Category.Key())
			}
		}
		fields = append(fields, row)
	}
	data, err := json.Marshal(fields)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("PHASE1_OPTION_FIELDS_OUTPUT"), append(data, '\n'), 0o600); err != nil {
		t.Fatal(err)
	}
}
