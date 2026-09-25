// Phase 1 operation tables: the native table driver.
//
// scripts/phase1_mutation_go.py copies every .go file of this directory into
// internal/phase1table of a git-archive export of the pin (upstream/ is never
// edited), copies each bridges/<package>/*.go file into internal/<package>, and
// builds a main package, exactly like the facts and syntax mutation oracles.
// docs/PHASE1-mutation-witnesses.md (section 9) is the contract.
//
// One request row is one (column, input) pair. A column names the pinned
// operation(s) it observes; columns register themselves from the per-group
// files (<group>_columns.go) through Register. A row runs two stages:
//
//   - setup builds the input (parse or bind a source file, parse a tsconfig,
//     decode a constructed value) and returns the column closure. It is not a
//     coverage segment, so nothing it runs is Go reach.
//   - column calls the closure: the pinned operation and nothing else, since
//     every input was built in setup. It is the only production segment, so a
//     row's Go reach is exactly what its column entered.
//
// The column's single observed value is digested as sha256(canonical(value)),
// canonical being Python's json.dumps(sort_keys=True, separators=(",", ":"),
// ensure_ascii=True) over null, booleans, integers, strings, lists and
// objects. A setup panic or a column panic is the stage's outcome; the native
// freeze refuses any row that does not complete both stages. A column whose
// callers rely on a panic records it as a value through Guard.
//
// Usage:
//
//	phase1table <requests.ndjson> <rows.ndjson>            one output line per row
//	phase1table survey <requests.ndjson> <survey.ndjson>   selection classes per S06 request
//
// A rows line is {"row","outcomes":{"setup","column"},"messages"?,
// "digests":{"column"}?,"micros"}; PHASE1_TABLE_RAW=1 adds "value" (the
// canonical value itself) so the digest can be recomputed independently.
// Survey reads S06 parse requests and PHASE1_TABLE_COLUMNS (comma-separated
// column ids, each a surveyed column); each line is {"row","bytes","classes":
// {column:[class,...]}} with a column present only when its class list is
// nonempty. Classes only choose inputs; every expected value comes from rows.
package main

import (
	"bufio"
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf16"
	"unicode/utf8"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/binder"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions/tsoptionstest"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

// ---------------------------------------------------------------------------
// the column registry

// Input kinds a column may declare (data/phase1/tables/<group>.json "input").
// The four source kinds are S06 parse inputs and may be surveyed: source and
// bound (Walk), source_jsdoc and bound_jsdoc (WalkJSDoc).
var inputKinds = map[string]bool{"source": true, "bound": true, "source_jsdoc": true, "bound_jsdoc": true,
	"config": true, "values": true}

// sourceKinds are the parsed input kinds: {bind, include JSDoc in the walk}.
var sourceKinds = map[string][2]bool{"source": {false, false}, "bound": {true, false},
	"source_jsdoc": {false, true}, "bound_jsdoc": {true, true}}

// Column is one registered table column.
type Column struct {
	// ID is the column id of the table spec, e.g. "ast.IsDeclarationName".
	ID string
	// Group is the registering file's group; Register sets it.
	Group string
	// Input is the input kind (see inputKinds).
	Input string
	// Build runs in setup: it decodes and prepares the input and returns the
	// closure the column stage calls. It must leave every allocation, parse
	// and lazy computation that is not the operation's own work to setup.
	Build func(raw json.RawMessage) (func() any, error)
	// Survey, for the four source kinds, returns the selection classes of
	// one parsed (or bound) S06 file. Nil: the column's rows are synthetic.
	Survey func(p *Parsed) []string
}

var registry = map[string]*Column{}

// Register adds a group's columns; a repeated id or an unknown input kind
// stops the driver before any row runs.
func Register(group string, columns ...Column) {
	for i := range columns {
		column := columns[i]
		if column.ID == "" || column.Build == nil || !inputKinds[column.Input] {
			panic(fmt.Sprintf("phase1table: malformed column %q in group %s", column.ID, group))
		}
		if _, parsed := sourceKinds[column.Input]; column.Survey != nil && !parsed {
			panic(fmt.Sprintf("phase1table: column %s surveys a %s input", column.ID, column.Input))
		}
		if _, ok := registry[column.ID]; ok {
			panic(fmt.Sprintf("phase1table: column %s registered twice", column.ID))
		}
		column.Group = group
		registry[column.ID] = &column
	}
}

func fatal(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "phase1table: "+format+"\n", args...)
	os.Exit(2)
}

