// Parser-only GOOS=js authority. Both entry points parse a fresh file; no cache.
package main

import (
	"runtime"
	"syscall/js"

	"github.com/microsoft/TypeScript/tsc/internal/api/encoder"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/zeebo/xxh3"
)

func bytes(value js.Value) []byte {
	result := make([]byte, value.Length())
	js.CopyBytesToGo(result, value)
	return result
}

func parse(args []js.Value) *ast.SourceFile {
	text := string(bytes(args[0]))
	name := string(bytes(args[1]))
	return parser.ParseSourceFile(ast.SourceFileParseOptions{
		FileName: name, Path: tspath.Path(name),
		ExternalModuleIndicatorOptions: ast.ExternalModuleIndicatorOptions{
			JSX: len(args) > 3 && args[3].Bool(),
			Force: len(args) > 4 && args[4].Bool(),
		},
	}, text, core.ScriptKind(args[2].Int()))
}

func main() {
	js.Global().Set("s10Parser", map[string]any{
		"parse": js.FuncOf(func(_ js.Value, args []js.Value) any {
			file := parse(args)
			return []any{file.NodeCount, file.IdentifierCount, len(file.Diagnostics())}
		}),
		"parse_and_encode": js.FuncOf(func(_ js.Value, args []js.Value) any {
			file := parse(args)
			file.Hash = xxh3.HashString128(file.Text())
			data, _, err := encoder.EncodeSourceFile(file)
			if err != nil {
				panic(err)
			}
			result := js.Global().Get("Uint8Array").New(len(data))
			js.CopyBytesToJS(result, data)
			return result
		}),
		// Include the final collection in each timed parse batch: Rust drops
		// every AST before returning; deferred Go reclamation isn't free work.
		"collect": js.FuncOf(func(_ js.Value, _ []js.Value) any {
			runtime.GC()
			return nil
		}),
	})
	select {}
}
