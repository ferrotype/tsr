// Parse-only S06 observations over the pinned production parser and encoder.
// Installed in a disposable pin export; no production algorithm is implemented here.
package main

import (
	"bufio"
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"slices"

	"github.com/microsoft/TypeScript/tsc/internal/api/encoder"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

type object = map[string]any

type request struct {
	Version    int      `json:"version"`
	ID         string   `json:"id"`
	Primary    *string  `json:"primary"`
	Op         string   `json:"op"`
	SourceHex  string   `json:"source_hex"`
	Filename   string   `json:"filename"`
	Path       string   `json:"path"`
	ScriptKind int32    `json:"script_kind"`
	JSX        bool     `json:"jsx"`
	Force      bool     `json:"force"`
	Operations []string `json:"operations"`
}

var operations = []string{"parse", "node_index_before", "encode_source_file", "node_index_after"}

func decode(raw []byte) (request, []byte, error) {
	var r request
	// Every named request value is scalar or an array of strings. Checking the
	// top-level keys before typed decoding rejects duplicates and missing fields.
	d := json.NewDecoder(bytes.NewReader(raw))
	token, err := d.Token()
	if err != nil || token != json.Delim('{') {
		return r, nil, fmt.Errorf("expected request object")
	}
	keys := map[string]bool{}
	for d.More() {
		key, err := d.Token()
		if err != nil {
			return r, nil, err
		}
		name, ok := key.(string)
		if !ok || keys[name] {
			return r, nil, fmt.Errorf("invalid or duplicate request key")
		}
		keys[name] = true
		var value json.RawMessage
		if err := d.Decode(&value); err != nil {
			return r, nil, err
		}
	}
	if _, err := d.Token(); err != nil {
		return r, nil, err
	}
	if _, err := d.Token(); err != io.EOF {
		return r, nil, fmt.Errorf("trailing request data")
	}
	expected := []string{"version", "id", "primary", "op", "source_hex", "filename", "path", "script_kind", "jsx", "force", "operations"}
	if len(keys) != len(expected) {
		return r, nil, fmt.Errorf("unexpected request fields")
	}
	for _, key := range expected {
		if !keys[key] {
			return r, nil, fmt.Errorf("missing request field %s", key)
		}
	}
	if err := json.Unmarshal(raw, &r); err != nil {
		return r, nil, err
	}
	// This wrapper intentionally implements only the named fuzz schedule.
	if r.Version != 1 || r.ID != "r5/parser" || r.Primary != nil || r.Op != "parse" ||
		r.Filename != "/s06/fixture.ts" || r.Path != r.Filename || r.ScriptKind != 3 || r.JSX || r.Force ||
		!slices.Equal(r.Operations, operations) || len(r.SourceHex) > 8192 {
		return r, nil, fmt.Errorf("unsupported named parser request")
	}
	source, err := hex.DecodeString(r.SourceHex)
	if err != nil || hex.EncodeToString(source) != r.SourceHex {
		return r, nil, fmt.Errorf("invalid canonical source hex")
	}
	return r, source, nil
}

type session struct {
	id          string
	seq, stages int
	output      *json.Encoder
}

func (s *session) frame(value object) {
	value["version"], value["id"] = 1, s.id
	if err := s.output.Encode(value); err != nil {
		panic(err)
	}
}

func (s *session) observe(stage, kind string, value object) {
	s.frame(object{"tag": "observation", "seq": s.seq, "stage": stage, "kind": kind, "value": value})
	s.seq++
}

func (s *session) stage(name string, action func() error) (ok bool) {
	outcome, message := "ok", ""
	defer func() {
		if recovered := recover(); recovered != nil {
			outcome, message, ok = "panic", fmt.Sprint(recovered), false
		}
		s.frame(object{"tag": "stage", "stage": name, "outcome": outcome, "message_hex": hex.EncodeToString([]byte(message))})
		s.stages++
	}()
	if err := action(); err != nil {
		outcome, message = "error", err.Error()
		return false
	}
	return true
}

func (s *session) table(stage string, table *encoder.NodeIndexTable) {
	indices := map[*ast.Node]int{}
	for index, node := range table.Nodes {
		if node != nil {
			indices[node] = index
		}
	}
	s.observe(stage, "table", object{"length": len(table.Nodes)})
	for index, node := range table.Nodes {
		var value any
		if node != nil {
			value = object{"kind": node.Kind, "pos": node.Pos(), "end": node.End(), "flags": node.Flags,
				"parent": indices[node.Parent], "lookup": table.GetIndex(node)}
		}
		s.observe(stage, "node", object{"index": index, "node": value})
	}
	absent := ast.NewNodeFactory(ast.NodeFactoryHooks{}).NewToken(ast.KindUnknown)
	s.observe(stage, "absent_lookup", object{"index": table.GetIndex(absent)})
}

func execute(s *session, r request, source []byte) {
	var file *ast.SourceFile
	if !s.stage("parse", func() error {
		file = parser.ParseSourceFile(ast.SourceFileParseOptions{
			FileName: r.Filename, Path: tspath.Path(r.Path),
			ExternalModuleIndicatorOptions: ast.ExternalModuleIndicatorOptions{JSX: r.JSX, Force: r.Force},
		}, string(source), core.ScriptKind(r.ScriptKind))
		node := file.AsNode()
		s.observe("parse", "source_file", object{"kind": node.Kind, "pos": node.Pos(), "end": node.End(), "flags": node.Flags,
			"node_count": file.NodeCount, "text_count": file.TextCount, "identifier_count": file.IdentifierCount,
			"script_kind": file.ScriptKind, "language_variant": file.LanguageVariant, "declaration_file": file.IsDeclarationFile,
			"hash": encoder.SourceFileHash(file)})
		for _, collection := range []struct {
			name  string
			items []*ast.Diagnostic
		}{{"parse", file.Diagnostics()}, {"js", file.JSDiagnostics()}, {"jsdoc", file.JSDocDiagnostics()}} {
			for index, diagnostic := range collection.items {
				args := make([]string, len(diagnostic.MessageArgs()))
				for index, arg := range diagnostic.MessageArgs() {
					args[index] = hex.EncodeToString([]byte(arg))
				}
				s.observe("parse", "diagnostic", object{"collection": collection.name, "index": index,
					"code": diagnostic.Code(), "category": diagnostic.Category(), "pos": diagnostic.Pos(), "end": diagnostic.End(),
					"key_hex": hex.EncodeToString([]byte(diagnostic.MessageKey())), "text_hex": hex.EncodeToString([]byte(diagnostic.MessageText())), "args_hex": args})
			}
		}
		return nil
	}) {
		return
	}
	var before, encodedTable *encoder.NodeIndexTable
	if !s.stage("node_index_before", func() error {
		before = encoder.GetNodeIndexTable(file)
		independent := encoder.BuildNodeIndexTable(file)
		again := encoder.GetNodeIndexTable(file)
		s.observe("node_index_before", "identity", object{"independent_order_equal": slices.Equal(before.Nodes, independent.Nodes), "cache_reused": before == again})
		s.table("node_index_before", before)
		return nil
	}) {
		return
	}
	if !s.stage("encode_source_file", func() error {
		var encoded []byte
		var err error
		encoded, encodedTable, err = encoder.EncodeSourceFile(file)
		if err != nil {
			return err
		}
		s.observe("encode_source_file", "bytes", object{"length": len(encoded)})
		for offset := 0; offset < len(encoded); offset += 65536 {
			end := min(offset+65536, len(encoded))
			s.observe("encode_source_file", "chunk", object{"offset": offset, "hex": hex.EncodeToString(encoded[offset:end])})
		}
		return nil
	}) {
		return
	}
	s.stage("node_index_after", func() error {
		after := encoder.GetNodeIndexTable(file)
		s.observe("node_index_after", "identity", object{"before_reused": before == after, "encoded_reused": encodedTable == after})
		s.table("node_index_after", after)
		return nil
	})
}

func run() error {
	input := bufio.NewReaderSize(os.Stdin, 32768)
	raw, err := input.ReadSlice('\n')
	if err != nil || len(raw) > 16384 {
		return fmt.Errorf("missing/oversized LF-terminated request")
	}
	if _, err := input.ReadByte(); err != io.EOF {
		return fmt.Errorf("named parser process accepts exactly one request")
	}
	r, source, err := decode(raw)
	if err != nil {
		return err
	}
	s := session{id: r.ID, output: json.NewEncoder(os.Stdout)}
	s.frame(object{"tag": "begin", "op": r.Op})
	execute(&s, r, source)
	s.frame(object{"tag": "end", "observations": s.seq, "stages": s.stages})
	return nil
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "R5 parser adapter:", err)
		os.Exit(2)
	}
}
