// Native observations for the formatter port (docs/S09-3-formatter-plan.md).
//
// A persistent process: one JSON request per input line, one JSON observation
// per output line. It is built inside a fresh export of the pinned tree, as the
// S06 oracle is, because it imports internal packages. It calls the pinned
// navigation, indentation and formatting entry points and nothing else; no
// upstream source is modified.
//
// Observations are row streams. Each stream is published as a row count and the
// SHA-256 of its rows, so a corpus-wide comparison stays small, and literally
// when the request asks for detail, so one mismatching file can be diagnosed.
// The row grammar is line based and avoids JSON on purpose: the Rust side has to
// reproduce it byte for byte.
package main

import (
	"bufio"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"hash"
	"io"
	"os"
	"strconv"
	"strings"

	"github.com/microsoft/TypeScript/tsc/internal/api/encoder"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/astnav"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/format"
	"github.com/microsoft/TypeScript/tsc/internal/ls/lsutil"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/printer"
	"github.com/microsoft/TypeScript/tsc/internal/scanner"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

const (
	protocolVersion = 1
	maxRequest      = 64 << 20
	// At most this many evenly spaced probe positions per file, plus the end.
	maxPositions = 512
	// The leading statements of a file that are encoded, decoded and printed.
	maxStatements = 4
)

type request struct {
	Version    int      `json:"version"`
	ID         string   `json:"id"`
	SourceHex  string   `json:"source_hex"`
	FileName   string   `json:"filename"`
	Path       string   `json:"path"`
	ScriptKind int      `json:"script_kind"`
	JSX        bool     `json:"jsx"`
	Force      bool     `json:"force"`
	Ops        []string `json:"ops"`
	Detail     bool     `json:"detail"`
}

// stream accumulates rows; every row ends in a newline.
type stream struct {
	rows     int
	failures int
	hash     hash.Hash
	detail *strings.Builder
}

func newStream(detail bool) *stream {
	s := &stream{hash: sha256.New()}
	if detail {
		s.detail = &strings.Builder{}
	}
	return s
}

func (s *stream) row(text string) {
	s.rows++
	// '!' marks a native panic and '?' a returned native error.
	if strings.Contains(text, "|!") || strings.Contains(text, "|?") {
		s.failures++
	}
	io.WriteString(s.hash, text)
	io.WriteString(s.hash, "\n")
	if s.detail != nil {
		s.detail.WriteString(text)
		s.detail.WriteString("\n")
	}
}

func (s *stream) result() map[string]any {
	out := map[string]any{"rows": s.rows, "failures": s.failures, "sha256": hex.EncodeToString(s.hash.Sum(nil))}
	if s.detail != nil {
		out["detail"] = s.detail.String()
	}
	return out
}

func node(n *ast.Node) string {
	if n == nil {
		return "-"
	}
	return strconv.Itoa(int(n.Kind)) + "," + strconv.Itoa(n.Pos()) + "," + strconv.Itoa(n.End())
}

// call runs one native entry point. A native panic becomes the row's value,
// marked with '!', so one failing position does not hide the rest of the file
// and the port is held to the failure as well as to the successes.
func call(action func() string) (text string) {
	defer func() {
		if recovered := recover(); recovered != nil {
			text = "!" + fmt.Sprint(recovered)
		}
	}()
	return action()
}

// positions are evenly spaced byte offsets, ending with the end of the text.
// They depend on the length alone, so both sides derive the same set.
func positions(length int) []int {
	stride := (length + maxPositions) / maxPositions
	if stride < 1 {
		stride = 1
	}
	var out []int
	for p := 0; p < length; p += stride {
		out = append(out, p)
	}
	return append(out, length)
}

var childKinds = []ast.Kind{
	ast.KindOpenBraceToken, ast.KindCloseBraceToken, ast.KindOpenParenToken,
	ast.KindCloseParenToken, ast.KindOpenBracketToken, ast.KindCloseBracketToken,
}

// The call shapes are the ones the formatter uses (format/indent.go,
// format/context.go, format/span.go, format/rulecontext.go).
func navigation(file *ast.SourceFile, detail bool) map[string]any {
	s := newStream(detail)
	for _, p := range positions(len(file.Text())) {
		at := strconv.Itoa(p)
		var token, preceding *ast.Node
		s.row("T|" + at + "|" + call(func() string {
			token = astnav.GetTokenAtPosition(file, p)
			return node(token)
		}))
		s.row("P|" + at + "|" + call(func() string { return node(astnav.FindPrecedingToken(file, p)) }))
		s.row("X|" + at + "|" + call(func() string {
			preceding = astnav.FindPrecedingTokenEx(file, p, nil, true)
			return node(preceding)
		}))
		if token != nil {
			s.row("S|" + at + "|" + call(func() string {
				return strconv.Itoa(astnav.GetStartOfNode(token, file, false)) +
					"|" + strconv.Itoa(astnav.GetStartOfNode(token, file, true))
			}))
			if token.Parent != nil {
				for _, kind := range childKinds {
					s.row("C|" + at + "|" + strconv.Itoa(int(kind)) + "|" + call(func() string {
						return node(astnav.FindChildOfKind(token.Parent, kind, file))
					}))
				}
			}
		}
		if preceding != nil && preceding.Parent != nil {
			s.row("N|" + at + "|" + call(func() string {
				return node(astnav.FindNextToken(preceding, preceding.Parent, file))
			}))
			s.row("F|" + at + "|" + call(func() string {
				return node(astnav.FindNextToken(preceding, file.AsNode(), file))
			}))
		}
	}
	return s.result()
}

type variant struct {
	name     string
	settings lsutil.FormatCodeSettings
}

// The variants are fixed here and mirrored on the Rust side by name.
func variants() []variant {
	base := lsutil.GetDefaultFormatCodeSettings()
	tabs := base
	tabs.ConvertTabsToSpaces = core.TSFalse
	two := base
	two.IndentSize, two.TabSize = 2, 2
	dense := base
	dense.InsertSpaceAfterConstructor = core.TSTrue
	dense.InsertSpaceAfterFunctionKeywordForAnonymousFunctions = core.TSTrue
	dense.InsertSpaceAfterOpeningAndBeforeClosingNonemptyParenthesis = core.TSTrue
	dense.InsertSpaceAfterOpeningAndBeforeClosingNonemptyBrackets = core.TSTrue
	dense.InsertSpaceAfterOpeningAndBeforeClosingEmptyBraces = core.TSTrue
	dense.InsertSpaceAfterOpeningAndBeforeClosingTemplateStringBraces = core.TSTrue
	dense.InsertSpaceAfterOpeningAndBeforeClosingJsxExpressionBraces = core.TSTrue
	dense.InsertSpaceAfterTypeAssertion = core.TSTrue
	dense.InsertSpaceBeforeFunctionParenthesis = core.TSTrue
	dense.InsertSpaceBeforeTypeAnnotation = core.TSTrue
	dense.PlaceOpenBraceOnNewLineForFunctions = core.TSTrue
	dense.PlaceOpenBraceOnNewLineForControlBlocks = core.TSTrue
	dense.Semicolons = lsutil.SemicolonPreferenceInsert
	terse := base
	terse.InsertSpaceAfterCommaDelimiter = core.TSFalse
	terse.InsertSpaceAfterSemicolonInForStatements = core.TSFalse
	terse.InsertSpaceBeforeAndAfterBinaryOperators = core.TSFalse
	terse.InsertSpaceAfterKeywordsInControlFlowStatements = core.TSFalse
	terse.InsertSpaceAfterOpeningAndBeforeClosingNonemptyBraces = core.TSFalse
	terse.IndentSwitchCase = core.TSFalse
	terse.TrimTrailingWhitespace = core.TSFalse
	terse.Semicolons = lsutil.SemicolonPreferenceRemove
	return []variant{{"default", base}, {"tabs", tabs}, {"two", two}, {"dense", dense}, {"terse", terse}}
}

// indentation probes every line start under both assumptions, and every eighth
// navigation position under the ordinary one.
func indentation(file *ast.SourceFile, settings lsutil.FormatCodeSettings, detail bool) map[string]any {
	s := newStream(detail)
	for _, start := range scanner.GetECMALineStarts(file) {
		p := int(start)
		for _, assume := range []bool{false, true} {
			s.row("L|" + strconv.Itoa(p) + "|" + strconv.Itoa(core.IfElse(assume, 1, 0)) + "|" + call(func() string {
				return strconv.Itoa(format.GetIndentation(p, file, settings, assume))
			}))
		}
	}
	for index, p := range positions(len(file.Text())) {
		if index%8 == 0 {
			s.row("I|" + strconv.Itoa(p) + "|" + call(func() string {
				return strconv.Itoa(format.GetIndentation(p, file, settings, false))
			}))
		}
	}
	return s.result()
}

func document(file *ast.SourceFile, settings lsutil.FormatCodeSettings, detail bool) map[string]any {
	ctx := format.WithFormatCodeSettings(context.Background(), settings, settings.NewLineCharacter)
	edits := format.FormatDocument(ctx, file)
	s := newStream(detail)
	for _, edit := range edits {
		s.row("E|" + strconv.Itoa(edit.Pos()) + "|" + strconv.Itoa(edit.End()) + "|" +
			hex.EncodeToString([]byte(edit.NewText)))
	}
	out := s.result()
	// The edit list is the observation. Applying it is a second, separate fact:
	// some settings make the pinned formatter emit overlapping edits, which
	// ApplyBulkEdits cannot apply, and that is recorded rather than hidden.
	func() {
		defer func() {
			if recovered := recover(); recovered != nil {
				out["text_panic"] = fmt.Sprint(recovered)
			}
		}()
		text := sha256.Sum256([]byte(core.ApplyBulkEdits(file.Text(), edits)))
		out["text_sha256"] = hex.EncodeToString(text[:])
	}()
	return out
}

// decoded is what an API request carries: the statement encoded to protocol
// bytes and decoded into a fresh tree with no source file and no parents above
// it. The wire digest is its own row, so an encoding difference shows up as one
// and does not pass for a printing difference.
func decoded(s *stream, file *ast.SourceFile, index int) *ast.Node {
	var root *ast.Node
	s.row("W|" + strconv.Itoa(index) + "|" + call(func() string {
		wire, _, err := encoder.EncodeNode(file.Statements.Nodes[index], file)
		if err != nil {
			return "?" + err.Error()
		}
		sum := sha256.Sum256(wire)
		node, err := encoder.DecodeNodes(wire)
		if err != nil {
			return "?" + err.Error()
		}
		root = node
		return hex.EncodeToString(sum[:])
	}))
	return root
}

func statements(file *ast.SourceFile) int {
	if file.Statements == nil {
		return 0
	}
	return min(len(file.Statements.Nodes), maxStatements)
}

// positioned is printer.PrintAndPositionNode as the insertion handler calls it:
// the printed text, then every node of the positioned clone in child order.
func positioned(file *ast.SourceFile, detail bool) map[string]any {
	s := newStream(detail)
	for index := range statements(file) {
		at := strconv.Itoa(index)
		root := decoded(s, file, index)
		if root == nil {
			continue
		}
		var clone *ast.Node
		s.row("P|" + at + "|" + call(func() string {
			settings := lsutil.GetDefaultFormatCodeSettings()
			text, node := printer.PrintAndPositionNode(ast.NewNodeFactory(ast.NodeFactoryHooks{}), root, nil,
				settings.NewLineCharacter, settings.IndentSize, nil)
			clone = node
			return hex.EncodeToString([]byte(text))
		}))
		if clone == nil {
			continue
		}
		s.row("N|" + at + "|" + call(func() string {
			var out strings.Builder
			var walk func(n *ast.Node) bool
			// Nested, so a structural difference is visible as one:
			// kind,pos,end(children...) in child order.
			walk = func(n *ast.Node) bool {
				out.WriteString(node(n))
				out.WriteByte('(')
				n.ForEachChild(walk)
				out.WriteByte(')')
				return false
			}
			walk(clone)
			return out.String()
		}))
	}
	return s.result()
}

// targets are where a statement is inserted: three line starts spread over the
// file and one offset that is usually inside a line.
func targets(file *ast.SourceFile) []int {
	lines := scanner.GetECMALineStarts(file)
	var out []int
	add := func(p int) {
		for _, seen := range out {
			if seen == p {
				return
			}
		}
		out = append(out, p)
	}
	for _, index := range []int{0, len(lines) / 3, 2 * len(lines) / 3} {
		add(int(lines[index]))
	}
	all := positions(len(file.Text()))
	add(all[len(all)/2])
	return out
}

// insertion is the body of the pinned handleFormatNodeForInsertion after its
// request decoding, with the target offset already in bytes.
func insertion(file *ast.SourceFile, settings lsutil.FormatCodeSettings, detail bool) map[string]any {
	s := newStream(detail)
	for index := range statements(file) {
		root := decoded(s, file, index)
		if root == nil {
			continue
		}
		for _, pos := range targets(file) {
			s.row("R|" + strconv.Itoa(index) + "|" + strconv.Itoa(pos) + "|" + call(func() string {
				newLine := settings.NewLineCharacter
				factory := ast.NewNodeFactory(ast.NodeFactoryHooks{})
				text, nodeWithPos := printer.PrintAndPositionNode(factory, root, nil, newLine, settings.IndentSize, nil)
				synthetic := printer.CreateSyntheticSourceFile(factory, nodeWithPos, text, file.ParseOptions())
				atLineStart := format.GetLineStartPositionForPosition(pos, file) == pos
				initial := format.GetIndentation(pos, file, settings, atLineStart)
				delta := 0
				if settings.IndentSize != 0 && format.ShouldIndentChildNode(settings, root, nil, nil) {
					delta = settings.IndentSize
				}
				ctx := format.WithFormatCodeSettings(context.Background(), settings, newLine)
				changes := format.FormatNodeGivenIndentation(ctx, nodeWithPos, synthetic, file.LanguageVariant, initial, delta)
				return hex.EncodeToString([]byte(core.ApplyBulkEdits(text, changes)))
			}))
		}
	}
	return s.result()
}

// guarded records a native panic as the observation instead of ending the
// process: a panic is a native outcome the port has to account for.
func guarded(out map[string]any, name string, action func() any) {
	defer func() {
		if recovered := recover(); recovered != nil {
			out[name] = map[string]any{"panic": fmt.Sprint(recovered)}
		}
	}()
	out[name] = action()
}

func observe(r request) map[string]any {
	out := map[string]any{"id": r.ID}
	source, err := hex.DecodeString(r.SourceHex)
	if err != nil {
		out["error"] = "source_hex: " + err.Error()
		return out
	}
	var file *ast.SourceFile
	guarded(out, "parse", func() any {
		options := ast.SourceFileParseOptions{
			FileName: r.FileName, Path: tspath.Path(r.Path),
			ExternalModuleIndicatorOptions: ast.ExternalModuleIndicatorOptions{JSX: r.JSX, Force: r.Force},
		}
		file = parser.ParseSourceFile(options, string(source), core.ScriptKind(r.ScriptKind))
		return map[string]any{"end": file.End(), "node_count": file.NodeCount,
			"language_variant": int(file.LanguageVariant), "lines": len(scanner.GetECMALineStarts(file))}
	})
	if file == nil {
		return out
	}
	for _, op := range r.Ops {
		switch op {
		case "nav":
			guarded(out, "nav", func() any { return navigation(file, r.Detail) })
		case "indent":
			result := map[string]any{}
			for _, v := range variants()[:3] {
				guarded(result, v.name, func() any { return indentation(file, v.settings, r.Detail) })
			}
			out["indent"] = result
		case "format":
			result := map[string]any{}
			for _, v := range variants() {
				guarded(result, v.name, func() any { return document(file, v.settings, r.Detail) })
			}
			out["format"] = result
		case "position":
			guarded(out, "position", func() any { return positioned(file, r.Detail) })
		case "rules":
			result := map[string]any{}
			for _, v := range variants() {
				guarded(result, v.name, func() any {
					s := newStream(r.Detail)
					format.S09RulesProbe(file, v.settings, s.row)
					return s.result()
				})
			}
			out["rules"] = result
		case "rulesmap":
			guarded(out, "rulesmap", func() any {
				s := newStream(r.Detail)
				format.S09RulesMapProbe(s.row)
				return s.result()
			})
		case "scan":
			guarded(out, "scan", func() any {
				s := newStream(r.Detail)
				format.S09ScanProbe(file, s.row)
				return s.result()
			})
		case "insert":
			result := map[string]any{}
			for _, v := range variants()[:2] {
				guarded(result, v.name, func() any { return insertion(file, v.settings, r.Detail) })
			}
			out["insert"] = result
		default:
			out["error"] = "unknown operation " + strconv.Quote(op)
			return out
		}
	}
	return out
}

func run(in io.Reader, out io.Writer) error {
	reader := bufio.NewReaderSize(in, 1<<20)
	writer := bufio.NewWriter(out)
	seen := map[string]bool{}
	for {
		line, err := reader.ReadBytes('\n')
		if len(line) > maxRequest {
			return fmt.Errorf("request record too large")
		}
		if err == io.EOF {
			if len(line) != 0 {
				return fmt.Errorf("truncated request record")
			}
			return writer.Flush()
		}
		if err != nil {
			return err
		}
		var r request
		decoder := json.NewDecoder(strings.NewReader(string(line)))
		decoder.DisallowUnknownFields()
		if err := decoder.Decode(&r); err != nil {
			return fmt.Errorf("invalid request: %w", err)
		}
		if r.Version != protocolVersion || r.ID == "" || len(r.Ops) == 0 {
			return fmt.Errorf("request %q: unsupported version, empty id or no operations", r.ID)
		}
		if seen[r.ID] {
			return fmt.Errorf("duplicate request identity %q", r.ID)
		}
		seen[r.ID] = true
		encoded, err := json.Marshal(observe(r))
		if err != nil {
			return err
		}
		writer.Write(encoded)
		writer.WriteByte('\n')
		if err := writer.Flush(); err != nil {
			return err
		}
	}
}

func main() {
	if err := run(os.Stdin, os.Stdout); err != nil {
		fmt.Fprintln(os.Stderr, "S09 format oracle protocol:", err)
		os.Exit(2)
	}
}
