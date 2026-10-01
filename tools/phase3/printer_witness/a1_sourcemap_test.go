package printer

// Native witness for the printer's source-map positions: each case parses a
// file, prints it through Write with a source-map generator as emitter.go's
// printSourceFile does, and records the printed text and generator.String().
// Overlaid into the pinned package; not part of it.

import (
	"encoding/hex"
	"encoding/json"
	"os"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/sourcemap"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

type a1MapCase struct {
	ID             string `json:"id"`
	FileName       string `json:"file_name"`
	SourceHex      string `json:"source_hex"`
	InlineSources  bool   `json:"inline_sources"`
	OmitBraces     bool   `json:"omit_braces"`
	RemoveComments bool   `json:"remove_comments"`
	CRLF           bool   `json:"crlf"`
	Mapped         bool   `json:"mapped"`
}

type a1MapResult struct {
	ID        string `json:"id"`
	OutputHex string `json:"output_hex"`
	MapHex    string `json:"map_hex"`
}

type a1MappedSource struct {
	fileName string
	text     string
	lineMap  []core.TextPos
}

func (s *a1MappedSource) FileName() string            { return s.fileName }
func (s *a1MappedSource) Text() string                { return s.text }
func (s *a1MappedSource) ECMALineMap() []core.TextPos { return s.lineMap }

func a1RunMap(c a1MapCase) a1MapResult {
	raw, err := hex.DecodeString(c.SourceHex)
	if err != nil {
		panic(err)
	}
	text := string(raw)
	file := parser.ParseSourceFile(ast.SourceFileParseOptions{
		FileName: c.FileName,
		Path:     tspath.Path(c.FileName),
	}, text, core.GetScriptKindFromFileName(c.FileName))
	newLine := core.NewLineKindLF
	if c.CRLF {
		newLine = core.NewLineKindCRLF
	}
	handlers := PrintHandlers{}
	if c.Mapped {
		original := &a1MappedSource{fileName: "/original.ts", text: text, lineMap: core.ComputeECMALineStarts(text)}
		handlers.MapSourcePosition = func(source sourcemap.Source, pos int) (sourcemap.Source, int, bool) {
			if source.FileName() != c.FileName {
				return source, pos, true
			}
			if pos%3 == 0 {
				return nil, 0, false
			}
			return original, pos, true
		}
	}
	p := NewPrinter(PrinterOptions{
		NewLine:                     newLine,
		RemoveComments:              c.RemoveComments,
		SourceMap:                   true,
		InlineSources:               c.InlineSources,
		OmitBraceSourceMapPositions: c.OmitBraces,
	}, handlers, NewEmitContext())
	generator := sourcemap.NewGenerator("main.js", "" /*sourceRoot*/, "/", tspath.ComparePathsOptions{
		UseCaseSensitiveFileNames: true,
		CurrentDirectory:          "/",
	})
	writer := NewTextWriter(newLine.GetNewLineCharacter(), 0)
	p.Write(file.AsNode(), file, writer, generator)
	return a1MapResult{ID: c.ID, OutputHex: hex.EncodeToString([]byte(writer.String())), MapHex: hex.EncodeToString([]byte(generator.String()))}
}

func TestA1SourceMaps(t *testing.T) {
	requests := os.Getenv("A1_MAP_CASES")
	if requests == "" {
		t.Skip("A1_MAP_CASES is not set")
	}
	data, err := os.ReadFile(requests)
	if err != nil {
		t.Fatal(err)
	}
	var cases []a1MapCase
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatal(err)
	}
	var results []a1MapResult
	for _, c := range cases {
		results = append(results, a1RunMap(c))
	}
	out, err := json.MarshalIndent(results, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("A1_MAP_OUTPUT"), out, 0o644); err != nil {
		t.Fatal(err)
	}
}