// ---------------------------------------------------------------------------
// canonical JSON

// Canonical appends the canonical encoding of value. Integers of every width
// (json.Number too), strings (valid UTF-8 only: byte strings are carried as
// hex), bools, nil, []any, []string, []int, []bool and map[string]any are
// supported; anything else is a column defect and stops the driver.
func Canonical(buffer *bytes.Buffer, value any) {
	switch v := value.(type) {
	case nil:
		buffer.WriteString("null")
	case bool:
		if v {
			buffer.WriteString("true")
		} else {
			buffer.WriteString("false")
		}
	case int:
		buffer.WriteString(strconv.FormatInt(int64(v), 10))
	case int8:
		buffer.WriteString(strconv.FormatInt(int64(v), 10))
	case int16:
		buffer.WriteString(strconv.FormatInt(int64(v), 10))
	case int32:
		buffer.WriteString(strconv.FormatInt(int64(v), 10))
	case int64:
		buffer.WriteString(strconv.FormatInt(v, 10))
	case uint:
		buffer.WriteString(strconv.FormatUint(uint64(v), 10))
	case uint8:
		buffer.WriteString(strconv.FormatUint(uint64(v), 10))
	case uint16:
		buffer.WriteString(strconv.FormatUint(uint64(v), 10))
	case uint32:
		buffer.WriteString(strconv.FormatUint(uint64(v), 10))
	case uint64:
		buffer.WriteString(strconv.FormatUint(v, 10))
	case json.Number:
		// A decoded input number (DecodeValue): integers only, as everywhere.
		integer, err := v.Int64()
		if err != nil {
			fatal("canonical: non-integer number %s", v)
		}
		buffer.WriteString(strconv.FormatInt(integer, 10))
	case string:
		canonicalString(buffer, v)
	case []any:
		buffer.WriteByte('[')
		for i, item := range v {
			if i > 0 {
				buffer.WriteByte(',')
			}
			Canonical(buffer, item)
		}
		buffer.WriteByte(']')
	case []string:
		items := make([]any, len(v))
		for i, item := range v {
			items[i] = item
		}
		Canonical(buffer, items)
	case []int:
		items := make([]any, len(v))
		for i, item := range v {
			items[i] = item
		}
		Canonical(buffer, items)
	case []bool:
		items := make([]any, len(v))
		for i, item := range v {
			items[i] = item
		}
		Canonical(buffer, items)
	case map[string]any:
		keys := make([]string, 0, len(v))
		for key := range v {
			keys = append(keys, key)
		}
		sort.Strings(keys)
		buffer.WriteByte('{')
		for i, key := range keys {
			if i > 0 {
				buffer.WriteByte(',')
			}
			canonicalString(buffer, key)
			buffer.WriteByte(':')
			Canonical(buffer, v[key])
		}
		buffer.WriteByte('}')
	default:
		fatal("canonical: unsupported value %T", value)
	}
}

