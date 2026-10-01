package compiler

// Phase 3 transform probe (overlay only). It runs a named chain of the pinned
// script transformers over one source file of a program, with the options
// getScriptTransformers builds, and prints the result with emitJSFile's
// printer options and no source map. The chain "script" is the pin's own
// chain for the file. Nothing here changes what the pin computes.
import (
	"context"
	"sort"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/binder"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/printer"
	"github.com/microsoft/TypeScript/tsc/internal/transformers"
	"github.com/microsoft/TypeScript/tsc/internal/transformers/estransforms"
	"github.com/microsoft/TypeScript/tsc/internal/transformers/inliners"
	"github.com/microsoft/TypeScript/tsc/internal/transformers/jsxtransforms"
	"github.com/microsoft/TypeScript/tsc/internal/transformers/moduletransforms"
	"github.com/microsoft/TypeScript/tsc/internal/transformers/tstransforms"
)

func phase3Transformers() map[string]transformers.TransformerFactory {
	result := map[string]transformers.TransformerFactory{
		"metadata":         tstransforms.NewMetadataTransformer,
		"typeeraser":       tstransforms.NewTypeEraserTransformer,
		"importelision":    tstransforms.NewImportElisionTransformer,
		"runtimesyntax":    tstransforms.NewRuntimeSyntaxTransformer,
		"legacydecorators": tstransforms.NewLegacyDecoratorsTransformer,
		"jsx":              jsxtransforms.NewJSXTransformer,
		"es":               estransforms.GetESTransformer,
		"usestrict":        estransforms.NewUseStrictTransformer,
		"module":           getModuleTransformer,
		"commonjs":         moduletransforms.NewCommonJSModuleTransformer,
		"esmodule":         moduletransforms.NewESModuleTransformer,
		"impliedmodule":    moduletransforms.NewImpliedModuleTransformer,
		"constenum":        inliners.NewConstEnumInliningTransformer,
	}
	for name, factory := range estransforms.Phase3Transformers {
		result[name] = factory
	}
	return result
}

func Phase3TransformerNames() []string {
	names := []string{"script"}
	for name := range phase3Transformers() {
		names = append(names, name)
	}
	sort.Strings(names)
	return names
}

func Phase3Transform(ctx context.Context, program *Program, sourceFile *ast.SourceFile, chain []string) string {
	host, done := newEmitHost(ctx, program, sourceFile)
	defer done()
	emitContext, putEmitContext := printer.GetEmitContext()
	defer putEmitContext()
	options := host.Options()

	var chosen []*transformers.Transformer
	if len(chain) == 1 && chain[0] == "script" {
		chosen = getScriptTransformers(emitContext, host, sourceFile)
	} else {
		// The resolver selection of getScriptTransformers.
		importElisionEnabled := !options.VerbatimModuleSyntax.IsTrue() && !ast.IsInJSFile(sourceFile.AsNode())
		jsxTransformEnabled := options.GetJSXTransformEnabled() && sourceFile.LanguageVariant == core.LanguageVariantJSX
		emitResolver := host.GetEmitResolver()
		var referenceResolver binder.ReferenceResolver
		if importElisionEnabled || jsxTransformEnabled || !options.GetIsolatedModules() || options.EmitDecoratorMetadata.IsTrue() {
			referenceResolver = emitResolver
		} else {
			referenceResolver = binder.NewReferenceResolver(options, binder.ReferenceResolverHooks{})
		}
		opts := transformers.TransformOptions{
			Context:                   emitContext,
			CompilerOptions:           options,
			Resolver:                  referenceResolver,
			EmitResolver:              emitResolver,
			GetEmitModuleFormatOfFile: host.GetEmitModuleFormatOfFile,
		}
		known := phase3Transformers()
		for _, name := range chain {
			factory, ok := known[name]
			if !ok {
				panic("unknown transformer: " + name)
			}
			if transformer := factory(&opts); transformer != nil {
				chosen = append(chosen, transformer)
			}
		}
	}
	for _, transformer := range chosen {
		sourceFile = transformer.TransformSourceFile(sourceFile)
	}

	printerOptions := printer.PrinterOptions{
		RemoveComments: options.RemoveComments.IsTrue(),
		NewLine:        options.NewLine,
		NoEmitHelpers:  options.NoEmitHelpers.IsTrue(),
		Target:         options.Target,
	}
	writer := printer.NewTextWriter(options.NewLine.GetNewLineCharacter(), 0)
	printer.NewPrinter(printerOptions, printer.PrintHandlers{}, emitContext).Write(sourceFile.AsNode(), sourceFile, writer, nil)
	return writer.String()
}
