// Install this access-only overlay at internal/testutil/contentmappertest/s11_mapper_test.go.
// Every response comes from the pinned test mapper over its real framed pipe.
// No transform, mapping tuple, or option-diagnostic construction is duplicated.
package contentmappertest_test

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/microsoft/TypeScript/tsc/internal/ipc"
	upstreamjson "github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/jsonrpc"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/contentmappertest"
)

type s11MapperRequest struct {
	Method string          `json:"method"`
	Params json.RawMessage `json:"params"`
}

type s11MapperFixture struct {
	ID       string             `json:"id"`
	Mapper   string             `json:"mapper"`
	Requests []s11MapperRequest `json:"requests"`
}

type s11MapperResponse struct {
	Result upstreamjson.Value     `json:"result,omitzero"`
	Error  *jsonrpc.ResponseError `json:"error,omitzero"`
}

type s11MapperObservation struct {
	ID          string              `json:"id"`
	Responses   []s11MapperResponse `json:"responses"`
	ServerBytes []byte              `json:"server_bytes"`
	PluginBytes []byte              `json:"plugin_bytes"`
}

// Record actual successful IO bytes before the mapper protocol parses them.
// Read boundaries are deliberately not part of the comparison.
type s11RecordingStream struct {
	io.ReadWriteCloser
	sent     bytes.Buffer
	received bytes.Buffer
}

func (s *s11RecordingStream) Write(p []byte) (int, error) {
	n, err := s.ReadWriteCloser.Write(p)
	s.sent.Write(p[:n])
	return n, err
}
func (s *s11RecordingStream) Read(p []byte) (int, error) {
	n, err := s.ReadWriteCloser.Read(p)
	s.received.Write(p[:n])
	return n, err
}

func s11ObserveMapper(t *testing.T, fixture s11MapperFixture) s11MapperObservation {
	t.Helper()
	ctx, cancel := context.WithTimeout(t.Context(), 20*time.Second)
	defer cancel()
	conn, err := contentmappertest.NewSpawner().Spawn([]string{fixture.Mapper}, "/node_modules/mapper", io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	stopClosing := context.AfterFunc(ctx, func() { _ = conn.Close() })
	defer stopClosing()
	// NewSpawner returns net.Pipe. Bound each write/read as well as the whole
	// fixture so an oracle protocol regression cannot strand this capture.
	deadlines, ok := conn.(interface{ SetDeadline(time.Time) error })
	if !ok {
		t.Fatal("S11 mapper spawner no longer supports bounded pipe operations")
	}
	recorded := &s11RecordingStream{ReadWriteCloser: conn}
	protocol := ipc.NewJSONRPCProtocol(recorded)
	observation := s11MapperObservation{ID: fixture.ID, Responses: []s11MapperResponse{}}
	for index, request := range fixture.Requests {
		if request.Method == "" || len(request.Params) == 0 {
			t.Fatalf("invalid S11 mapper request %q[%d]", fixture.ID, index)
		}
		if err := ctx.Err(); err != nil {
			t.Fatal(err)
		}
		if err := deadlines.SetDeadline(time.Now().Add(5 * time.Second)); err != nil {
			t.Fatal(err)
		}
		id := jsonrpc.NewIDString(fmt.Sprintf("%s:%d", fixture.ID, index))
		if err := protocol.WriteRequest(id, request.Method, upstreamjson.Value(request.Params)); err != nil {
			t.Fatalf("S11 mapper %q request %d write: %v", fixture.ID, index, err)
		}
		response, err := protocol.ReadMessage()
		if err != nil {
			t.Fatalf("S11 mapper %q request %d read: %v", fixture.ID, index, err)
		}
		if !response.IsResponse() || response.ID == nil || *response.ID != *id {
			t.Fatalf("S11 mapper %q request %d response identity mismatch", fixture.ID, index)
		}
		if (len(response.Result) != 0) == (response.Error != nil) {
			t.Fatalf("S11 mapper %q request %d requires exactly one result or error", fixture.ID, index)
		}
		observation.Responses = append(observation.Responses, s11MapperResponse{
			Result: response.Result, Error: response.Error,
		})
	}
	observation.ServerBytes = recorded.sent.Bytes()
	observation.PluginBytes = recorded.received.Bytes()
	return observation
}

func TestS11Mapper(t *testing.T) {
	input, output := os.Getenv("S11_MAPPER_INPUT"), os.Getenv("S11_MAPPER_OUTPUT")
	if input == "" || output == "" {
		t.Fatal("S11_MAPPER_INPUT and S11_MAPPER_OUTPUT are required")
	}
	raw, err := os.ReadFile(input)
	if err != nil {
		t.Fatal(err)
	}
	if !utf8.Valid(raw) {
		t.Fatal("S11 mapper fixture document must be UTF-8")
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	var fixtures []s11MapperFixture
	if err := decoder.Decode(&fixtures); err != nil {
		t.Fatal(err)
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		t.Fatal("trailing S11 mapper fixture data")
	}
	if len(fixtures) == 0 {
		t.Fatal("empty S11 mapper fixture list")
	}
	seen := make(map[string]bool, len(fixtures))
	observations := make([]s11MapperObservation, 0, len(fixtures))
	for _, fixture := range fixtures {
		if fixture.ID == "" || seen[fixture.ID] || fixture.Mapper == "" || len(fixture.Requests) == 0 || len(fixture.Requests) > 32 {
			t.Fatalf("invalid or duplicate S11 mapper fixture %q", fixture.ID)
		}
		seen[fixture.ID] = true
		observations = append(observations, s11ObserveMapper(t, fixture))
	}
	encoded, err := upstreamjson.Marshal(observations)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(output, encoded, 0o600); err != nil {
		t.Fatal(err)
	}
}
