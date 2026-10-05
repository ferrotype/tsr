// Carried test transport for the pinned lsptestutil package. No compiler,
// expected result, rendering, or assertion is replaced.
package lsptestutil

import (
	"context"
	"encoding/base64"
	"fmt"
	"io"
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
	mappers *rustMappers
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
		if msg.Kind == jsonrpc.MessageKindNotification {
			if consumed, err := r.mappers.notification(msg.AsRequest()); consumed || err != nil {
				if err != nil {
					return nil, err
				}
				continue
			}
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
	mappers := &rustMappers{streams: make(map[string]*rustMapperStream), spawn: opts.Spawn}
	callback := func(ctx context.Context, req *lsproto.RequestMessage) *lsproto.ResponseMessage {
		if req.Method == "testhost/spawnPlugin" {
			return mappers.open(req)
		}
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
	client := &LSPClient{Server: server, inputWriter: lsp.ToWriter(in), outputReader: &rustReader{Reader: lsp.ToReader(out), ready: ready, mappers: mappers}, pendingRequests: make(map[jsonrpc.ID]chan *lsproto.ResponseMessage), onServerRequest: callback, ctx: ctx}
	server.client = client
	mappers.client = client
	done := make(chan error, 1)
	go func() { done <- client.MessageRouter(ctx) }()
	cleanup := func() error {
		mappers.close()
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

// rustMappers carries the existing S11 byte tunnel. The mapper implementation
// remains opts.Spawn, including native test mappers and their assertions.
type rustMappers struct {
	mu      sync.Mutex
	streams map[string]*rustMapperStream
	next    int
	closed  bool
	client  *LSPClient
	spawn   func([]string, string, io.Writer) (io.ReadWriteCloser, error)
}
type rustMapperStream struct {
	mu          sync.Mutex
	ready       *sync.Cond
	process     io.ReadWriteCloser
	errors      *io.PipeReader
	errorWriter *io.PipeWriter
	credits     [2]int
	closed      bool
	started     bool
	input       chan []byte
	done        chan struct{}
}

func (m *rustMappers) notify(method string, params any) error {
	return m.client.writeToServer((&lsproto.RequestMessage{Method: lsproto.Method(method), Params: params}).Message())
}
func (m *rustMappers) open(req *lsproto.RequestMessage) *lsproto.ResponseMessage {
	var params struct {
		Name    string `json:"name"`
		Options struct {
			Command []string `json:"command"`
			Cwd     string   `json:"cwd"`
		} `json:"options"`
	}
	data, err := json.Marshal(req.Params)
	if err == nil {
		err = json.Unmarshal(data, &params)
	}
	if err == nil && m.spawn == nil {
		err = fmt.Errorf("test mapper spawner unavailable")
	}
	var stream *rustMapperStream
	if err == nil {
		reader, writer := io.Pipe()
		process, spawnErr := m.spawn(params.Options.Command, params.Options.Cwd, writer)
		err = spawnErr
		if err != nil {
			_ = reader.Close()
			_ = writer.Close()
		} else {
			stream = &rustMapperStream{process: process, errors: reader, errorWriter: writer, input: make(chan []byte, 64), done: make(chan struct{})}
			stream.ready = sync.NewCond(&stream.mu)
		}
	}
	if err != nil {
		return &lsproto.ResponseMessage{ID: req.ID, JSONRPC: req.JSONRPC, Error: &jsonrpc.ResponseError{Code: -32603, Message: err.Error()}}
	}
	m.mu.Lock()
	if m.closed {
		m.mu.Unlock()
		m.closeStream(stream)
		return &lsproto.ResponseMessage{ID: req.ID, JSONRPC: req.JSONRPC, Error: &jsonrpc.ResponseError{Code: -32603, Message: "mapper client closed"}}
	}
	m.next++
	name := fmt.Sprintf("mapper:%d", m.next)
	m.streams[name] = stream
	m.mu.Unlock()
	return &lsproto.ResponseMessage{ID: req.ID, JSONRPC: req.JSONRPC, Result: map[string]any{"stream": name}}
}
func (m *rustMappers) notification(req *lsproto.RequestMessage) (bool, error) {
	if req.Method != "testhost/streamCredit" && req.Method != "testhost/streamData" && req.Method != "testhost/streamClose" {
		return false, nil
	}
	var params struct {
		Stream  string `json:"stream"`
		Channel string `json:"channel"`
		Bytes   int    `json:"bytes"`
		Data    string `json:"data"`
	}
	data, err := json.Marshal(req.Params)
	if err == nil {
		err = json.Unmarshal(data, &params)
	}
	if err != nil {
		return true, err
	}
	m.mu.Lock()
	stream := m.streams[params.Stream]
	m.mu.Unlock()
	if stream == nil {
		return true, fmt.Errorf("unknown mapper stream %s", params.Stream)
	}
	switch req.Method {
	case "testhost/streamCredit":
		channel := 0
		if params.Channel == "stderr" {
			channel = 1
		} else if params.Channel != "stdout" {
			return true, fmt.Errorf("bad stream channel")
		}
		stream.mu.Lock()
		if !stream.closed {
			stream.credits[channel] += params.Bytes
		}
		start := !stream.started
		stream.started = true
		stream.ready.Broadcast()
		stream.mu.Unlock()
		if start {
			if err = m.notify("test/streamCredit", map[string]any{"stream": params.Stream, "bytes": 65536}); err != nil {
				return true, err
			}
			go m.pump(params.Stream, stream, 0, stream.process)
			go m.pump(params.Stream, stream, 1, stream.errors)
			go func() {
				for {
					select {
					case bytes := <-stream.input:
						if _, err := stream.process.Write(bytes); err != nil {
							m.closeStream(stream)
							return
						}
						if err := m.notify("test/streamCredit", map[string]any{"stream": params.Stream, "bytes": len(bytes)}); err != nil {
							m.closeStream(stream)
							return
						}
					case <-stream.done:
						return
					}
				}
			}()
		}
	case "testhost/streamData":
		bytes, err := base64.StdEncoding.DecodeString(params.Data)
		if err != nil {
			return true, err
		}
		select {
		case stream.input <- bytes:
		case <-stream.done:
		}
	case "testhost/streamClose":
		m.closeStream(stream)
	}
	return true, nil
}
func (m *rustMappers) pump(name string, stream *rustMapperStream, channel int, reader io.Reader) {
	label := "stdout"
	if channel == 1 {
		label = "stderr"
	}
	bytes := make([]byte, 32768)
	for {
		stream.mu.Lock()
		for stream.credits[channel] == 0 && !stream.closed {
			stream.ready.Wait()
		}
		if stream.closed {
			stream.mu.Unlock()
			return
		}
		limit := min(len(bytes), stream.credits[channel])
		stream.mu.Unlock()
		n, err := reader.Read(bytes[:limit])
		if n > 0 {
			stream.mu.Lock()
			stream.credits[channel] -= n
			stream.mu.Unlock()
			if m.notify("test/streamData", map[string]any{"stream": name, "channel": label, "data": base64.StdEncoding.EncodeToString(bytes[:n])}) != nil {
				m.closeStream(stream)
				return
			}
		}
		if err != nil {
			_ = m.notify("test/streamEnd", map[string]any{"stream": name, "channel": label})
			return
		}
	}
}
func (m *rustMappers) closeStream(stream *rustMapperStream) {
	stream.mu.Lock()
	if stream.closed {
		stream.mu.Unlock()
		return
	}
	stream.closed = true
	close(stream.done)
	stream.ready.Broadcast()
	stream.mu.Unlock()
	_ = stream.process.Close()
	_ = stream.errors.Close()
	_ = stream.errorWriter.Close()
}
func (m *rustMappers) close() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.closed = true
	for _, stream := range m.streams {
		m.closeStream(stream)
	}
}
