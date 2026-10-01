package printer

// Native witness for the printer's source-map positions: each case parses a
// file, prints it through Write with a source-map generator as emitter.go's
// printSourceFile does, and records the printed text and generator.String().
// Overlaid into the pinned package; not part of it.
//
// A case with `writes` instead reuses one printer: it parses `file_name` and
// each of `extra_files`, rewrites marker statements and identifiers when
// `rewrite` is set ($notemitted; becomes a NotEmittedStatement,
// $created_<x> a created identifier, $ranged_<x> a created identifier with the
// marker's source-map range), applies `edits` to nodes picked by kind and
// pre-order index, and performs each write in order: the picked node of the
// picked file, with or without its source file and with one of `generators`
// source-map generators or none. It records each write's text and each
// generator's String().
//
// Run from upstream/tsc with an overlay that adds this file to
// internal/printer: A1_MAP_CASES=<requests.json> A1_MAP_OUTPUT=<results.json>
// go test -overlay <overlay.json> ./internal/printer -run 'TestA1SourceMaps$'.

import (
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"strings"
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

	ExtraFiles             []a1MapFile  `json:"extra_files"`
	Rewrite                bool         `json:"rewrite"`
	Edits                  []a1MapEdit  `json:"edits"`
	Generators             int          `json:"generators"`
	Writes                 []a1MapWrite `json:"writes"`
	PreserveSourceNewlines bool         `json:"preserve_source_newlines"`
}

type a1MapFile struct {
	FileName  string `json:"file_name"`
	SourceHex string `json:"source_hex"`
}

// a1MapPick picks the index-th node of a kind (named without "Kind") in a
// pre-order walk of a file's printed tree; an empty kind picks the file.
type a1MapPick struct {
	File  int    `json:"file"`
	Kind  string `json:"kind"`
	Index int    `json:"index"`
}

type a1MapEdit struct {
	a1MapPick
	Flags          EmitFlags `json:"flags"`
	SourceMapRange []int     `json:"source_map_range"`
	CommentRange   []int     `json:"comment_range"`
	Token          string    `json:"token"`
	TokenRange     []int     `json:"token_range"`
}

type a1MapWrite struct {
	a1MapPick
	NoSourceFile bool `json:"no_source_file"`
	// Generator is a 1-based index into the case's generators; 0 is none.
	Generator int `json:"generator"`
}

type a1MapResult struct {
	ID        string   `json:"id"`
	OutputHex string   `json:"output_hex,omitempty"`
	MapHex    string   `json:"map_hex,omitempty"`
	Outputs   []string `json:"outputs_hex,omitempty"`
	Maps      []string `json:"maps_hex,omitempty"`
}

type a1MappedSource struct {
	fileName string
	text     string
	lineMap  []core.TextPos
}

func (s *a1MappedSource) FileName() string            { return s.fileName }
func (s *a1MappedSource) Text() string                { return s.text }
func (s *a1MappedSource) ECMALineMap() []core.TextPos { return s.lineMap }

func a1ParseMapFile(fileName string, sourceHex string) *ast.SourceFile {
	raw, err := hex.DecodeString(sourceHex)
	if err != nil {
		panic(err)
	}
	return parser.ParseSourceFile(ast.SourceFileParseOptions{
		FileName: fileName,
		Path:     tspath.Path(fileName),
	}, string(raw), core.GetScriptKindFromFileName(fileName))
}

// a1KindName is a kind's name without its "Kind" prefix.
func a1KindName(kind ast.Kind) string {
	return strings.TrimPrefix(kind.String(), "Kind")
}

func a1PickNode(files []*ast.SourceFile, pick a1MapPick) *ast.Node {
	root := files[pick.File].AsNode()
	if pick.Kind == "" {
		return root
	}
	var found *ast.Node
	count := 0
	var walk func(node *ast.Node) bool
	walk = func(node *ast.Node) bool {
		if a1KindName(node.Kind) == pick.Kind {
			if count == pick.Index {
				found = node
				return true
			}
			count++
		}
		return node.ForEachChild(walk)
	}
	walk(root)
	if found == nil {
		panic("no node picked")
	}
	return found
}

func a1Range(values []int) core.TextRange {
	return core.NewTextRange(values[0], values[1])
}

