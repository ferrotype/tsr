package harnessutil

// The checkerbench compile: one program, bound before the interval, then the
// pre-emit diagnostic sequence of compileFilesWithHost inside it. No emit and
// no second program: Rust's executor checks once without emitting, and the
// frozen E2 comparison established that the diagnostic set is the same.

import (
	"context"
	"runtime"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
)

var S08CheckerbenchCompile func(host compiler.CompilerHost, config *tsoptions.ParsedCommandLine, harnessOptions *HarnessOptions) *CompilationResult

// S08CheckerbenchLastProgram is the program the current variant checked, for the
// walker's census and the retained checkpoint.
var S08CheckerbenchLastProgram compiler.ProgramLike

func S08InstallCheckerbenchCompile() {
	S08CheckerbenchCompile = func(host compiler.CompilerHost, config *tsoptions.ParsedCommandLine, harnessOptions *HarnessOptions) *CompilationResult {
		ctx := context.Background()
		compilerOptions := config.CompilerOptions().Clone()
		compilerOptions.TraceResolution = core.TSFalse
		benchConfig := &tsoptions.ParsedCommandLine{
			ParsedConfig: &tsoptions.ParsedOptions{
				CompilerOptions: compilerOptions,
				FileNames:       config.FileNames(),
				ContentMappers:  config.ContentMappers(),
			},
			ConfigFile: config.ConfigFile,
			Errors:     config.Errors,
		}
		program := createProgram(host, benchConfig)
		// Bound inputs are prepared before the checker interval.
		program.Program().BindSourceFiles()
		S08CheckerbenchLastProgram = program
		clock := core.S08Bench
		if clock.Mode == "alloc" {
			S08LiveBeforeInterval = S08LiveHeap()
		}
		clock.Start()
		var errors []*ast.Diagnostic
		errors = append(errors, program.GetConfigFileParsingDiagnostics()...)
		errors = append(errors, program.GetProgramDiagnostics()...)
		errors = append(errors, program.GetSyntacticDiagnostics(ctx, nil)...)
		errors = append(errors, program.GetSemanticDiagnostics(ctx, nil)...)
		errors = append(errors, program.GetGlobalDiagnostics(ctx)...)
		if program.Options().GetEmitDeclarations() {
			errors = append(errors, program.GetDeclarationDiagnostics(ctx, nil)...)
		}
		if harnessOptions.CaptureSuggestions {
			errors = append(errors, program.GetSuggestionDiagnostics(ctx, nil)...)
		}
		clock.Stop()
		errors = compiler.SortAndDeduplicateDiagnostics(errors)
		return newCompilationResult(host, config.CompilerOptions(), program, nil, errors, harnessOptions)
	}
}

// Retained checkpoint endpoints (alloc mode): live heap after explicit GC,
// outside the interval, with the bound inputs live at both endpoints.
var S08LiveBeforeInterval uint64

func S08LiveHeap() uint64 {
	runtime.GC()
	var m runtime.MemStats
	runtime.ReadMemStats(&m)
	return m.HeapAlloc
}