func canonicalString(buffer *bytes.Buffer, v string) {
	if !utf8.ValidString(v) {
		fatal("canonical: invalid UTF-8 string; carry bytes as hex (Hex)")
	}
	buffer.WriteByte('"')
	for _, r := range v {
		switch r {
		case '"':
			buffer.WriteString(`\"`)
		case '\\':
			buffer.WriteString(`\\`)
		case '\n':
			buffer.WriteString(`\n`)
		case '\r':
			buffer.WriteString(`\r`)
		case '\t':
			buffer.WriteString(`\t`)
		case '\b':
			buffer.WriteString(`\b`)
		case '\f':
			buffer.WriteString(`\f`)
		default:
			if r < 0x20 || r > 0x7e {
				if r > 0xffff {
					a, b := utf16.EncodeRune(r)
					fmt.Fprintf(buffer, `\u%04x\u%04x`, a, b)
				} else {
					fmt.Fprintf(buffer, `\u%04x`, r)
				}
			} else {
				buffer.WriteRune(r)
			}
		}
	}
	buffer.WriteByte('"')
}

// Hex is how a column observes Go strings that may hold arbitrary bytes
// (symbol names with the \xFE internal prefix, source text slices).
func Hex(text string) string {
	return hex.EncodeToString([]byte(text))
}

// ---------------------------------------------------------------------------
// panics recorded as values

// Classify reduces a panic to its class: nil dereferences and index or slice
// bounds errors by kind (Go and Rust word them differently), any other panic
// by its message.
func Classify(r any) string {
	if err, ok := r.(runtime.Error); ok {
		text := err.Error()
		switch {
		case strings.Contains(text, "nil pointer dereference"):
			return "nil_dereference"
		case strings.Contains(text, "index out of range"), strings.Contains(text, "slice bounds out of range"):
			return "index_out_of_range"
		}
		return "runtime:" + text
	}
	switch v := r.(type) {
	case error:
		return "message:" + v.Error()
	case string:
		return "message:" + v
	}
	return fmt.Sprintf("message:%v", r)
}

// Guard runs one call of the operation. A panic whose class is in the
// column's declared contract (the spec's panic_contract) becomes the value
// {"panic": class}; any other panic propagates and fails the column stage.
func Guard(contract []string, call func() any) (value any) {
	defer func() {
		if r := recover(); r != nil {
			class := Classify(r)
			for _, allowed := range contract {
				if class == allowed {
					value = map[string]any{"panic": class}
					return
				}
			}
			panic(r)
		}
	}()
	return call()
}

// ---------------------------------------------------------------------------
// inputs

// DecodeInput decodes a column's input strictly: unknown fields are errors.
func DecodeInput(raw json.RawMessage, target any) error {
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(target); err != nil {
		return err
	}
	if decoder.More() {
		return errors.New("trailing data after the input")
	}
	return nil
}

// DecodeValue decodes any JSON value, numbers as json.Number (Canonical
// writes integers exactly and refuses anything else).
func DecodeValue(raw json.RawMessage) (any, error) {
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.UseNumber()
	var value any
	if err := decoder.Decode(&value); err != nil {
		return nil, err
	}
	if decoder.More() {
		return nil, errors.New("trailing data after the value")
	}
	return value, nil
}

// SourceInput is the parse input of the four source kinds: the fields of
// an S06 parse request (scripts/s06_oracle behavior.go parse).
type SourceInput struct {
	Filename   string `json:"filename"`
	Path       string `json:"path"`
	JSX        bool   `json:"jsx"`
	Force      bool   `json:"force"`
	ScriptKind int64  `json:"script_kind"`
	SourceHex  string `json:"source_hex"`
}

// Parsed is a parsed (and, for bound inputs, bound) source file with its
// nodes in document order (Walk, or WalkJSDoc for the _jsdoc input kinds). A
// node's reference in a value is its index in Nodes, and no other node has one.
type Parsed struct {
	File  *ast.SourceFile
	Nodes []*ast.Node
	// JSDoc is true when Nodes is WalkJSDoc's order.
	JSDoc bool
	index map[*ast.Node]int
}

