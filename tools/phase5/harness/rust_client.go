// Carried transport only: the pin's client, assertions and mapper stay intact.
package lsptestutil

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"sync"
	"testing"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/jsonrpc"
	"github.com/microsoft/TypeScript/tsc/internal/lsp"
	"github.com/microsoft/TypeScript/tsc/internal/lsp/lsproto"
	"gotest.tools/v3/assert"
)

type rustFrame struct {
	message *lsproto.Message
	err     error
}
type rustSession struct {
	frames chan rustFrame
	done   chan struct{}
}
type rustWorker struct {
	cmd    *exec.Cmd
	input  io.WriteCloser
	writer lsp.Writer
	mu     sync.Mutex
	active *rustSession
	err    error
	exited chan struct{}
}

var rustLease sync.Mutex
var retainedWorker *rustWorker

// The worker reader outlives clients. A client router ends at the reset barrier,
// not by closing the process pipe, which retains only the server's parse cache.
func startRustWorker() (*rustWorker, error) {
	cmd := exec.Command(os.Getenv("TSR_LSP_SERVER"), "--stdio")
	input, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	output, err := cmd.StdoutPipe()
	if err != nil {
		_ = input.Close()
		return nil, err
	}
	cmd.Stderr = os.Stderr
	if err := cmd.Start(); err != nil {
		_ = input.Close()
		_ = output.Close()
		return nil, err
	}
	w := &rustWorker{cmd: cmd, input: input, writer: lsp.ToWriter(input), exited: make(chan struct{})}
	go func() {
		reader := lsp.ToReader(output)
		for {
			message, err := reader.Read()
			w.mu.Lock()
			session := w.active
			if err != nil {
				w.err = err
			}
			if session == nil && err == nil {
				err = fmt.Errorf("private worker sent traffic outside a session")
				w.err = err
			}
			if session != nil {
				select {
				case session.frames <- rustFrame{message, err}:
				case <-session.done:
				}
			}
			w.mu.Unlock()
			if err != nil {
				_ = input.Close()
				_ = cmd.Process.Kill()
				break
			}
		}
		_ = cmd.Wait()
		close(w.exited)
	}()
	return w, nil
}
func (w *rustWorker) retire() {
	_ = w.input.Close()
	_ = w.cmd.Process.Kill()
	select {
	case <-w.exited:
	case <-time.After(2 * time.Second):
		panic("private worker did not retire; supervisor must restart the process")
	}
}

type rustServer struct {
	ready  chan struct{}
	client *LSPClient
	t      *testing.T
}

func (s *rustServer) InitComplete() <-chan struct{} { return s.ready }
func (s *rustServer) SetCompilerOptionsForInferredProjects(_ context.Context, options *core.CompilerOptions) {
	s.request("test/setOptions", map[string]any{"options": options})
}
func (s *rustServer) request(method string, params any) *lsproto.ResponseMessage {
	id := jsonrpc.NewIDString("test:" + method)
	response, ok := s.client.SendRequestWorker(s.t, &lsproto.RequestMessage{ID: id, Method: lsproto.Method(method), Params: params}, id)
	assert.Assert(s.t, ok && response != nil && response.Error == nil, "private %s failed: %v", method, response)
	return response
}
func (s *rustServer) ProjectState(t *testing.T) json.Value {
	return s.request("test/projectState", map[string]any{}).Result.(json.Value)
}

// Access-only helper used by the carried state writer; native servers keep the
// original Session/Snapshot path in the overlay.
func RustProjectState(t *testing.T, client *LSPClient) (json.Value, bool) {
	if server, ok := client.Server.(*rustServer); ok {
		return server.ProjectState(t), true
	}
	return nil, false
}

// Faults change one successful response of the selected request method. They
// are confined to this carried transport and never mutate requests or the server.
type rustFault struct {
	mu       sync.Mutex
	selector string
	pending  map[jsonrpc.ID]lsproto.Method
	fired    bool
}
type rustFaultWriter struct {
	writer lsp.Writer
	fault  *rustFault
}

