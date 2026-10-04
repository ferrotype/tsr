// Carried test transport for the pinned lsptestutil package. No compiler,
// expected result, rendering, or assertion is replaced.
package lsptestutil

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"sync"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/jsonrpc"
	"github.com/microsoft/TypeScript/tsc/internal/lsp"
	"github.com/microsoft/TypeScript/tsc/internal/lsp/lsproto"
	"gotest.tools/v3/assert"
)

type rustServer struct {
	ready  chan struct{}
	client *LSPClient
	t      *testing.T
}

func (s *rustServer) InitComplete() <-chan struct{} { return s.ready }
func (s *rustServer) SetCompilerOptionsForInferredProjects(_ context.Context, options *core.CompilerOptions) {
	id := jsonrpc.NewIDString("test-options")
	request := (&lsproto.RequestMessage{ID: id, Method: "test/setOptions", Params: map[string]any{"options": options}})
	response, ok := s.client.SendRequestWorker(s.t, request, id)
	assert.Assert(s.t, ok && response.Error == nil, "inferred options failed: %v", response)
}

type rustReader struct {
	lsp.Reader
	ready chan struct{}
	once  sync.Once
}

func (r *rustReader) Read() (*lsproto.Message, error) {
	for {
		msg, err := r.Reader.Read()
		if err != nil {
			return msg, err
		}
		if msg.Kind == jsonrpc.MessageKindNotification && msg.AsRequest().Method == "testhost/lspInitialized" {
			r.once.Do(func() { close(r.ready) })
			continue
		}
		return msg, nil
	}
}
func newRustClient(t *testing.T, opts lsp.ServerOptions, handler ServerRequestHandler) (*LSPClient, func() error) {
	t.Helper()
	cmd := exec.Command(os.Getenv("TSR_LSP_SERVER"), "--stdio")
	in, err := cmd.StdinPipe()
	assert.NilError(t, err)
	out, err := cmd.StdoutPipe()
	assert.NilError(t, err)
	cmd.Stderr = opts.Err
	assert.NilError(t, cmd.Start())
	ctx, cancel := context.WithCancel(t.Context())
	ready := make(chan struct{})
	server := &rustServer{ready: ready, t: t}
	callback := func(ctx context.Context, req *lsproto.RequestMessage) *lsproto.ResponseMessage {
		var path string
		var value any
		if req.Method == "readFile" || req.Method == "fileExists" || req.Method == "directoryExists" || req.Method == "getAccessibleEntries" || req.Method == "realpath" {
			data, err := json.Marshal(req.Params)
			assert.NilError(t, err)
			assert.NilError(t, json.Unmarshal(data, &path))
			switch req.Method {
			case "readFile":
				text, ok := opts.FS.ReadFile(path)
				value = map[string]any{}
				if ok {
					value = map[string]any{"content": text}
				}
			case "fileExists":
				value = opts.FS.FileExists(path)
			case "directoryExists":
				value = opts.FS.DirectoryExists(path)
			case "realpath":
				value = opts.FS.Realpath(path)
			case "getAccessibleEntries":
				entries := opts.FS.GetAccessibleEntries(path)
				value = map[string]any{"files": append([]string{}, entries.Files...), "directories": append([]string{}, entries.Directories...)}
			}
			return &lsproto.ResponseMessage{ID: req.ID, JSONRPC: req.JSONRPC, Result: value}
		}
		if handler != nil {
			return handler(ctx, req)
		}
		return nil
	}
	client := &LSPClient{Server: server, inputWriter: lsp.ToWriter(in), outputReader: &rustReader{Reader: lsp.ToReader(out), ready: ready}, pendingRequests: make(map[jsonrpc.ID]chan *lsproto.ResponseMessage), onServerRequest: callback, ctx: ctx}
	server.client = client
	done := make(chan error, 1)
	go func() { done <- client.MessageRouter(ctx) }()
	cleanup := func() error {
		cancel()
		_ = in.Close()
		err := cmd.Wait()
		routerErr := <-done
		if err != nil {
			return fmt.Errorf("private server: %w", err)
		}
		return routerErr
	}
	// Register cleanup before any assertion can fail during the handshake.
	t.Cleanup(func() {
		if cmd.ProcessState == nil {
			_ = cleanup()
		}
	})
	id := jsonrpc.NewIDString("test-init")
	req := (&lsproto.RequestMessage{ID: id, Method: "test/initialize", Params: map[string]any{
		"version": 3, "caseSensitive": opts.FS.UseCaseSensitiveFileNames(), "base": map[string]string{}, "symlinks": map[string]string{}, "plugins": []string{}, "options": map[string]any{},
		"callbacks": []string{"readFile", "fileExists", "directoryExists", "getAccessibleEntries", "realpath"},
		"project":   map[string]any{"currentDirectory": opts.Cwd, "defaultLibraryPath": opts.DefaultLibraryPath, "positionEncoding": "utf-16", "progressDelayNanos": int64(opts.ProgressDelay)},
	}})
	response, ok := client.SendRequestWorker(t, req, id)
	assert.Assert(t, ok && response.Error == nil, "private initialization: %v", response)
	return client, cleanup
}
