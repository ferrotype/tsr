package harnessutil

// The Phase 3 build-info witness's access to the harness's own program
// construction (an access-only overlay: it adds this file and replaces no
// pinned source). One step is one compilation as CompileFilesEx performs it
// for its post-emit program: a fresh in-memory file system over the step's
// files, the harness's compiler host, and `createProgram`, which wraps the
// program with `incremental.NewProgram` over the build info its test reader
// reads when the options set `incremental`.
import (
	"context"
	"testing/fstest"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/execute/incremental"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

// Phase3StepResult is what one step observed: each action's result in order
// and the files the step wrote, in written order.
type Phase3StepResult struct {
	Emits       []*compiler.EmitResult
	Diagnostics [][]*ast.Diagnostic
	Outputs     []*TestFile
}

// Phase3IncrementalStep runs `actions` ("emit": `Emit(ctx, EmitOptions{})`;
// "diagnostics": the post-emit program's diagnostics in compileFilesWithHost's
// order) on a program created by createProgram over `files`. With
// `forceIncremental` the program is wrapped as createProgram wraps an
// `incremental` one whatever its options (a `tsc -b` program's
// non-incremental build info).
func Phase3IncrementalStep(files map[string]string, symlinks map[string]string, caseSensitive bool, currentDirectory string, config *tsoptions.ParsedCommandLine, actions []string, forceIncremental bool) *Phase3StepResult {
	testfs := map[string]any{}
	for name, content := range files {
		testfs[name] = &fstest.MapFile{Data: []byte(content)}
	}
	for from, to := range symlinks {
		testfs[from] = vfstest.Symlink(to)
	}
	fs := vfstest.FromMap(testfs, caseSensitive)
	fs = bundled.WrapFS(fs)
	fs = NewOutputRecorderFS(fs)
	host := createCompilerHost(fs, bundled.LibPath(), currentDirectory, nil)
	program := createProgram(host, config)
	if forceIncremental && !config.CompilerOptions().Incremental.IsTrue() {
		oldProgram := incremental.ReadBuildInfoProgram(config, getTestBuildInfoReader(host), host)
		program = incremental.NewProgram(program.(*compiler.Program), oldProgram, incremental.CreateHost(host), nil, false)
	}
	ctx := context.Background()
	result := &Phase3StepResult{}
	for _, action := range actions {
		switch action {
		case "emit":
			result.Emits = append(result.Emits, program.Emit(ctx, compiler.EmitOptions{}))
		case "diagnostics":
			var diagnostics []*ast.Diagnostic
			diagnostics = append(diagnostics, program.GetConfigFileParsingDiagnostics()...)
			diagnostics = append(diagnostics, program.GetProgramDiagnostics()...)
			diagnostics = append(diagnostics, program.GetSyntacticDiagnostics(ctx, nil)...)
			diagnostics = append(diagnostics, program.GetSemanticDiagnostics(ctx, nil)...)
			diagnostics = append(diagnostics, program.GetGlobalDiagnostics(ctx)...)
			if program.Options().GetEmitDeclarations() {
				diagnostics = append(diagnostics, program.GetDeclarationDiagnostics(ctx, nil)...)
			}
			result.Diagnostics = append(result.Diagnostics, compiler.SortAndDeduplicateDiagnostics(diagnostics))
		default:
			panic("unknown step action: " + action)
		}
	}
	result.Outputs = fs.(*OutputRecorderFS).Outputs()
	return result
}