func (w *rustFaultWriter) Write(message *lsproto.Message) error {
	var tracked *jsonrpc.ID
	if message.Kind == jsonrpc.MessageKindRequest {
		request := message.AsRequest()
		f := w.fault
		f.mu.Lock()
		selected := (f.selector == "hover" && request.Method == "textDocument/hover") || (f.selector == "completion" && request.Method == "textDocument/completion")
		if selected && !f.fired && request.ID != nil {
			f.pending[*request.ID] = request.Method
			tracked = request.ID
		}
		f.mu.Unlock()
	}
	err := w.writer.Write(message)
	if err != nil && tracked != nil {
		w.fault.mu.Lock()
		delete(w.fault.pending, *tracked)
		w.fault.mu.Unlock()
	}
	return err
}
func (f *rustFault) corrupt(message *lsproto.Message) {
	if message.Kind != jsonrpc.MessageKindResponse {
		return
	}
	response := message.AsResponse()
	if response.ID == nil {
		return
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	method, selected := f.pending[*response.ID]
	delete(f.pending, *response.ID)
	if !selected || f.fired || response.Error != nil {
		return
	}
	switch method {
	case "textDocument/hover":
		response.Result = json.Value(`{"contents":{"kind":"markdown","value":"TSR deliberate hover fault"}}`)
	case "textDocument/completion":
		response.Result = json.Value(`{"isIncomplete":false,"items":[{"label":"TSR_deliberate_completion_fault","kind":1,"sortText":"0"}]}`)
	default:
		return
	}
	f.fired = true
	fmt.Fprintf(os.Stderr, "TSR_FAULT injected %s response\n", method)
}

type rustReader struct {
	fault   *rustFault
	session *rustSession
	mappers *rustMappers
	ready   chan struct{}
	once    sync.Once
}

func (r *rustReader) Read() (*lsproto.Message, error) {
	for {
		frame, ok := <-r.session.frames
		if !ok {
			return nil, io.EOF
		}
		if frame.err != nil {
			return nil, frame.err
		}
		message := frame.message
		r.fault.corrupt(message)
		if message.Kind == jsonrpc.MessageKindNotification {
			request := message.AsRequest()
			if request.Method == "testhost/lspInitialized" {
				r.once.Do(func() { close(r.ready) })
				continue
			}
			if request.Method == "testhost/failure" {
				return nil, fmt.Errorf("private worker failure: %v", request.Params)
			}
			if consumed, err := r.mappers.notification(request); consumed || err != nil {
				if err != nil {
					return nil, err
				}
				continue
			}
		}
		return message, nil
	}
}
func newRustClient(t *testing.T, opts lsp.ServerOptions, handler ServerRequestHandler) (*LSPClient, func() error) {
	t.Helper()
	if !rustLease.TryLock() {
		t.Fatal("concurrent private sessions: run with -test.parallel=1")
	}
	worker := retainedWorker
	if worker != nil {
		worker.mu.Lock()
		failed := worker.err != nil
		worker.mu.Unlock()
		if failed {
			worker.retire()
			worker = nil
		}
	}
	if worker == nil {
		var err error
		worker, err = startRustWorker()
		if err != nil {
			rustLease.Unlock()
			t.Fatal(err)
		}
		retainedWorker = worker
	}
	session := &rustSession{frames: make(chan rustFrame, 64), done: make(chan struct{})}
	worker.mu.Lock()
	worker.active = session
	worker.mu.Unlock()
	ctx, cancel := context.WithCancel(context.Background())
	server := &rustServer{ready: make(chan struct{}), t: t}
	mappers := &rustMappers{streams: map[string]*rustMapperStream{}, spawn: opts.Spawn}
	callback := func(ctx context.Context, req *lsproto.RequestMessage) *lsproto.ResponseMessage {
		if req.Method == "testhost/spawnPlugin" {
			return mappers.open(req)
		}
		var path string
		var value any
		switch req.Method {
		case "readFile", "fileExists", "directoryExists", "getAccessibleEntries", "realpath":
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
	fault := &rustFault{selector: os.Getenv("TSR_FAULT"), pending: map[jsonrpc.ID]lsproto.Method{}}
	client := &LSPClient{Server: server, inputWriter: &rustFaultWriter{writer: worker.writer, fault: fault}, outputReader: &rustReader{session: session, mappers: mappers, ready: server.ready, fault: fault}, pendingRequests: map[jsonrpc.ID]chan *lsproto.ResponseMessage{}, onServerRequest: callback, ctx: ctx}
	server.client = client
	mappers.client = client
	routed := make(chan error, 1)
	go func() { routed <- client.MessageRouter(ctx) }()
	var once sync.Once
	var closeError error
	cleanup := func() error {
		once.Do(func() {
			var endOnce sync.Once
			endSession := func() { endOnce.Do(func() { cancel(); close(session.done) }) }
			// Stop mapper writes before the reset barrier; leave callback routing alive
			// until the server has cancelled and joined its session work.
			stopped := make(chan struct{})
			go func() { mappers.close(); close(stopped) }()
			select {
			case <-stopped:
			case <-time.After(10 * time.Second):
				closeError = fmt.Errorf("mapper cleanup deadline")
				endSession()
				worker.retire()
				select {
				case <-stopped:
				case <-time.After(time.Second):
					panic("mapper did not stop after worker retirement")
				}
			}
			routerFinished := false
			if closeError == nil {
				id := jsonrpc.NewIDString("test:reset")
				response := make(chan *lsproto.ResponseMessage, 1)
				client.pendingRequestsMu.Lock()
				client.pendingRequests[*id] = response
				client.pendingRequestsMu.Unlock()
				resetDeadline := time.NewTimer(10 * time.Second)
				defer resetDeadline.Stop()
				written := make(chan error, 1)
				go func() {
					written <- client.writeToServer((&lsproto.RequestMessage{ID: id, Method: "test/reset", Params: map[string]any{}}).Message())
				}()
				select {
				case closeError = <-written:
				case <-resetDeadline.C:
					closeError = fmt.Errorf("private reset write deadline")
					endSession()
					worker.retire()
					select {
					case <-written:
					case <-time.After(time.Second):
						panic("private reset writer did not stop after retirement")
					}
				}
				if closeError == nil {
					select {
					case reply := <-response:
						if reply == nil || reply.Error != nil {
							closeError = fmt.Errorf("private reset failed: %v", reply)
						}
					case closeError = <-routed:
						routerFinished = true
						if closeError == nil {
							closeError = fmt.Errorf("private router ended before reset")
						}
					case <-resetDeadline.C:
						closeError = fmt.Errorf("private reset deadline")
					}
				}
			}
			endSession()
			if closeError != nil {
				worker.retire()
				retainedWorker = nil
			}
			worker.mu.Lock()
			worker.active = nil
			close(session.frames)
			worker.mu.Unlock()
			if !routerFinished {
				select {
				case err := <-routed:
					if closeError == nil {
						closeError = err
					}
				case <-time.After(time.Second):
					panic("private callback router did not stop")
				}
			}
			if closeError != nil {
				worker.retire()
				retainedWorker = nil
			}
			rustLease.Unlock()
		})
		return closeError
	}
	t.Cleanup(func() {
		if err := cleanup(); err != nil {
			t.Error(err)
		}
	})
	server.request("test/initialize", map[string]any{
		"version": 3, "caseSensitive": opts.FS.UseCaseSensitiveFileNames(), "base": map[string]string{}, "symlinks": map[string]string{}, "plugins": []string{}, "options": map[string]any{},
		"callbacks": []string{"readFile", "fileExists", "directoryExists", "getAccessibleEntries", "realpath"},
		"project":   map[string]any{"currentDirectory": opts.Cwd, "defaultLibraryPath": opts.DefaultLibraryPath, "positionEncoding": "utf-16", "progressDelayNanos": int64(opts.ProgressDelay)},
	})
	return client, cleanup
}
