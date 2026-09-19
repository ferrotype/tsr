package tsbaseline

import (
	"encoding/hex"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/baseline"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/harnessutil"
)

// This records only the public pulls made by the original baseline walker,
// not every internal checker read. Source positions retain Go byte semantics.
type S08Query struct {
	Operation     string `json:"operation"`
	File          string `json:"file"`
	Kind          int16  `json:"kind"`
	Pos           int    `json:"pos"`
	End           int    `json:"end"`
	ResultFlags   uint32 `json:"result_flags,omitempty"`
	TypeID        uint32 `json:"type_id,omitempty"`
	Flags         uint32 `json:"flags,omitempty"`
	InternalFlags uint32 `json:"internal_flags,omitempty"`
	Absent        bool   `json:"absent,omitempty"`
}

var S08Queries []S08Query

// E2 observes default TypeToString only after the ordinary type/symbol walk.
// Holding the already-returned types avoids replaying type queries or changing
// the baseline walk's checker/cache order.
var S08CollectTypeStrings bool
var S08TypeStrings []map[string]any
var s08PendingTypeStrings []func() map[string]any

func s08Query(operation string, walker *typeWriterWalker, node *ast.Node) S08Query {
	return S08Query{Operation: operation, File: walker.currentSourceFile.FileName(), Kind: int16(node.Kind), Pos: node.Pos(), End: node.End()}
}

func s08GetTypeAtLocation(walker *typeWriterWalker, c *checker.Checker, node *ast.Node) *checker.Type {
	result := c.GetTypeAtLocation(node)
	restore := harnessutil.S08EnterObservation()
	q := s08Query("GetTypeAtLocation", walker, node)
	if result == nil {
		q.Absent = true
	} else {
		q.ResultFlags = uint32(result.Flags())
		q.TypeID = uint32(result.Id())
	}
	S08Queries = append(S08Queries, q)
	if S08CollectTypeStrings && result != nil {
		stamp := s08Query("TypeToString", walker, node)
		s08PendingTypeStrings = append(s08PendingTypeStrings, func() map[string]any {
			return map[string]any{"operation":stamp.Operation, "file":stamp.File, "kind":stamp.Kind,
				"pos":stamp.Pos, "end":stamp.End, "text_hex":hex.EncodeToString([]byte(c.TypeToString(result)))}
		})
	}
	restore()
	return result
}

func s08RecordDisplay(operation string, walker *typeWriterWalker, node *ast.Node, typ *checker.Type, flags, internalFlags uint32) {
	restore := harnessutil.S08EnterObservation()
	q := s08Query(operation, walker, node)
	if typ != nil {
		q.TypeID = uint32(typ.Id())
	}
	q.Flags = flags
	q.InternalFlags = internalFlags
	S08Queries = append(S08Queries, q)
	restore()
}

func s08GetSymbolAtLocation(walker *typeWriterWalker, c *checker.Checker, node *ast.Node) *ast.Symbol {
	result := c.GetSymbolAtLocation(node)
	restore := harnessutil.S08EnterObservation()
	q := s08Query("GetSymbolAtLocation", walker, node)
	q.Absent = result == nil
	S08Queries = append(S08Queries, q)
	restore()
	return result
}

type S08Baseline struct {
	State   string  `json:"state"`
	TextHex *string `json:"text_hex,omitempty"`
}

func S08BaselineValue(value string) S08Baseline {
	restore := harnessutil.S08EnterObservation()
	if value == baseline.NoContent {
		restore()
		return S08Baseline{State: "no_content"}
	}
	text := hex.EncodeToString([]byte(value))
	restore()
	return S08Baseline{State: "content", TextHex: &text}
}

// Same walker instance and type-before-symbol order as DoTypeAndSymbolBaseline.
// Only the reference-file comparison/old-Strada fixups are omitted.
func S08TypeSymbolBaselines(program compiler.ProgramLike, files []*harnessutil.TestFile, header string, hadErrors bool) (S08Baseline, S08Baseline) {
	s08PendingTypeStrings = nil
	S08TypeStrings = []map[string]any{}
	defer func() { s08PendingTypeStrings = nil }()
	walker := newTypeWriterWalker(program, hadErrors)
	types := generateBaseline(files, walker, header, false)
	symbols := generateBaseline(files, walker, header, true)
	for _, display := range s08PendingTypeStrings { S08TypeStrings = append(S08TypeStrings, display()) }
	return S08BaselineValue(types), S08BaselineValue(symbols)
}
