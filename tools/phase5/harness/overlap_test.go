// Transport-only witnesses; overlaid into lsptestutil for a separate probe.
package lsptestutil

import (
	"context"
	"io"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/lsp"
	"github.com/microsoft/TypeScript/tsc/internal/lsp/lsproto"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
	"gotest.tools/v3/assert"
)

func TestL7OverlappingClientsKeepFilesOptionsAndPrimaryUsable(t *testing.T) {
	const text = `import { value } from "./dep"; function f(param) { return value; }`
	open := func(label string, implicit core.Tristate) (*LSPClient, func() error) {
		fs := vfstest.FromMap(map[string]string{"/entry.ts": text, "/dep.ts": "export const value = \"" + label + "\";"}, true)
		handler := func(_ context.Context, request *lsproto.RequestMessage) *lsproto.ResponseMessage {
			switch request.Method {
			case lsproto.MethodClientRegisterCapability, lsproto.MethodClientUnregisterCapability, lsproto.MethodWindowWorkDoneProgressCreate:
				return &lsproto.ResponseMessage{ID: request.ID, JSONRPC: request.JSONRPC, Result: lsproto.Null{}}
			}
			return nil
		}
		client, closeClient := NewLSPClient(t, lsp.ServerOptions{Err: io.Discard, Cwd: "/", FS: fs, DefaultLibraryPath: "/"}, handler)
		t.Cleanup(func() { assert.NilError(t, closeClient()) })
		client.SetCompilerOptionsForInferredProjects(&core.CompilerOptions{NoLib: core.TSTrue, NoImplicitAny: implicit})
		message, _, ok := SendRequest(t, client, lsproto.InitializeInfo, &lsproto.InitializeParams{Capabilities: &lsproto.ClientCapabilities{}})
		assert.Assert(t, ok && message.AsResponse().Error == nil)
		SendNotification(t, client, lsproto.InitializedInfo, &lsproto.InitializedParams{})
		<-client.Server.InitComplete()
		SendNotification(t, client, lsproto.TextDocumentDidOpenInfo, &lsproto.DidOpenTextDocumentParams{TextDocument: &lsproto.TextDocumentItem{Uri: "file:///entry.ts", LanguageId: "typescript", Version: 1, Text: text}})
		return client, closeClient
	}
	check := func(client *LSPClient, label string, implicit bool) {
		message, result, ok := SendRequest(t, client, lsproto.TextDocumentHoverInfo, &lsproto.HoverParams{TextDocument: lsproto.TextDocumentIdentifier{Uri: "file:///entry.ts"}, Position: lsproto.Position{Line: 0, Character: 9}})
		assert.Assert(t, ok && message.AsResponse().Error == nil)
		data, err := json.Marshal(result)
		assert.NilError(t, err)
		assert.Assert(t, strings.Contains(string(data), label), "filesystem leaked: %s", data)
		message, diagnostics, ok := SendRequest(t, client, lsproto.TextDocumentDiagnosticInfo, &lsproto.DocumentDiagnosticParams{TextDocument: lsproto.TextDocumentIdentifier{Uri: "file:///entry.ts"}})
		assert.Assert(t, ok && message.AsResponse().Error == nil)
		found := false
		for _, d := range diagnostics.FullDocumentDiagnosticReport.Items {
			if d.Code != nil && d.Code.Integer != nil && *d.Code.Integer == 7006 {
				found = true
			}
		}
		assert.Equal(t, found, implicit, "inferred options leaked")
	}
	primary, closePrimary := open("primary", core.TSTrue)
	check(primary, "primary", true)
	secondary, closeSecondary := open("secondary", core.TSFalse)
	check(secondary, "secondary", false)
	check(primary, "primary", true)
	assert.NilError(t, closeSecondary())
	check(primary, "primary", true)
	assert.NilError(t, closePrimary())
}
