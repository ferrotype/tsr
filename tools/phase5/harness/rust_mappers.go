// Shared S11 mapper tunnel for the carried native test client.
package lsptestutil

import (
	"encoding/base64"
	"fmt"
	"github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/jsonrpc"
	"github.com/microsoft/TypeScript/tsc/internal/lsp/lsproto"
	"io"
	"sync"
)

// rustMappers carries the existing S11 byte tunnel. The mapper implementation
// remains opts.Spawn, including native test mappers and their assertions.
type rustMappers struct {
	mu      sync.Mutex
	pumps   sync.WaitGroup
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
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.closed {
		return io.ErrClosedPipe
	}
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
		m.mu.Lock()
		if m.closed {
			m.mu.Unlock()
			return true, nil
		}
		stream.mu.Lock()
		if !stream.closed {
			stream.credits[channel] += params.Bytes
		}
		start := !stream.started && !stream.closed
		stream.started = true
		stream.ready.Broadcast()
		stream.mu.Unlock()
		if start {
			m.pumps.Add(3)
		}
		m.mu.Unlock()
		if start {
			if err = m.notify("test/streamCredit", map[string]any{"stream": params.Stream, "bytes": 65536}); err != nil {
				m.pumps.Done()
				m.pumps.Done()
				m.pumps.Done()
				return true, err
			}
			go func() { defer m.pumps.Done(); m.pump(params.Stream, stream, 0, stream.process) }()
			go func() { defer m.pumps.Done(); m.pump(params.Stream, stream, 1, stream.errors) }()
			go func() {
				defer m.pumps.Done()
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
	m.closed = true
	streams := make([]*rustMapperStream, 0, len(m.streams))
	for _, stream := range m.streams {
		streams = append(streams, stream)
	}
	m.mu.Unlock()
	for _, stream := range streams {
		m.closeStream(stream)
	}
	m.pumps.Wait()
}
