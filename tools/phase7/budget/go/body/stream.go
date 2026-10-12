package budget

import (
	"bytes"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"runtime"
	"strconv"
	"strings"

	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

const streamFrameLimit = 2 * 1024 * 1024
const streamChunkLimit = 1024 * 1024

// A source cannot exceed the observation-frame cap. Check before allocating
// its declared storage; corrupt metadata must not request an unbounded block.
const streamContentLimit = 256 * 1024 * 1024

type streamContent struct {
	path, hash string
	size       uint64
	data       []byte
	received   uint64
}
type streamPreparation struct {
	expectedEntries, expectedFiles, expectedContents uint64
	entries, files, contents                         uint64
	paths, loaded                                    map[string]bool
	content                                          *streamContent
	contentStarted                                   bool
}

func strictObject(raw []byte, names ...string) (map[string]json.RawMessage, error) {
	scan := json.NewDecoder(bytes.NewReader(raw))
	scan.UseNumber()
	if err := uniqueJSON(scan); err != nil {
		return nil, err
	}
	if _, err := scan.Token(); err != io.EOF {
		return nil, fmt.Errorf("trailing JSON")
	}
	fields, err := required(raw, names...)
	if err != nil {
		return nil, err
	}
	if err := keys(fields, names...); err != nil {
		return nil, err
	}
	return fields, nil
}
func decimal(raw json.RawMessage) (uint64, error) {
	var value string
	if err := json.Unmarshal(raw, &value); err != nil {
		return 0, err
	}
	n, err := strconv.ParseUint(value, 10, 64)
	if err != nil || strconv.FormatUint(n, 10) != value {
		return 0, fmt.Errorf("noncanonical unsigned decimal")
	}
	return n, nil
}
func base64Bytes(raw json.RawMessage) ([]byte, error) {
	var value string
	if err := json.Unmarshal(raw, &value); err != nil {
		return nil, err
	}
	data, err := base64.StdEncoding.DecodeString(value)
	if err != nil || base64.StdEncoding.EncodeToString(data) != value {
		return nil, fmt.Errorf("noncanonical base64")
	}
	return data, nil
}
func streamPath(raw json.RawMessage) (string, error) {
	value, err := base64Bytes(raw)
	if err != nil {
		return "", err
	}
	path := string(value)
	if !strings.HasPrefix(path, "/") || tspath.NormalizePath(path) != path {
		return "", fmt.Errorf("stream requires normalized absolute POSIX path")
	}
	return path, nil
}
func (e *Engine) streamCommand(raw []byte) (response Response, err error) {
	response.Version = 2
	if e.failed {
		return response, fmt.Errorf("budget consumer terminal after failure")
	}
	defer func() {
		if err != nil {
			e.failed = true
		}
	}()
	if len(raw) > streamFrameLimit {
		return response, fmt.Errorf("v2 frame exceeds 2 MiB")
	}
	var peek struct {
		Command string `json:"command"`
	}
	if err := json.Unmarshal(raw, &peek); err != nil {
		return response, err
	}
	names := []string{"version", "command", "id"}
	switch peek.Command {
	case "prepare_begin":
		names = append(names, "input", "expected_entries", "expected_files", "expected_contents")
	case "prepare_metadata":
		names = append(names, "entries")
	case "content_begin":
		names = append(names, "path_b64", "raw_size", "sha256")
	case "content_chunk":
		names = append(names, "offset", "bytes_b64")
	case "observe_source", "observe_write":
		names = append(names, "index")
	case "content_end", "prepare_end", "execute", "checkpoint", "observe", "observe_begin", "release":
	default:
		return response, fmt.Errorf("unknown v2 command")
	}
	fields, err := strictObject(raw, names...)
	if err != nil {
		return response, err
	}
	var version int
	if json.Unmarshal(fields["version"], &version) != nil || version != 2 {
		return response, fmt.Errorf("unsupported stream version")
	}
	var id string
	if json.Unmarshal(fields["id"], &id) != nil {
		return response, fmt.Errorf("invalid id")
	}
	if _, err := decimal(fields["id"]); err != nil {
		return response, err
	}
	response.ID = id
	if peek.Command == "prepare_begin" {
		if e.state != "" || e.preparation != nil || e.protocolVersion != 0 {
			return response, fmt.Errorf("prepare_begin requires empty consumer")
		}
		// Decode the same strict input shape as v1, with no entry payload.
		envelope := append([]byte(`{"version":1,"command":"prepare","id":"0","input":`), fields["input"]...)
		envelope = append(envelope, '}')
		command, err := Decode(envelope)
		if err != nil {
			return response, err
		}
		if len(command.Input.Entries) != 0 {
			return response, fmt.Errorf("prepare_begin input.entries must be empty")
		}
		if command.Input.Operation != "check" && command.Input.Operation != "check_emit" {
			return response, fmt.Errorf("unsupported operation")
		}
		s, err := newSnapshot(command.Input)
		if err != nil {
			return response, err
		}
		p := &streamPreparation{paths: map[string]bool{}, loaded: map[string]bool{}}
		p.expectedEntries, err = decimal(fields["expected_entries"])
		if err != nil {
			return response, err
		}
		p.expectedFiles, err = decimal(fields["expected_files"])
		if err != nil {
			return response, err
		}
		p.expectedContents, err = decimal(fields["expected_contents"])
		if err != nil {
			return response, err
		}
		if p.expectedFiles > p.expectedEntries || p.expectedContents > p.expectedFiles {
			return response, fmt.Errorf("invalid declared preparation counts")
		}
		e.id = id
		e.protocolVersion = 2
		e.preparation = p
		e.input = command.Input
		e.snapshot = s
		e.state = "preparing"
		response.Kind = "preparing"
		return response, nil
	}
	if e.id != id || e.protocolVersion != 2 {
		return response, fmt.Errorf("unknown retained v2 operation")
	}
	p := e.preparation
	if p == nil {
		if peek.Command == "observe_begin" || peek.Command == "observe_source" || peek.Command == "observe_write" {
			if e.state != "checkpointed" {
				return response, fmt.Errorf("bounded observation requires checkpointed operation")
			}
			var index uint64
			if peek.Command != "observe_begin" {
				index, err = decimal(fields["index"])
				if err != nil {
					return response, err
				}
			}
			if e.observation == nil {
				value, freezeErr := e.observe()
				if freezeErr != nil {
					return response, freezeErr
				}
				e.observation = &value
			}
			switch peek.Command {
			case "observe_begin":
				response.Kind = "observation_begin"
				response.Value = ObservationHeader{Observation: *e.observation, SourceCount: n(len(e.observation.Sources)), WriteCount: n(len(e.observation.Writes))}
			case "observe_source":
				if index >= uint64(len(e.observation.Sources)) {
					return response, fmt.Errorf("source index out of range")
				}
				response.Kind = "observation_source"
				response.Value = e.observation.Sources[index]
			case "observe_write":
				if index >= uint64(len(e.observation.Writes)) {
					return response, fmt.Errorf("write index out of range")
				}
				response.Kind = "observation_write"
				response.Value = e.observation.Writes[index]
			}
			return response, nil
		}
		if peek.Command != "execute" && peek.Command != "checkpoint" && peek.Command != "observe" && peek.Command != "release" {
			return response, fmt.Errorf("preparation already finalized")
		}
		internal, _ := json.Marshal(Command{Version: 1, Command: peek.Command, ID: id})
		response, err = e.executeV1(internal)
		response.Version = 2
		return response, err
	}
	switch peek.Command {
	case "prepare_metadata":
		if p.contentStarted || len(fields["entries"]) > streamChunkLimit {
			return response, fmt.Errorf("metadata after content or exceeds 1 MiB")
		}
		var entries []json.RawMessage
		if err := json.Unmarshal(fields["entries"], &entries); err != nil {
			return response, err
		}
		for _, rawEntry := range entries {
			var kind struct {
				Kind string `json:"kind"`
			}
			if err := json.Unmarshal(rawEntry, &kind); err != nil {
				return response, err
			}
			names := []string{"kind", "path_b64"}
			switch kind.Kind {
			case "directory":
			case "physical_file":
				names = append(names, "raw_size")
			case "symlink":
				names = append(names, "target_b64")
			default:
				return response, fmt.Errorf("unknown metadata kind")
			}
			entry, err := strictObject(rawEntry, names...)
			if err != nil {
				return response, err
			}
			path, err := streamPath(entry["path_b64"])
			if err != nil {
				return response, err
			}
			key := e.snapshot.key(path)
			if p.paths[key] {
				return response, fmt.Errorf("duplicate canonical metadata path")
			}
			p.paths[key] = true
			n := node{path: path, kind: kind.Kind}
			if kind.Kind == "physical_file" {
				size, err := decimal(entry["raw_size"])
				if err != nil || size > math.MaxInt64 {
					return response, fmt.Errorf("invalid physical raw_size")
				}
				n.kind = "unavailable_file"
				n.rawSize = int64(size)
				p.files++
			}
			if kind.Kind == "symlink" {
				target, err := base64Bytes(entry["target_b64"])
				if err != nil {
					return response, err
				}
				n.target = string(target)
			}
			e.snapshot.insert(n)
			p.entries++
			if p.entries > p.expectedEntries || p.files > p.expectedFiles {
				return response, fmt.Errorf("metadata exceeds declared counts")
			}
		}
		response.Kind = "metadata"
	case "content_begin":
		if p.content != nil || p.entries != p.expectedEntries || p.files != p.expectedFiles {
			return response, fmt.Errorf("content requires complete metadata and no open file")
		}
		path, err := streamPath(fields["path_b64"])
		if err != nil {
			return response, err
		}
		key := e.snapshot.key(path)
		n, known := e.snapshot.entries[key]
		if !known || n.kind != "unavailable_file" || !p.paths[key] || p.loaded[key] {
			return response, fmt.Errorf("content requires unique known physical placeholder")
		}
		size, err := decimal(fields["raw_size"])
		if err != nil || size != uint64(n.rawSize) || size > streamContentLimit {
			return response, fmt.Errorf("content size differs from metadata")
		}
		var hash string
		if json.Unmarshal(fields["sha256"], &hash) != nil || len(hash) != 64 {
			return response, fmt.Errorf("invalid sha256")
		}
		decoded, err := hex.DecodeString(hash)
		if err != nil || hex.EncodeToString(decoded) != hash {
			return response, fmt.Errorf("noncanonical sha256")
		}
		p.content = &streamContent{path: n.path, hash: hash, size: size, data: make([]byte, int(size))}
		p.contentStarted = true
		response.Kind = "content_started"
	case "content_chunk":
		if p.content == nil {
			return response, fmt.Errorf("chunk requires open content")
		}
		offset, err := decimal(fields["offset"])
		if err != nil || offset != p.content.received {
			return response, fmt.Errorf("noncontiguous content offset")
		}
		data, err := base64Bytes(fields["bytes_b64"])
		if err != nil {
			return response, err
		}
		if len(data) > streamChunkLimit || uint64(len(data)) > p.content.size-offset {
			return response, fmt.Errorf("chunk exceeds size or 1 MiB")
		}
		copy(p.content.data[int(offset):], data)
		p.content.received += uint64(len(data))
		response.Kind = "content_chunk"
	case "content_end":
		if p.content == nil {
			return response, fmt.Errorf("content_end requires open file")
		}
		content := p.content
		if content.received != content.size || fmt.Sprintf("%x", sha256.Sum256(content.data)) != content.hash {
			return response, fmt.Errorf("content byte count or hash mismatch")
		}
		key := e.snapshot.key(content.path)
		p.loaded[key] = true
		e.snapshot.entries[key] = node{path: content.path, kind: "physical_file", data: content.data, rawSize: int64(content.size)}
		p.content = nil
		p.contents++
		if p.contents > p.expectedContents {
			return response, fmt.Errorf("content exceeds declared count")
		}
		response.Kind = "content_verified"
	case "prepare_end":
		if p.content != nil || p.entries != p.expectedEntries || p.files != p.expectedFiles || p.contents != p.expectedContents {
			return response, fmt.Errorf("incomplete streamed snapshot")
		}
		e.snapshot.indexChildren()
		e.fs = bundled.WrapFS(immutableSnapshotFS(e.snapshot))
		e.preparation = nil
		e.input.Entries = nil
		e.state = "prepared"
		p = nil
		runtime.GC()
		response.Kind = "prepared"
	default:
		return response, fmt.Errorf("compiler stages require prepare_end")
	}
	return response, nil
}
