package budget

import (
	"context"
	"encoding/json"
	"fmt"
	"runtime"
	"strconv"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/locale"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/vfs"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/iovfs"
)

type parseHost struct {
	fs  vfs.FS
	cwd string
}

func (h parseHost) FS() vfs.FS                  { return h.fs }
func (h parseHost) GetCurrentDirectory() string { return h.cwd }

type Write struct {
	Name Bytes `json:"name"`
	Text Bytes `json:"text"`
}
type Source struct {
	Name            Bytes  `json:"name"`
	Path            Bytes  `json:"path"`
	Text            Bytes  `json:"text"`
	DefaultLibrary  bool   `json:"default_library"`
	NodeCount       string `json:"node_count"`
	IdentifierCount string `json:"identifier_count"`
}
type Repopulate struct {
	Kind            int   `json:"kind"`
	ModuleReference Bytes `json:"module_reference"`
	Mode            int   `json:"mode"`
	PackageName     Bytes `json:"package_name"`
}
type Diagnostic struct {
	File            *int         `json:"file"`
	Pos             string       `json:"pos"`
	End             string       `json:"end"`
	Code            int32        `json:"code"`
	Category        int          `json:"category"`
	MessageKey      Bytes        `json:"message_key"`
	MessageText     Bytes        `json:"message_text"`
	MessageArgs     []Bytes      `json:"message_args"`
	Localized       Bytes        `json:"localized"`
	Chain           []Diagnostic `json:"chain"`
	Related         []Diagnostic `json:"related"`
	Source          Bytes        `json:"source"`
	Unnecessary     bool         `json:"unnecessary"`
	Deprecated      bool         `json:"deprecated"`
	SkippedOnNoEmit bool         `json:"skipped_on_no_emit"`
	Repopulate      *Repopulate  `json:"repopulate"`
}
type Observation struct {
	Outcome         string       `json:"outcome"`
	Roots           []Bytes      `json:"roots"`
	OptionsJSON     Bytes        `json:"options_json"`
	Sources         []Source     `json:"sources"`
	Diagnostics     []Diagnostic `json:"diagnostics"`
	Checkers        string       `json:"checkers"`
	Types           string       `json:"types"`
	Nodes           string       `json:"nodes"`
	Identifiers     string       `json:"identifiers"`
	Writes          []Write      `json:"writes"`
	EmittedFiles    []Bytes      `json:"emitted_files"`
	EmitSkipped     *bool        `json:"emit_skipped"`
	EmitDiagnostics []Diagnostic `json:"emit_diagnostics"`
}

// ObservationHeader shares scalar values with the frozen post-checkpoint
// observation while keeping the large byte rows out of its JSON response.
type ObservationHeader struct {
	Observation
	Sources     []Source `json:"sources,omitempty"`
	Writes      []Write  `json:"writes,omitempty"`
	SourceCount string   `json:"source_count"`
	WriteCount  string   `json:"write_count"`
}
type retainedWrite struct{ name, text string }
type Engine struct {
	id, state       string
	input           *Input
	snapshot        *snapshot
	fs              vfs.FS
	config          *tsoptions.ParsedCommandLine
	program         *compiler.Program
	diagnostics     []*ast.Diagnostic
	writes          []retainedWrite
	emit            *compiler.EmitResult
	failed          bool
	preparation     *streamPreparation
	protocolVersion int
	observation     *Observation
}