// Walk is the document order of source and bound inputs, the facts oracle's
// preorder: a node, then each child subtree in ForEachChild order. Reparsed
// nodes are in it where the parser attaches them: reparsed declarations (JS
// @typedef, @callback, @import, @overload) in the enclosing list just before
// the element whose JSDoc holds them, @typedef, @callback and @import moved out
// to the nearest statement list (parser.go parseListIndex), or after the
// file's statements for the end-of-file token's JSDoc (parser.go:445); and
// reparsed parameters, types, type parameters and members as ordinary children
// of their hosts (reparser.go). JSDoc comments and everything under them are
// not in it.
func Walk(root *ast.Node) []*ast.Node {
	return walk(root, nil)
}

// WalkJSDoc is the document order of source_jsdoc and bound_jsdoc inputs:
// Walk, with a node's JSDoc comments (Node.JSDoc, which parses lazy JSDoc in
// setup) and their subtrees visited before its children, the order of
// ast.ForEachChildAndJSDoc, except that a comment is visited only under its
// parent: a reparsed @typedef or @callback declaration lists its source
// statement's comment as its own JSDoc (reparser.go:100,119), and that comment
// is visited once, under the statement. Every JSDoc comment, tag and type
// expression then has one reference, and Walk's order is a subsequence of it.
func WalkJSDoc(file *ast.SourceFile) []*ast.Node {
	return walk(file.AsNode(), file)
}

func walk(root *ast.Node, jsdoc *ast.SourceFile) []*ast.Node {
	nodes := []*ast.Node{}
	stack := []*ast.Node{root}
	for len(stack) > 0 {
		node := stack[len(stack)-1]
		stack = stack[:len(stack)-1]
		nodes = append(nodes, node)
		children := []*ast.Node{}
		if jsdoc != nil {
			for _, comment := range node.JSDoc(jsdoc) {
				if comment.Parent == node {
					children = append(children, comment)
				}
			}
		}
		node.ForEachChild(func(child *ast.Node) bool { children = append(children, child); return false })
		for i := len(children) - 1; i >= 0; i-- {
			stack = append(stack, children[i])
		}
	}
	return nodes
}

// newParsed indexes the walk of a parsed (and possibly bound) file. A node
// that the walk reaches twice would have two references, so it fails setup.
func newParsed(file *ast.SourceFile, jsdoc bool) *Parsed {
	var nodes []*ast.Node
	if jsdoc {
		nodes = WalkJSDoc(file)
	} else {
		nodes = Walk(file.AsNode())
	}
	index := make(map[*ast.Node]int, len(nodes))
	for i, node := range nodes {
		if first, ok := index[node]; ok {
			panic(fmt.Sprintf("phase1table: the walk reaches node %d (%s) again at %d", first, node.Kind, i))
		}
		index[node] = i
	}
	return &Parsed{File: file, Nodes: nodes, JSDoc: jsdoc, index: index}
}

// Ref is a node's reference in a value: nil for no node, else its index in
// Nodes. A node outside the walk has no reference and stops the driver: a
// shared placeholder such as -1 would let a port that returns the wrong JSDoc
// or synthesized node match Go. A column that observes JSDoc nodes takes a
// source_jsdoc or bound_jsdoc input; one that observes nodes no walk holds
// (factory output) projects them by their own fields.
func (p *Parsed) Ref(node *ast.Node) any {
	if node == nil {
		return nil
	}
	if at, ok := p.index[node]; ok {
		return at
	}
	walk := "Walk (a JSDoc node needs a source_jsdoc or bound_jsdoc input)"
	if p.JSDoc {
		walk = "WalkJSDoc"
	}
	fatal("Ref: %s at [%d, %d) of %s is outside the %s; project it by its own fields", node.Kind, node.Pos(),
		node.End(), p.File.FileName(), walk)
	return nil
}

// Refs is the reference list of a node slice (nil and empty are both []).
func (p *Parsed) Refs(nodes []*ast.Node) []any {
	out := make([]any, 0, len(nodes))
	for _, node := range nodes {
		out = append(out, p.Ref(node))
	}
	return out
}