func a1TokenKind(name string) ast.Kind {
	for kind := ast.KindUnknown; kind <= ast.KindCount; kind++ {
		if a1KindName(kind) == name {
			return kind
		}
	}
	panic("unknown token " + name)
}

// a1Rewrite applies the marker rewrites of a case to one file.
func a1Rewrite(ec *EmitContext, file *ast.SourceFile) *ast.SourceFile {
	var visitor *ast.NodeVisitor
	visitor = ec.NewNodeVisitor(func(node *ast.Node) *ast.Node {
		switch node.Kind {
		case ast.KindExpressionStatement:
			if expression := node.Expression(); expression.Kind == ast.KindIdentifier && expression.Text() == "$notemitted" {
				return ec.NewNotEmittedStatement(node)
			}
		case ast.KindIdentifier:
			text := node.Text()
			switch {
			case strings.HasPrefix(text, "$created_"):
				return ec.Factory.NewIdentifier(strings.TrimPrefix(text, "$created_"))
			case strings.HasPrefix(text, "$ranged_"):
				identifier := ec.Factory.NewIdentifier(strings.TrimPrefix(text, "$ranged_"))
				ec.SetSourceMapRange(identifier, node.Loc)
				return identifier
			}
			return node
		}
		return node.VisitEachChild(visitor)
	})
	return visitor.VisitSourceFile(file)
}

func a1RunWrites(c a1MapCase) a1MapResult {
	ec := NewEmitContext()
	files := []*ast.SourceFile{a1ParseMapFile(c.FileName, c.SourceHex)}
	for _, extra := range c.ExtraFiles {
		files = append(files, a1ParseMapFile(extra.FileName, extra.SourceHex))
	}
	if c.Rewrite {
		for i, file := range files {
			files[i] = a1Rewrite(ec, file)
		}
	}
	for _, edit := range c.Edits {
		node := a1PickNode(files, edit.a1MapPick)
		if edit.Flags != 0 {
			ec.AddEmitFlags(node, edit.Flags)
		}
		if edit.SourceMapRange != nil {
			ec.SetSourceMapRange(node, a1Range(edit.SourceMapRange))
		}
		if edit.CommentRange != nil {
			ec.SetCommentRange(node, a1Range(edit.CommentRange))
		}
		if edit.Token != "" {
			ec.SetTokenSourceMapRange(node, a1TokenKind(edit.Token), a1Range(edit.TokenRange))
		}
	}
	p := NewPrinter(PrinterOptions{
		NewLine:                     core.NewLineKindLF,
		RemoveComments:              c.RemoveComments,
		SourceMap:                   true,
		InlineSources:               c.InlineSources,
		OmitBraceSourceMapPositions: c.OmitBraces,
		PreserveSourceNewlines:      c.PreserveSourceNewlines,
	}, PrintHandlers{}, ec)
	var generators []*sourcemap.Generator
	for range c.Generators {
		generators = append(generators, sourcemap.NewGenerator("main.js", "" /*sourceRoot*/, "/", tspath.ComparePathsOptions{
			UseCaseSensitiveFileNames: true,
			CurrentDirectory:          "/",
		}))
	}
	result := a1MapResult{ID: c.ID}
	for _, write := range c.Writes {
		node := a1PickNode(files, write.a1MapPick)
		var sourceFile *ast.SourceFile
		if !write.NoSourceFile {
			sourceFile = files[write.File]
		}
		var generator *sourcemap.Generator
		if write.Generator > 0 {
			generator = generators[write.Generator-1]
		}
		result.Outputs = append(result.Outputs, hex.EncodeToString([]byte(a1Write(p, node, sourceFile, generator))))
	}
	for _, generator := range generators {
		result.Maps = append(result.Maps, hex.EncodeToString([]byte(generator.String())))
	}
	return result
}

// a1Write prints one node and returns the text, or the panic's message.
func a1Write(p *Printer, node *ast.Node, sourceFile *ast.SourceFile, generator *sourcemap.Generator) (text string) {
	defer func() {
		if r := recover(); r != nil {
			text = fmt.Sprint("panic: ", r)
		}
	}()
	writer := NewTextWriter("\n", 0)
	p.Write(node, sourceFile, writer, generator)
	return writer.String()
}

func a1RunMap(c a1MapCase) a1MapResult {
	if c.Writes != nil {
		return a1RunWrites(c)
	}
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
