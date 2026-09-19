// This access-only overlay is installed as internal/api/s11_callbackfs_test.go.
// It calls the pinned callbackFS; the scripted connection supplies fixture inputs,
// not an implementation of callback selection, fallback, or filesystem behavior.
// Fixtures use valid UTF-8 JSON. Upstream's JSON marshaler replaces invalid UTF-8;
// this oracle does not establish arbitrary-byte transport parity.
package api

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"testing"
	"unicode/utf8"

	upstreamjson "github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

type s11Fixture struct {
	ID            string            `json:"id"`
	CaseSensitive bool              `json:"caseSensitive"`
	Base          map[string]string `json:"base"`
	Symlinks      map[string]string `json:"symlinks"`
	Operation     string            `json:"operation"`
	Path          string            `json:"path"`
	Enabled       bool              `json:"enabled"`
	Response      json.RawMessage   `json:"response"`
}

type s11Callback struct {
	Method string `json:"method"`
	Params any    `json:"params"`
}

type s11Observation struct {
	ID        string        `json:"id"`
	Result    any           `json:"result"`
	Callbacks []s11Callback `json:"callbacks"`
}

type s11Connection struct {
	fixture   s11Fixture
	callbacks []s11Callback
}

func (c *s11Connection) Run(context.Context) error {
	return errors.New("S11 oracle does not run a transport")
}

func (c *s11Connection) Call(ctx context.Context, method string, params any) (upstreamjson.Value, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if method != c.fixture.Operation {
		return nil, fmt.Errorf("S11 callback method drift: got %q, expected %q", method, c.fixture.Operation)
	}
	c.callbacks = append(c.callbacks, s11Callback{Method: method, Params: params})
	return upstreamjson.Value(c.fixture.Response), nil
}

func (c *s11Connection) Notify(context.Context, string, any) error {
	return errors.New("S11 filesystem observation unexpectedly emitted a notification")
}

func TestS11CallbackFS(t *testing.T) {
	input, output := os.Getenv("S11_INPUT"), os.Getenv("S11_OUTPUT")
	if input == "" || output == "" {
		t.Fatal("S11_INPUT and S11_OUTPUT are required")
	}
	raw, err := os.ReadFile(input)
	if err != nil {
		t.Fatal(err)
	}
	if !utf8.Valid(raw) {
		t.Fatal("S11 fixture document must be UTF-8")
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	var fixtures []s11Fixture
	if err := decoder.Decode(&fixtures); err != nil {
		t.Fatal(err)
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		t.Fatal("trailing S11 fixture data")
	}
	if len(fixtures) == 0 {
		t.Fatal("empty S11 fixture list")
	}
	seen := make(map[string]bool, len(fixtures))
	observations := make([]s11Observation, 0, len(fixtures))
	for _, fixture := range fixtures {
		if fixture.ID == "" || seen[fixture.ID] || len(fixture.Response) == 0 {
			t.Fatalf("invalid or duplicate S11 fixture %q", fixture.ID)
		}
		seen[fixture.ID] = true
		base := make(map[string]any, len(fixture.Base)+len(fixture.Symlinks))
		for path, content := range fixture.Base {
			base[path] = content
		}
		for path, target := range fixture.Symlinks {
			if _, exists := base[path]; exists {
				t.Fatalf("S11 fixture %q has a file and symlink at %q", fixture.ID, path)
			}
			base[path] = vfstest.Symlink(target)
		}
		callbacks := []string{}
		if fixture.Enabled {
			callbacks = append(callbacks, fixture.Operation)
		}
		fs := newCallbackFS(vfstest.FromMap(base, fixture.CaseSensitive), callbacks)
		conn := &s11Connection{fixture: fixture, callbacks: []s11Callback{}}
		fs.SetConnection(context.Background(), conn)
		var result any
		switch fixture.Operation {
		case callbackReadFile:
			content, ok := fs.ReadFile(fixture.Path)
			var value any
			if ok {
				value = content
			}
			result = map[string]any{"content": value}
		case callbackFileExists:
			result = fs.FileExists(fixture.Path)
		case callbackDirectoryExists:
			result = fs.DirectoryExists(fixture.Path)
		case callbackGetAccessibleEntries:
			entries := fs.GetAccessibleEntries(fixture.Path)
			// The S11 callback wire contract has two lists, no symlink metadata.
			// Normalize nil lists only; retain upstream entry order and spelling.
			result = map[string]any{
				"files":       append([]string{}, entries.Files...),
				"directories": append([]string{}, entries.Directories...),
			}
		case callbackRealpath:
			result = fs.Realpath(fixture.Path)
		default:
			t.Fatalf("unknown S11 operation %q", fixture.Operation)
		}
		observations = append(observations, s11Observation{
			ID: fixture.ID, Result: result, Callbacks: conn.callbacks,
		})
	}
	encoded, err := upstreamjson.Marshal(observations)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(output, encoded, 0o600); err != nil {
		t.Fatal(err)
	}
}