// SymbolKey identifies a symbol across Go and Rust: [the reference of its
// first declaration (nil when it has none), its name as hex]. Nil for no
// symbol.
func (p *Parsed) SymbolKey(symbol *ast.Symbol) any {
	if symbol == nil {
		return nil
	}
	var first any
	if len(symbol.Declarations) > 0 {
		first = p.Ref(symbol.Declarations[0])
	}
	return []any{first, Hex(symbol.Name)}
}

func parseSource(in SourceInput) (*ast.SourceFile, error) {
	source, err := hex.DecodeString(in.SourceHex)
	if err != nil {
		return nil, fmt.Errorf("source_hex: %w", err)
	}
	opts := ast.SourceFileParseOptions{FileName: in.Filename, Path: tspath.Path(in.Path),
		ExternalModuleIndicatorOptions: ast.ExternalModuleIndicatorOptions{JSX: in.JSX, Force: in.Force}}
	return parser.ParseSourceFile(opts, string(source), core.ScriptKind(in.ScriptKind)), nil
}

// ParseSource is the setup of a source column: parse, then Walk.
func ParseSource(raw json.RawMessage) (*Parsed, error) {
	return parseInput(raw, "source")
}

// BindSource is the setup of a bound column: parse, bind, then Walk.
func BindSource(raw json.RawMessage) (*Parsed, error) {
	return parseInput(raw, "bound")
}

// ParseSourceJSDoc is the setup of a source_jsdoc column: parse, then
// WalkJSDoc.
func ParseSourceJSDoc(raw json.RawMessage) (*Parsed, error) {
	return parseInput(raw, "source_jsdoc")
}

// BindSourceJSDoc is the setup of a bound_jsdoc column: parse, bind, then
// WalkJSDoc.
func BindSourceJSDoc(raw json.RawMessage) (*Parsed, error) {
	return parseInput(raw, "bound_jsdoc")
}

func parseInput(raw json.RawMessage, kind string) (*Parsed, error) {
	var in SourceInput
	if err := DecodeInput(raw, &in); err != nil {
		return nil, err
	}
	file, err := parseSource(in)
	if err != nil {
		return nil, err
	}
	how := sourceKinds[kind]
	if how[0] {
		binder.BindSourceFile(file)
	}
	return newParsed(file, how[1]), nil
}

// ConfigInput is the input of a config column: a virtual file system, the
// tsconfig.json text at currentDirectory, and the column's own arguments.
type ConfigInput struct {
	Files            map[string]string `json:"files"`
	CurrentDirectory string            `json:"currentDirectory"`
	CaseSensitive    bool              `json:"caseSensitive"`
	JSONText         string            `json:"jsonText"`
	Args             json.RawMessage   `json:"args"`
}

// ParseConfig is the setup of a config column: tsoptionstest's VFS host and
// ParseJsonSourceFileConfigFileContent over <currentDirectory>/tsconfig.json.
func ParseConfig(raw json.RawMessage) (*tsoptions.ParsedCommandLine, *ConfigInput, error) {
	var in ConfigInput
	if err := DecodeInput(raw, &in); err != nil {
		return nil, nil, err
	}
	host := tsoptionstest.NewVFSParseConfigHost(in.Files, in.CurrentDirectory, in.CaseSensitive)
	configFileName := tspath.CombinePaths(in.CurrentDirectory, "tsconfig.json")
	source := tsoptions.NewTsconfigSourceFileFromFilePath(configFileName,
		tspath.ToPath(configFileName, in.CurrentDirectory, in.CaseSensitive), in.JSONText)
	parsed := tsoptions.ParseJsonSourceFileConfigFileContent(source, host, in.CurrentDirectory, nil, nil,
		configFileName, nil, nil)
	return parsed, &in, nil
}

// ---------------------------------------------------------------------------
// rows

