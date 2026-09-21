//go:build ignore

// Access-only adapter for the pinned diagnostics generator. Run beside generate.go
// with `go test generate.go s03_export_test.go -run TestS03ExportDiagnostics`.
package main

import (
	"compress/gzip"
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"
)

type exportedMessage struct {
	Name                         string `json:"name"`
	Key                          string `json:"key"`
	Text                         string `json:"text"`
	Code                         int    `json:"code"`
	Category                     string `json:"category"`
	ReportsUnnecessary           bool   `json:"reportsUnnecessary"`
	ReportsDeprecated            bool   `json:"reportsDeprecated"`
	ElidedInCompatibilityPyramid bool   `json:"elidedInCompatibilityPyramid"`
}

func TestS03ExportDiagnostics(t *testing.T) {
	output := os.Getenv("S03_DIAGNOSTICS_OUTPUT")
	if output == "" {
		t.Fatal("S03_DIAGNOSTICS_OUTPUT must name the export file")
	}
	// The pinned generator gives extras precedence by numeric code.
	messages := readRawMessages("diagnosticMessages.json")
	for code, message := range readRawMessages("extraDiagnosticMessages.json") {
		messages[code] = message
	}
	codes := make([]int, 0, len(messages))
	for code := range messages {
		codes = append(codes, code)
	}
	slices.Sort(codes)
	rows := make([]exportedMessage, 0, len(codes))
	for _, code := range codes {
		message := messages[code]
		name, key := convertPropertyName(message.key, code)
		rows = append(rows, exportedMessage{
			Name: name, Key: key, Text: message.key, Code: code, Category: message.Category,
			ReportsUnnecessary:           message.ReportsUnnecessary,
			ReportsDeprecated:            message.ReportsDeprecated,
			ElidedInCompatibilityPyramid: message.ElidedInCompatibilityPyramid,
		})
	}
	verifyPinnedDeclarations(t, rows)
	data, err := json.MarshalIndent(struct {
		Version  int                          `json:"version"`
		Messages []exportedMessage            `json:"messages"`
		Locales  map[string]map[string]string `json:"locales"`
	}{Version: 1, Messages: rows, Locales: exportLocales(t)}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(output, append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}

// Cross-check every exported field against the untouched pinned Go output. The
// generator computes names/keys; Go's parser independently reads the checked-in
// declarations, catching missing extras, flags, or a changed adapter inventory.
func verifyPinnedDeclarations(t *testing.T, rows []exportedMessage) {
	t.Helper()
	file, err := parser.ParseFile(token.NewFileSet(), "diagnostics_generated.go", nil, 0)
	if err != nil {
		t.Fatal(err)
	}
	actual := make(map[string]exportedMessage, len(rows))
	for _, row := range rows {
		actual[row.Name] = row
	}
	seen := 0
	for _, declaration := range file.Decls {
		group, ok := declaration.(*ast.GenDecl)
		if !ok || group.Tok != token.VAR {
			continue
		}
		for _, spec := range group.Specs {
			value := spec.(*ast.ValueSpec)
			if len(value.Names) != 1 || len(value.Values) != 1 {
				t.Fatal("unexpected pinned diagnostic declaration shape")
			}
			literal := value.Values[0].(*ast.UnaryExpr).X.(*ast.CompositeLit)
			row := exportedMessage{Name: value.Names[0].Name}
			for _, element := range literal.Elts {
				field := element.(*ast.KeyValueExpr)
				name := field.Key.(*ast.Ident).Name
				switch name {
				case "code":
					row.Code, err = strconv.Atoi(field.Value.(*ast.BasicLit).Value)
				case "category":
					row.Category = strings.TrimPrefix(field.Value.(*ast.Ident).Name, "Category")
				case "key", "text":
					var text string
					text, err = strconv.Unquote(field.Value.(*ast.BasicLit).Value)
					if name == "key" {
						row.Key = text
					} else {
						row.Text = text
					}
				case "reportsUnnecessary":
					row.ReportsUnnecessary = field.Value.(*ast.Ident).Name == "true"
				case "reportsDeprecated":
					row.ReportsDeprecated = field.Value.(*ast.Ident).Name == "true"
				case "elidedInCompatibilityPyramid":
					row.ElidedInCompatibilityPyramid = field.Value.(*ast.Ident).Name == "true"
				default:
					t.Fatalf("unrecognized pinned diagnostic field %q", name)
				}
				if err != nil {
					t.Fatal(err)
				}
			}
			if row != actual[row.Name] {
				t.Fatalf("export differs from pinned diagnostic declaration %s", row.Name)
			}
			seen++
		}
	}
	if seen != len(rows) {
		t.Fatalf("export has %d rows but pinned declarations have %d", len(rows), seen)
	}
}

// The generator's compressed localization assets are data authorities too.
func exportLocales(t *testing.T) map[string]map[string]string {
	t.Helper()
	paths, err := filepath.Glob("loc/*.json.gz")
	if err != nil {
		t.Fatal(err)
	}
	if len(paths) != 13 {
		t.Fatalf("expected 13 localization assets, found %d", len(paths))
	}
	result := map[string]map[string]string{}
	for _, path := range paths {
		file, err := os.Open(path)
		if err != nil {
			t.Fatal(err)
		}
		reader, err := gzip.NewReader(file)
		if err != nil {
			file.Close()
			t.Fatal(err)
		}
		data, err := io.ReadAll(reader)
		reader.Close()
		file.Close()
		if err != nil {
			t.Fatal(err)
		}
		var values map[string]string
		if err = json.Unmarshal(data, &values); err != nil {
			t.Fatal(err)
		}
		name := strings.TrimSuffix(filepath.Base(path), ".json.gz")
		result[name] = values
	}
	return result
}
