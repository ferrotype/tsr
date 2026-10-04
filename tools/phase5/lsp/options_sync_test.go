// Additional L2 client contract, run unchanged against both implementations.
package lsp_test

import (
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/lsp/lsproto"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/lsptestutil"
	"gotest.tools/v3/assert"
)

func TestL2InferredOptionsAndDocumentSync(t *testing.T) {
	client := initProjectInfoClient(t, map[string]string{"/home/projects/options.ts": "function f(value) { return value; }"})
	client.Server.SetCompilerOptionsForInferredProjects(t.Context(), &core.CompilerOptions{NoLib: core.TSTrue, NoImplicitAny: core.TSTrue})
	uri := lsproto.DocumentUri("file:///home/projects/options.ts")
	lsptestutil.SendNotification(t, client, lsproto.TextDocumentDidOpenInfo, &lsproto.DidOpenTextDocumentParams{TextDocument: &lsproto.TextDocumentItem{Uri: uri, LanguageId: "typescript", Version: 1, Text: "function f(value) { return value; }"}})
	codes := func() []int32 {
		msg, response, ok := lsptestutil.SendRequest(t, client, lsproto.TextDocumentDiagnosticInfo, &lsproto.DocumentDiagnosticParams{TextDocument: lsproto.TextDocumentIdentifier{Uri: uri}})
		assert.Assert(t, ok && msg.AsResponse().Error == nil, "diagnostics failed: %v", msg)
		result := []int32{}
		for _, d := range response.FullDocumentDiagnosticReport.Items {
			result = append(result, *d.Code.Integer)
		}
		return result
	}
	assert.DeepEqual(t, codes(), []int32{7006})
	lsptestutil.SendNotification(t, client, lsproto.TextDocumentDidChangeInfo, &lsproto.DidChangeTextDocumentParams{TextDocument: lsproto.VersionedTextDocumentIdentifier{Uri: uri, Version: 2}, ContentChanges: []lsproto.TextDocumentContentChangePartialOrWholeDocument{{WholeDocument: &lsproto.TextDocumentContentChangeWholeDocument{Text: "function f(value: number) { return value; }"}}}})
	assert.DeepEqual(t, codes(), []int32{})
	client.Server.SetCompilerOptionsForInferredProjects(t.Context(), &core.CompilerOptions{NoLib: core.TSTrue, NoImplicitAny: core.TSFalse})
	lsptestutil.SendNotification(t, client, lsproto.TextDocumentDidChangeInfo, &lsproto.DidChangeTextDocumentParams{TextDocument: lsproto.VersionedTextDocumentIdentifier{Uri: uri, Version: 3}, ContentChanges: []lsproto.TextDocumentContentChangePartialOrWholeDocument{{WholeDocument: &lsproto.TextDocumentContentChangeWholeDocument{Text: "function f(value) { return value; }"}}}})
	assert.DeepEqual(t, codes(), []int32{7044}) // suggestion replaces the implicit-any error
}