type request struct {
	ID            string          `json:"id"`
	Op            string          `json:"op"`
	Column        string          `json:"column"`
	Input         json.RawMessage `json:"input"`
	RequestSHA256 string          `json:"request_sha256"`
}

// unbracketed runs setup: its panics are its outcome, and it is no segment.
func unbracketed(name string, outcomes, messages map[string]string, action func()) (ok bool) {
	defer func() {
		if value := recover(); value != nil {
			outcomes[name] = "panic"
			messages[name] = hex.EncodeToString([]byte(fmt.Sprint(value)))
			ok = false
		}
	}()
	action()
	outcomes[name] = "ok"
	return true
}

// stage runs a production segment, closing it on a panic too.
func stage(id string, name string, outcomes, messages map[string]string, action func()) (ok bool) {
	defer func() {
		if value := recover(); value != nil {
			phase1CoverEnd(id)
			outcomes[name] = "panic"
			messages[name] = hex.EncodeToString([]byte(fmt.Sprint(value)))
			ok = false
		}
	}()
	phase1CoverBegin(name)
	action()
	phase1CoverEnd(id)
	outcomes[name] = "ok"
	return true
}

func runRow(req *request, raw bool) map[string]any {
	started := time.Now()
	outcomes := map[string]string{"setup": "not_run", "column": "not_run"}
	messages := map[string]string{}
	row := map[string]any{"row": req.ID, "outcomes": outcomes}
	column, ok := registry[req.Column]
	if !ok {
		fatal("%s: unknown column %q", req.ID, req.Column)
	}
	var evaluate func() any
	if unbracketed("setup", outcomes, messages, func() {
		var err error
		evaluate, err = column.Build(req.Input)
		if err != nil {
			panic("input: " + err.Error())
		}
	}) {
		var value any
		if stage(req.ID, "column", outcomes, messages, func() { value = evaluate() }) {
			var buffer bytes.Buffer
			Canonical(&buffer, value)
			digest := sha256.Sum256(buffer.Bytes())
			row["digests"] = map[string]string{"column": hex.EncodeToString(digest[:])}
			if raw {
				row["value"] = json.RawMessage(buffer.Bytes())
			}
		}
	}
	if len(messages) > 0 {
		row["messages"] = messages
	}
	row["micros"] = time.Since(started).Microseconds()
	return row
}

// ---------------------------------------------------------------------------
// survey

type surveyRequest struct {
	ID string `json:"id"`
	SourceInput
}

func surveyColumns() []*Column {
	names := os.Getenv("PHASE1_TABLE_COLUMNS")
	if names == "" {
		fatal("survey needs PHASE1_TABLE_COLUMNS")
	}
	columns := []*Column{}
	for _, name := range strings.Split(names, ",") {
		column, ok := registry[name]
		if !ok || column.Survey == nil {
			fatal("survey: %q is not a surveyed column", name)
		}
		columns = append(columns, column)
	}
	return columns
}

func survey(line []byte, columns []*Column) map[string]any {
	var req surveyRequest
	// S06 requests carry more fields (version, op, operations, primary); the
	// survey reads the parse fields only.
	if err := json.Unmarshal(line, &req); err != nil {
		fatal("survey request: %v", err)
	}
	classes := map[string]any{}
	record := func(column *Column, parsed *Parsed) {
		found := column.Survey(parsed)
		if len(found) == 0 {
			return
		}
		set := map[string]bool{}
		for _, class := range found {
			set[class] = true
		}
		list := make([]string, 0, len(set))
		for class := range set {
			list = append(list, class)
		}
		sort.Strings(list)
		classes[column.ID] = list
	}
	// One parse per walk, as each row's setup does: parse, bind for the bound
	// kinds, then walk. The unbound columns of a walk are surveyed before its
	// file is bound; binding creates no node, so both see the same walk.
	for _, jsdoc := range []bool{false, true} {
		var file *ast.SourceFile
		var parsed *Parsed
		bound := false
		for _, kind := range []string{"source", "source_jsdoc", "bound", "bound_jsdoc"} {
			how := sourceKinds[kind]
			if how[1] != jsdoc {
				continue
			}
			for _, column := range columns {
				if column.Input != kind {
					continue
				}
				if file == nil {
					var err error
					if file, err = parseSource(req.SourceInput); err != nil {
						fatal("%s: %v", req.ID, err)
					}
				}
				if how[0] && !bound {
					binder.BindSourceFile(file)
					bound, parsed = true, nil
				}
				if parsed == nil {
					parsed = newParsed(file, jsdoc)
				}
				record(column, parsed)
			}
		}
	}
	return map[string]any{"row": req.ID, "bytes": len(req.SourceHex) / 2, "classes": classes}
}

