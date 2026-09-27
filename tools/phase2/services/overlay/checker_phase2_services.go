package checker

// Phase 2 C5.5 diagnostic overlay (never part of the pinned sources): records
// every call the language service makes into a checker, with its arguments
// and results, for the Rust replay. Instrumented entry points call
// phase2Enter first and phase2Call.exit last; only a call that enters a
// checker from outside (depth zero) is recorded.
//
// The recorder only reads fields. It never calls the node builder, a
// comparator, a formatter or a lazily computed query, and never requests an id
// the pin assigns lazily (symbol and node ids): values are identified by
// recorder-local tokens with the fields that construct them.

import (
	"bufio"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"reflect"
	"sort"
	"strings"
	"sync"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/jsnum"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

type phase2Recorder struct {
	mu       sync.Mutex
	file     *os.File
	out      *bufio.Writer
	test     *core.Phase2ServicesTest
	seq      int
	checkers map[*Checker]*phase2CheckerState
	programs map[Program]string
	objects  map[any]string
	blobs    map[string]bool
	next     map[string]int
}

type phase2CheckerState struct {
	token  string
	depth  int
	tokens map[any]string
	next   map[string]int
}

type phase2Call struct {
	st     *phase2CheckerState
	nested bool
	op     string
	args   []any
	before any
	defs   map[string]any
}

var phase2 = phase2Open()

func phase2Open() *phase2Recorder {
	path := os.Getenv("PHASE2_SERVICES_RECORD")
	if path == "" {
		return nil
	}
	file, err := os.Create(path)
	if err != nil {
		panic(err)
	}
	r := &phase2Recorder{file: file, out: bufio.NewWriterSize(file, 1<<20)}
	r.reset()
	core.Phase2ServicesOnEnd(func(*core.Phase2ServicesTest) {
		r.mu.Lock()
		defer r.mu.Unlock()
		r.emit(map[string]any{"e": "end"})
		r.test = nil
		r.reset()
		if err := r.out.Flush(); err != nil {
			panic(err)
		}
	})
	return r
}

func (r *phase2Recorder) reset() {
	r.checkers = make(map[*Checker]*phase2CheckerState)
	r.programs = make(map[Program]string)
	r.objects = make(map[any]string)
	r.blobs = make(map[string]bool)
	r.next = make(map[string]int)
}

func (r *phase2Recorder) emit(event map[string]any) {
	r.seq++
	event["n"] = r.seq
	data, err := json.Marshal(event)
	if err != nil {
		panic(err)
	}
	r.out.Write(data)
	r.out.WriteByte('\n')
}

// A call outside a fourslash test (package init, other tests) is not recorded.
func (r *phase2Recorder) currentTest() bool {
	test := core.Phase2ServicesCurrentTest()
	if test == nil {
		return false
	}
	if test != r.test {
		r.test = test
		r.reset()
		r.emit(map[string]any{"e": "test", "name": test.Name, "files": test.Files, "symlinks": test.Symlinks})
	}
	return true
}

func (r *phase2Recorder) checkerState(c *Checker) *phase2CheckerState {
	if st, ok := r.checkers[c]; ok {
		return st
	}
	r.next["c"]++
	st := &phase2CheckerState{token: fmt.Sprintf("c%d", r.next["c"]), tokens: make(map[any]string), next: make(map[string]int)}
	r.checkers[c] = st
	program, ok := r.programs[c.program]
	if !ok {
		r.next["p"]++
		program = fmt.Sprintf("p%d", r.next["p"])
		r.programs[c.program] = program
		event := map[string]any{"e": "program", "program": program, "cwd": c.program.GetCurrentDirectory(),
			"case_sensitive": c.program.UseCaseSensitiveFileNames(), "go_type": fmt.Sprintf("%T", c.program)}
		// A compiler program: its command line. Other checker programs (the
		// auto-import registry's alias resolver) have none.
		compiler, isCompiler := c.program.(interface {
			CommandLine() *tsoptions.ParsedCommandLine
			IsSourceFromProjectReference(path tspath.Path) bool
		})
		if isCompiler {
			config := compiler.CommandLine()
			event["roots"] = config.FileNames()
			event["options"] = config.CompilerOptions()
			event["config_file"] = config.ConfigName()
			references := make([]any, 0)
			for _, reference := range config.ProjectReferences() {
				references = append(references, map[string]any{"path": reference.Path, "original_path": reference.OriginalPath, "circular": reference.Circular})
			}
			event["project_references"] = references
			event["content_mappers"] = config.ContentMapperExtensions()
		}
		// The snapshot: every file the program checks, with the digest of the
		// text it checks (edits change it); a text is written once per test.
		files := make([]any, 0, len(c.program.SourceFiles()))
		for _, file := range c.program.SourceFiles() {
			entry := map[string]any{"name": file.FileName()}
			if isCompiler && compiler.IsSourceFromProjectReference(file.Path()) {
				entry["from_project_reference"] = true
			}
			if c.program.IsSourceFileDefaultLibrary(file.Path()) {
				entry["default_library"] = true
			} else {
				text := file.Text()
				sum := sha256.Sum256([]byte(text))
				key := hex.EncodeToString(sum[:])
				entry["text_sha256"] = key
				if !r.blobs[key] {
					r.blobs[key] = true
					r.emit(map[string]any{"e": "blob", "sha256": key, "text": hex.EncodeToString([]byte(text))})
				}
			}
			files = append(files, entry)
		}
		event["source_files"] = files
		r.emit(event)
	}
	r.emit(map[string]any{"e": "checker", "checker": st.token, "program": program})
	return st
}

func phase2Enter(c *Checker, op string, args []any) *phase2Call {
	r := phase2
	if r == nil || c == nil {
		return nil
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if !r.currentTest() {
		return nil
	}
	st := r.checkerState(c)
	st.depth++
	if st.depth > 1 {
		return &phase2Call{st: st, nested: true}
	}
	call := &phase2Call{st: st, op: op, defs: make(map[string]any)}
	call.args = make([]any, len(args))
	for i, arg := range args {
		call.args[i] = r.describe(c, call, arg)
	}
	return call
}

func (call *phase2Call) exit(c *Checker, args []any, results []any, panicked any) {
	r := phase2
	r.mu.Lock()
	defer r.mu.Unlock()
	call.st.depth--
	if call.nested || r.test == nil {
		return
	}
	event := map[string]any{"e": "call", "checker": call.st.token, "op": call.op, "args": call.args}
	if panicked != nil {
		event["panic"] = fmt.Sprint(panicked)
	} else {
		described := make([]any, len(results))
		for i, result := range results {
			if phase2Unordered[call.op] {
				result = phase2SortSymbols(result)
			}
			described[i] = r.describe(c, call, result)
		}
		event["results"] = described
		// Arguments the call may change (a verbosity context) are read again.
		after := make([]any, 0)
		for _, arg := range args {
			if v, ok := arg.(*VerbosityContext); ok && v != nil {
				after = append(after, r.describe(c, call, arg))
			}
		}
		if len(after) != 0 {
			event["args_after"] = after
		}
	}
	if len(call.defs) != 0 {
		event["defs"] = call.defs
	}
	r.emit(event)
}

// Results the pin collects from a symbol table carry no order.
var phase2Unordered = map[string]bool{
	"Checker.GetSymbolsInScope":                true,
	"Checker.GetExportsOfModule":               true,
	"Checker.GetExportsAndPropertiesOfModule":  true,
	"Checker.GetAllPossiblePropertiesOfTypes":  true,
	"Checker.ForEachExportAndPropertyOfModule": true,
	"Checker.GetAmbientModules":                true,
}

// phase2Order returns a map-ordered symbol list in the recorder's fixed order.
// It runs in every recording-build call, recorded or nested, so the checker
// and the language service see one order throughout.
func phase2Order(symbols []*ast.Symbol) []*ast.Symbol {
	if phase2 == nil || symbols == nil {
		return symbols
	}
	return phase2SortSymbols(symbols).([]*ast.Symbol)
}

func phase2SortSymbols(value any) any {
	symbols, ok := value.([]*ast.Symbol)
	if !ok {
		return value
	}
	sorted := append([]*ast.Symbol(nil), symbols...)
	key := func(s *ast.Symbol) string {
		position := ""
		if len(s.Declarations) != 0 {
			d := s.Declarations[0]
			file := ast.GetSourceFileOfNode(d)
			name := ""
			if file != nil {
				name = file.FileName()
			}
			position = fmt.Sprintf("%s:%09d:%09d", name, d.Pos(), d.End())
		}
		return fmt.Sprintf("%s\x00%s\x00%08x\x00%08x", s.Name, position, uint32(s.Flags), uint32(s.CheckFlags))
	}
	sort.SliceStable(sorted, func(i, j int) bool { return key(sorted[i]) < key(sorted[j]) })
	return sorted
}

func (r *phase2Recorder) object(value any, prefix string) string {
	if token, ok := r.objects[value]; ok {
		return token
	}
	r.next[prefix]++
	token := fmt.Sprintf("%s%d", prefix, r.next[prefix])
	r.objects[value] = token
	return token
}

func (r *phase2Recorder) token(c *Checker, call *phase2Call, value any, prefix string) (string, bool) {
	st := r.checkerState(c)
	if token, ok := st.tokens[value]; ok {
		return token, false
	}
	st.next[prefix]++
	token := fmt.Sprintf("%s%d", prefix, st.next[prefix])
	st.tokens[value] = token
	return token, true
}

func (r *phase2Recorder) describe(c *Checker, call *phase2Call, value any) any {
	switch v := value.(type) {
	case nil:
		return nil
	case *Type:
		if v == nil {
			return nil
		}
		return r.typeRef(c, call, v)
	case *ast.Symbol:
		if v == nil {
			return nil
		}
		return r.symbolRef(c, call, v)
	case *Signature:
		if v == nil {
			return nil
		}
		return r.signatureRef(c, call, v)
	case *IndexInfo:
		if v == nil {
			return nil
		}
		return map[string]any{"index": map[string]any{"key": r.describe(c, call, v.keyType), "value": r.describe(c, call, v.valueType), "readonly": v.isReadonly, "declaration": r.nodeRef(v.declaration)}}
	case *TypePredicate:
		if v == nil {
			return nil
		}
		return map[string]any{"predicate": map[string]any{"kind": int(v.kind), "index": v.parameterIndex, "name": v.parameterName, "type": r.describe(c, call, v.t)}}
	case *ast.Node:
		if v == nil {
			return nil
		}
		return r.nodeRef(v)
	case *ast.Diagnostic:
		if v == nil {
			return nil
		}
		return phase2Diagnostic(v)
	case *VerbosityContext:
		if v == nil {
			return nil
		}
		return map[string]any{"verbosity": map[string]any{"level": v.Level, "max_truncation_length": v.MaxTruncationLength, "can_increase": v.CanIncreaseVerbosity, "truncated": v.Truncated}}
	case *NodeBuilder:
		if v == nil {
			return nil
		}
		return map[string]any{"builder": r.object(v, "b")}
	case *EmitResolver:
		if v == nil {
			return nil
		}
		return map[string]any{"resolver": r.object(v, "r")}
	case *Checker:
		if v == nil {
			return nil
		}
		return map[string]any{"checker": r.checkerState(v).token}
	case *ast.SourceFile:
		if v == nil {
			return nil
		}
		return map[string]any{"source_file": v.FileName()}
	case interface{ AsNode() *ast.Node }:
		// A node by its concrete type (*ast.Identifier, *ast.ImportDeclaration).
		if rv := reflect.ValueOf(v); rv.Kind() == reflect.Pointer && rv.IsNil() {
			return nil
		}
		return r.nodeRef(v.AsNode())
	case context.Context:
		if v == nil {
			return nil
		}
		// Whether the request was already canceled when the call began.
		return map[string]any{"context": map[string]any{"canceled": v.Err() != nil}}
	case string:
		return phase2Name(v)
	case bool:
		return v
	case jsnum.Number:
		return map[string]any{"number": phase2Number(v)}
	case jsnum.PseudoBigInt:
		return map[string]any{"bigint": v.Base10Value, "negative": v.Negative}
	}
	rv := reflect.ValueOf(value)
	switch rv.Kind() {
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return rv.Int()
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return rv.Uint()
	case reflect.Float32, reflect.Float64:
		return map[string]any{"number": phase2Number(jsnum.Number(rv.Float()))}
	case reflect.Slice, reflect.Array:
		if rv.Kind() == reflect.Slice && rv.IsNil() {
			return nil
		}
		list := make([]any, rv.Len())
		for i := range list {
			list[i] = r.describe(c, call, rv.Index(i).Interface())
		}
		return list
	case reflect.Func:
		return map[string]any{"opaque": "func"}
	case reflect.Map:
		return map[string]any{"opaque": "map", "len": rv.Len()}
	case reflect.Interface:
		if rv.IsNil() {
			return nil
		}
	case reflect.Pointer:
		if rv.IsNil() {
			return nil
		}
		return map[string]any{"opaque": rv.Type().String(), "id": r.object(value, "o")}
	case reflect.Struct:
		fields := map[string]any{}
		for i := 0; i < rv.NumField(); i++ {
			field := rv.Type().Field(i)
			if field.IsExported() {
				fields[field.Name] = r.describe(c, call, rv.Field(i).Interface())
			}
		}
		return map[string]any{"struct": rv.Type().String(), "fields": fields}
	}
	return map[string]any{"opaque": fmt.Sprintf("%T", value)}
}

// Internal names of private members, unique symbols and pattern ambient
// modules carry process-wide symbol ids, which other goroutines advance; the
// recorder writes them without the id.
func phase2Name(name string) string {
	if strings.HasPrefix(name, "\xfe#") {
		i := 2
		for i < len(name) && name[i] >= '0' && name[i] <= '9' {
			i++
		}
		if i > 2 && i < len(name) && name[i] == '@' {
			return "\xfe#" + name[i:]
		}
	}
	// Unique symbols ("\xfe@<name>@<id>") and pattern ambient modules
	// ("\xfe\"<pattern>\"pattern@<id>") end with their symbol id.
	if strings.HasPrefix(name, "\xfe") {
		if j := strings.LastIndexByte(name, '@'); j > 1 && j+1 < len(name) {
			digits := true
			for _, b := range []byte(name[j+1:]) {
				digits = digits && b >= '0' && b <= '9'
			}
			if digits {
				return name[:j+1]
			}
		}
	}
	return name
}

func phase2Number(n jsnum.Number) any {
	return n.String()
}

func (r *phase2Recorder) nodeRef(node *ast.Node) any {
	if node == nil {
		return nil
	}
	file := ast.GetSourceFileOfNode(node)
	if file != nil && node.Flags&ast.NodeFlagsSynthesized == 0 {
		return map[string]any{"node": map[string]any{"file": file.FileName(), "pos": node.Pos(), "end": node.End(), "kind": node.Kind.String()}}
	}
	return map[string]any{"synthetic": phase2Tree(node)}
}

// phase2Tree is the structure of a synthesized node: its kind, its text for
// names and literals, and its children in visiting order.
func phase2Tree(node *ast.Node) any {
	tree := map[string]any{"kind": node.Kind.String()}
	switch node.Kind {
	case ast.KindIdentifier, ast.KindPrivateIdentifier, ast.KindStringLiteral, ast.KindNumericLiteral,
		ast.KindBigIntLiteral, ast.KindNoSubstitutionTemplateLiteral, ast.KindTemplateHead,
		ast.KindTemplateMiddle, ast.KindTemplateTail, ast.KindRegularExpressionLiteral:
		tree["text"] = node.Text()
	}
	if file := ast.GetSourceFileOfNode(node); file != nil && node.Flags&ast.NodeFlagsSynthesized == 0 {
		tree["source"] = map[string]any{"file": file.FileName(), "pos": node.Pos(), "end": node.End()}
	}
	var children []any
	node.ForEachChild(func(child *ast.Node) bool {
		children = append(children, phase2Tree(child))
		return false
	})
	if len(children) != 0 {
		tree["children"] = children
	}
	return tree
}

func phase2Diagnostic(d *ast.Diagnostic) any {
	result := map[string]any{"code": d.Code(), "category": int(d.Category()), "key": string(d.MessageKey()), "arguments": d.MessageArgs(), "pos": d.Pos(), "end": d.End()}
	if text := d.MessageText(); text != "" {
		result["message"] = text
	}
	if d.File() != nil {
		result["file"] = d.File().FileName()
	}
	if chain := d.MessageChain(); len(chain) != 0 {
		list := make([]any, len(chain))
		for i, item := range chain {
			list[i] = phase2Diagnostic(item)
		}
		result["chain"] = list
	}
	if related := d.RelatedInformation(); len(related) != 0 {
		list := make([]any, len(related))
		for i, item := range related {
			list[i] = phase2Diagnostic(item)
		}
		result["related"] = list
	}
	return result
}

func (r *phase2Recorder) symbolRef(c *Checker, call *phase2Call, s *ast.Symbol) any {
	token, fresh := r.token(c, call, s, "S")
	if fresh {
		def := map[string]any{"name": phase2Name(s.Name), "flags": uint32(s.Flags), "check_flags": uint32(s.CheckFlags)}
		switch s {
		case c.unknownSymbol:
			def["builtin"] = "unknown"
		case c.undefinedSymbol:
			def["builtin"] = "undefined"
		case c.argumentsSymbol:
			def["builtin"] = "arguments"
		case c.requireSymbol:
			def["builtin"] = "require"
		case c.globalThisSymbol:
			def["builtin"] = "globalThis"
		}
		declarations := make([]any, len(s.Declarations))
		for i, d := range s.Declarations {
			declarations[i] = r.nodeRef(d)
		}
		def["declarations"] = declarations
		def["value_declaration"] = r.nodeRef(s.ValueDeclaration)
		call.defs[token] = def
		if s.Parent != nil {
			def["parent"] = r.symbolRef(c, call, s.Parent)
		}
	}
	return map[string]any{"symbol": token}
}

func (r *phase2Recorder) signatureRef(c *Checker, call *phase2Call, s *Signature) any {
	token, fresh := r.token(c, call, s, "G")
	if fresh {
		def := map[string]any{"flags": uint32(s.flags), "min_argument_count": s.minArgumentCount, "declaration": r.nodeRef(s.declaration)}
		call.defs[token] = def
		def["type_parameters"] = r.describe(c, call, s.typeParameters)
		def["parameters"] = r.describe(c, call, s.parameters)
		def["this_parameter"] = r.describe(c, call, s.thisParameter)
		def["target"] = r.describe(c, call, s.target)
		def["composite"] = s.composite != nil
	}
	return map[string]any{"signature": token}
}

// Object flags that record how a type was built; the flags that cache a
// computed answer (the ...Computed and Is... pairs) depend on which queries
// ran before and are left out.
const phase2StableObjectFlags = ObjectFlagsClass | ObjectFlagsInterface | ObjectFlagsReference |
	ObjectFlagsTuple | ObjectFlagsAnonymous | ObjectFlagsMapped | ObjectFlagsInstantiated |
	ObjectFlagsObjectLiteral | ObjectFlagsEvolvingArray | ObjectFlagsObjectLiteralPatternWithComputedProperties |
	ObjectFlagsReverseMapped | ObjectFlagsJsxAttributes | ObjectFlagsJSLiteral | ObjectFlagsFreshLiteral |
	ObjectFlagsArrayLiteral | ObjectFlagsPrimitiveUnion | ObjectFlagsContainsWideningType |
	ObjectFlagsContainsObjectOrArrayLiteral | ObjectFlagsNonInferrableType | ObjectFlagsContainsSpread |
	ObjectFlagsObjectRestType | ObjectFlagsInstantiationExpressionType | ObjectFlagsSingleSignatureType

func (r *phase2Recorder) typeRef(c *Checker, call *phase2Call, t *Type) any {
	token, fresh := r.token(c, call, t, "T")
	if !fresh {
		return map[string]any{"type": token}
	}
	def := map[string]any{"id": uint32(t.id), "flags": uint32(t.flags), "object_flags": uint32(t.objectFlags & phase2StableObjectFlags)}
	if t.data != nil {
		def["kind"] = reflect.TypeOf(t.data).Elem().Name()
	}
	call.defs[token] = def
	if t.symbol != nil {
		def["symbol"] = r.symbolRef(c, call, t.symbol)
	}
	if t.alias != nil {
		def["alias"] = map[string]any{"symbol": r.describe(c, call, t.alias.symbol), "arguments": r.describe(c, call, t.alias.typeArguments)}
	}
	switch data := t.data.(type) {
	case *IntrinsicType:
		def["intrinsic"] = data.intrinsicName
	case *LiteralType:
		switch value := data.value.(type) {
		case string:
			def["value"] = map[string]any{"string": value}
		case jsnum.Number:
			def["value"] = map[string]any{"number": phase2Number(value)}
		case bool:
			def["value"] = map[string]any{"boolean": value}
		case jsnum.PseudoBigInt:
			def["value"] = map[string]any{"bigint": value.Base10Value, "negative": value.Negative}
		}
	case *UniqueESSymbolType:
		def["unique"] = phase2Name(data.name)
	case *UnionType:
		def["types"] = r.describe(c, call, data.types)
	case *IntersectionType:
		def["types"] = r.describe(c, call, data.types)
	case *TypeParameter:
		def["is_this_type"] = data.isThisType
		def["target"] = r.describe(c, call, data.target)
	case *IndexType:
		def["target"] = r.describe(c, call, data.target)
		def["index_flags"] = uint32(data.indexFlags)
	case *IndexedAccessType:
		def["object"] = r.describe(c, call, data.objectType)
		def["index"] = r.describe(c, call, data.indexType)
	case *ConditionalType:
		def["root"] = r.nodeRef(data.root.node.AsNode())
		def["check"] = r.describe(c, call, data.checkType)
		def["extends"] = r.describe(c, call, data.extendsType)
	case *SubstitutionType:
		def["base"] = r.describe(c, call, data.baseType)
		def["constraint"] = r.describe(c, call, data.constraint)
	case *StringMappingType:
		def["target"] = r.describe(c, call, data.target)
	case *TemplateLiteralType:
		def["texts"] = data.texts
		def["types"] = r.describe(c, call, data.types)
	case *MappedType:
		def["declaration"] = r.nodeRef(data.declaration.AsNode())
	case *TupleType:
		flags := make([]any, len(data.elementInfos))
		for i, info := range data.elementInfos {
			flags[i] = uint32(info.flags)
		}
		def["element_flags"] = flags
		def["readonly"] = data.readonly
	case *InterfaceType:
		// The declared type of a class or interface: its symbol names it.
	case *TypeReference:
		def["target"] = r.describe(c, call, data.target)
		if data.resolvedTypeArguments != nil {
			def["arguments"] = r.describe(c, call, data.resolvedTypeArguments)
		}
		if data.node != nil {
			def["node"] = r.nodeRef(data.node)
		}
	}
	return map[string]any{"type": token}
}