func (e *Engine) ExecuteCommand(raw []byte) (response Response, err error) {
	var version struct {
		Version int `json:"version"`
	}
	if json.Unmarshal(raw, &version) == nil && version.Version == 2 {
		return e.streamCommand(raw)
	}
	if e.protocolVersion == 2 {
		e.failed = true
		return Response{Version: 1}, fmt.Errorf("cannot mix protocol versions in retained operation")
	}
	return e.executeV1(raw)
}
func (e *Engine) executeV1(raw []byte) (response Response, err error) {
	response.Version = 1
	if e.failed {
		return response, fmt.Errorf("budget consumer terminal after failure")
	}
	defer func() {
		if err != nil {
			e.failed = true
		}
	}()
	c, err := Decode(raw)
	if err != nil {
		return response, err
	}
	response.ID = c.ID
	if c.Command != "prepare" && (e.state == "" || e.id != c.ID) {
		return response, fmt.Errorf("unknown retained operation")
	}
	switch c.Command {
	case "prepare":
		if e.state != "" || c.Input == nil {
			return response, fmt.Errorf("prepare requires empty consumer and input")
		}
		if c.Input.Operation != "check" && c.Input.Operation != "check_emit" {
			return response, fmt.Errorf("unsupported operation")
		}
		s, err := newSnapshot(c.Input)
		if err != nil {
			return response, err
		}
		e.id = c.ID
		e.state = "prepared"
		// Snapshot construction owns its file bytes; discard the decoded entry
		// payload before execute/checkpoint, matching the Rust frontend.
		c.Input.Entries = nil
		e.input = c.Input
		e.snapshot = s
		e.fs = bundled.WrapFS(iovfs.From(s, s.sensitive))
		runtime.GC()
		response.Kind = "prepared"
	case "execute":
		if e.state != "prepared" {
			return response, fmt.Errorf("execute requires prepared operation")
		}
		if err := e.execute(); err != nil {
			return response, err
		}
		if err := e.snapshot.error(); err != nil {
			return response, err
		}
		e.state = "retained"
		response.Kind = "executed"
	case "checkpoint":
		if e.state != "retained" {
			return response, fmt.Errorf("checkpoint requires retained operation")
		}
		runtime.GC()
		var m runtime.MemStats
		runtime.ReadMemStats(&m)
		runtime.KeepAlive(e.snapshot)
		runtime.KeepAlive(e.input)
		runtime.KeepAlive(e.fs)
		runtime.KeepAlive(e.config)
		runtime.KeepAlive(e.program)
		runtime.KeepAlive(e.diagnostics)
		runtime.KeepAlive(e.writes)
		runtime.KeepAlive(e.emit)
		response.Kind = "checkpoint"
		response.Value = map[string]string{"heap_alloc": u(m.HeapAlloc), "heap_inuse": u(m.HeapInuse), "heap_sys": u(m.HeapSys), "stack_inuse": u(m.StackInuse), "stack_sys": u(m.StackSys), "sys": u(m.Sys), "total_alloc": u(m.TotalAlloc), "mallocs": u(m.Mallocs), "frees": u(m.Frees), "num_gc": u(uint64(m.NumGC))}
		e.state = "checkpointed"
	case "observe":
		if e.state != "checkpointed" {
			return response, fmt.Errorf("observe requires checkpoint before serialization")
		}
		value, err := e.observe()
		if err != nil {
			return response, err
		}
		response.Kind = "observation"
		response.Value = value
	case "release":
		if e.state != "checkpointed" {
			return response, fmt.Errorf("release requires checkpointed operation")
		}
		*e = Engine{}
		response.Kind = "released"
	default:
		return response, fmt.Errorf("unsupported budget command")
	}
	return response, nil
}
func u(v uint64) string { return strconv.FormatUint(v, 10) }
func n(v int) string    { return strconv.Itoa(v) }
func (e *Engine) execute() error {
	h := parseHost{e.fs, e.snapshot.cwd}
	args := make([]string, len(e.input.Configuration.Arguments))
	for i, arg := range e.input.Configuration.Arguments {
		args[i] = string(arg)
	}
	cli := tsoptions.ParseCommandLine(args, h)
	if len(cli.FileNames()) != 0 || cli.CompilerOptions().Project != "" || cli.CompilerOptions().Watch.IsTrue() {
		return fmt.Errorf("arguments must contain compiler overrides only")
	}
	raw, ok := cli.Raw.(*collections.OrderedMap[string, any])
	if !ok {
		return fmt.Errorf("unexpected production parsed argument representation")
	}
	cfg, diags := tsoptions.GetParsedCommandLineOfConfigFile(string(e.input.Configuration.ConfigPath), cli.CompilerOptions(), raw, h, nil)
	e.config = cfg
	if cfg == nil {
		e.diagnostics = append(cli.Errors, diags...)
		return nil
	}
	cfg.Errors = append(cli.Errors, cfg.Errors...)
	if cfg.CompilerOptions().Incremental.IsTrue() || cfg.CompilerOptions().Watch.IsTrue() || cfg.CompilerOptions().RunExternalCode.IsTrue() || len(cfg.ContentMappers()) != 0 {
		return fmt.Errorf("incremental/watch/external content mapper capability unsupported")
	}
	p := compiler.NewProgram(compiler.ProgramOptions{Host: compiler.NewCompilerHost(h.cwd, h.fs, bundled.LibPath(), nil, nil, nil), Config: cfg, SingleThreaded: core.TSTrue})
	e.program = p
	ctx := context.Background()
	d := p.GetConfigFileParsingDiagnostics()
	d = append(d, p.GetProgramDiagnostics()...)
	d = append(d, p.GetSyntacticDiagnostics(ctx, nil)...)
	d = append(d, p.GetGlobalDiagnostics(ctx)...)
	for _, file := range p.GetSourceFiles() {
		d = append(d, p.GetSemanticDiagnostics(ctx, file)...)
		if p.Options().Declaration.IsTrue() || p.Options().Composite.IsTrue() {
			d = append(d, p.GetDeclarationDiagnostics(ctx, file)...)
		}
	}
	e.diagnostics = compiler.SortAndDeduplicateDiagnostics(d)
	if e.input.Operation == "check_emit" {
		e.emit = p.Emit(ctx, compiler.EmitOptions{WriteFile: func(name, text string, _ *compiler.WriteFileData) error {
			e.writes = append(e.writes, retainedWrite{name, text})
			return nil
		}})
	}
	return nil
}
func (e *Engine) observe() (Observation, error) {
	o := Observation{Outcome: "compiled", Roots: []Bytes{}, Sources: []Source{}, Diagnostics: []Diagnostic{}, Checkers: "0", Types: "0", Nodes: "0", Identifiers: "0", Writes: []Write{}, EmittedFiles: []Bytes{}, EmitDiagnostics: []Diagnostic{}}
	indices := map[*ast.SourceFile]int{}
	addSource := func(f *ast.SourceFile) int {
		if i, ok := indices[f]; ok {
			return i
		}
		i := len(o.Sources)
		indices[f] = i
		library := e.program != nil && e.program.IsSourceFileDefaultLibrary(f.Path())
		o.Sources = append(o.Sources, Source{Bytes(f.FileName()), Bytes(f.Path()), Bytes(f.Text()), library, n(f.NodeCount), n(f.IdentifierCount)})
		return i
	}
	var freeze func([]*ast.Diagnostic) []Diagnostic
	freeze = func(values []*ast.Diagnostic) []Diagnostic {
		out := make([]Diagnostic, 0, len(values))
		for _, d := range values {
			var file *int
			if d.File() != nil {
				i := addSource(d.File())
				file = &i
			}
			args := make([]Bytes, len(d.MessageArgs()))
			for i, arg := range d.MessageArgs() {
				args[i] = Bytes(arg)
			}
			row := Diagnostic{File: file, Pos: n(d.Pos()), End: n(d.End()), Code: d.Code(), Category: int(d.Category()), MessageKey: Bytes(d.MessageKey()), MessageText: Bytes(d.MessageText()), MessageArgs: args, Localized: Bytes(d.Localize(locale.Default)), Chain: freeze(d.MessageChain()), Related: freeze(d.RelatedInformation()), Source: Bytes(d.Source()), Unnecessary: d.ReportsUnnecessary(), Deprecated: d.ReportsDeprecated(), SkippedOnNoEmit: d.SkippedOnNoEmit()}
			if r := d.RepopulateInfo(); r != nil {
				row.Repopulate = &Repopulate{int(r.Kind), Bytes(r.ModuleReference), int(r.Mode), Bytes(r.PackageName)}
			}
			out = append(out, row)
		}
		return out
	}
	if e.config != nil {
		for _, root := range e.config.FileNames() {
			o.Roots = append(o.Roots, Bytes(root))
		}
		raw, err := json.Marshal(e.config.CompilerOptions())
		if err != nil {
			return o, err
		}
		o.OptionsJSON = raw
	}
	if e.program == nil {
		o.Outcome = "config_failure"
	} else {
		for _, f := range e.program.GetSourceFiles() {
			addSource(f)
		}
		checkers, types, nodes, identifiers := 0, 0, 0, 0
		e.program.ForEachCheckerParallel(func(_ int, c *checker.Checker) { checkers++; types += int(c.TypeCount) })
		for _, f := range e.program.GetSourceFiles() {
			nodes += f.NodeCount
			identifiers += f.IdentifierCount
		}
		o.Checkers = n(checkers)
		o.Types = n(types)
		o.Nodes = n(nodes)
		o.Identifiers = n(identifiers)
		if checkers != 1 {
			return o, fmt.Errorf("single checker obligation violated: %d", checkers)
		}
	}
	o.Diagnostics = freeze(e.diagnostics)
	for _, w := range e.writes {
		o.Writes = append(o.Writes, Write{Bytes(w.name), Bytes(w.text)})
	}
	if e.emit != nil {
		o.EmitSkipped = &e.emit.EmitSkipped
		o.EmitDiagnostics = freeze(e.emit.Diagnostics)
		for _, file := range e.emit.EmittedFiles {
			o.EmittedFiles = append(o.EmittedFiles, Bytes(file))
		}
	}
	return o, nil
}