// NodeClasses is the default survey classifier of a per-node column: for
// every node, (kind, value class) and (parent kind, value class), plus the
// joint (kind, parent kind) of every node whose value class is not "0".
// valueClass maps a node to a short class string.
func NodeClasses(p *Parsed, valueClass func(node *ast.Node) string) []string {
	classes := []string{}
	for _, node := range p.Nodes {
		value := valueClass(node)
		parent := -1
		if node.Parent != nil {
			parent = int(node.Parent.Kind)
		}
		classes = append(classes, fmt.Sprintf("k%d/%s", node.Kind, value), fmt.Sprintf("p%d/%s", parent, value))
		if value != "0" {
			classes = append(classes, fmt.Sprintf("j%d/%d/%s", node.Kind, parent, value))
		}
	}
	return classes
}

// BoolClass is the value class of a predicate.
func BoolClass(value bool) string {
	if value {
		return "1"
	}
	return "0"
}

// ---------------------------------------------------------------------------

func main() {
	var mode, input, output string
	switch {
	case len(os.Args) == 3:
		mode, input, output = "rows", os.Args[1], os.Args[2]
	case len(os.Args) == 4 && os.Args[1] == "survey":
		mode, input, output = "survey", os.Args[2], os.Args[3]
	default:
		fatal("usage: phase1table <requests.ndjson> <rows.ndjson> | phase1table survey <requests.ndjson> <survey.ndjson>")
	}
	in, err := os.Open(input)
	if err != nil {
		fatal("%v", err)
	}
	defer in.Close()
	out, err := os.Create(output)
	if err != nil {
		fatal("%v", err)
	}
	var columns []*Column
	if mode == "survey" {
		columns = surveyColumns()
	}
	raw := os.Getenv("PHASE1_TABLE_RAW") == "1"
	reader := bufio.NewReaderSize(in, 1<<20)
	writer := bufio.NewWriter(out)
	seen := map[string]bool{}
	for {
		line, err := reader.ReadBytes('\n')
		if err != nil && !errors.Is(err, io.EOF) {
			fatal("%v", err)
		}
		last := err != nil
		if len(bytes.TrimSpace(line)) == 0 {
			if last {
				break
			}
			fatal("blank request line")
		}
		var row map[string]any
		if mode == "survey" {
			row = survey(line, columns)
		} else {
			var req request
			decoder := json.NewDecoder(bytes.NewReader(line))
			decoder.DisallowUnknownFields()
			if err := decoder.Decode(&req); err != nil {
				fatal("request: %v", err)
			}
			if req.ID == "" || seen[req.ID] || req.Op != "table" {
				fatal("invalid, duplicate or non-table request %q", req.ID)
			}
			seen[req.ID] = true
			row = runRow(&req, raw)
		}
		encoded, err := json.Marshal(row)
		if err != nil {
			fatal("%v", err)
		}
		writer.Write(encoded)
		writer.WriteByte('\n')
		// One flushed line per row: a crash loses no finished row.
		if err := writer.Flush(); err != nil {
			fatal("%v", err)
		}
		if last {
			break
		}
	}
	if err := out.Close(); err != nil {
		fatal("%v", err)
	}
}
