//go:build !js

package budget

// Native acquisition is an untimed preparation tool. Normal native and wasm
// operation hosts remain frozen memory snapshots with no OS fallback.
import (
	"bufio"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"math"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"

	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/iovfs"
)

type discoveryFile struct {
	raw  []byte
	hash string
}
type discoveryFS struct {
	*snapshot
	root            string
	rootHandle      *os.Root
	missingExternal []string
	output          *json.Encoder
	mu              sync.Mutex
	files           map[string]discoveryFile
	reads           uint64
	failure         error
}
type readEvent struct {
	Version   int     `json:"version"`
	Kind      string  `json:"kind"`
	Path      string  `json:"path_b64"`
	Canonical *string `json:"canonical_path_b64"`
	Found     bool    `json:"found"`
	Size      *string `json:"raw_size"`
	Hash      *string `json:"sha256"`
}

func within(root, path string) bool {
	return path == root || strings.HasPrefix(path, root) && (root == "/" || len(path) > len(root) && path[len(root)] == '/')
}
func encoded(path string) string { return base64.StdEncoding.EncodeToString([]byte(path)) }
func decoded(raw json.RawMessage) (string, error) {
	var text string
	if err := json.Unmarshal(raw, &text); err != nil {
		return "", err
	}
	data, err := base64.StdEncoding.DecodeString(text)
	if err != nil || base64.StdEncoding.EncodeToString(data) != text {
		return "", fmt.Errorf("noncanonical base64 path")
	}
	return string(data), nil
}
func normalized(path string) error {
	if !strings.HasPrefix(path, "/") || tspath.NormalizePath(path) != path {
		return fmt.Errorf("metadata paths must be normalized absolute POSIX paths")
	}
	return nil
}
func metadataRow(reader *bufio.Reader) ([]byte, error) {
	var result []byte
	for {
		part, err := reader.ReadSlice('\n')
		if len(result)+len(part) > 1024*1024 {
			return nil, fmt.Errorf("oversized metadata row")
		}
		result = append(result, part...)
		if err == bufio.ErrBufferFull {
			continue
		}
		if err != nil {
			if err == io.EOF && len(result) > 0 {
				return nil, fmt.Errorf("unterminated metadata row")
			}
			return result, err
		}
		return result, nil
	}
}
func metadataObject(row []byte, names ...string) (map[string]json.RawMessage, error) {
	return strictObject(row, names...)
}
func ensureMissing(path string) error {
	_, err := os.Stat(path)
	if errors.Is(err, fs.ErrNotExist) || errors.Is(err, syscall.ENOTDIR) {
		return nil
	}
	if err != nil {
		return err
	}
	return fmt.Errorf("external metadata symlink target exists")
}
func loadDiscovery(metadata string, input *Input, output *json.Encoder) (*discoveryFS, error) {
	if len(input.Entries) != 0 {
		return nil, fmt.Errorf("discovery requires empty input.entries")
	}
	file, err := os.Open(metadata)
	if err != nil {
		return nil, err
	}
	defer file.Close()
	reader := bufio.NewReader(file)
	row, err := metadataRow(reader)
	if err != nil {
		return nil, err
	}
	header, err := metadataObject(row, "version", "kind", "root_b64")
	if err != nil {
		return nil, err
	}
	var version int
	var headerKind string
	if json.Unmarshal(header["version"], &version) != nil || json.Unmarshal(header["kind"], &headerKind) != nil || version != 1 || headerKind != "header" {
		return nil, fmt.Errorf("unsupported metadata header")
	}
	root, err := decoded(header["root_b64"])
	if err != nil {
		return nil, err
	}
	if err := normalized(root); err != nil {
		return nil, err
	}
	if !within(root, string(input.CurrentDirectory)) {
		return nil, fmt.Errorf("cwd outside metadata scope")
	}
	s, err := newSnapshot(input)
	if err != nil {
		return nil, err
	}
	s.insert(node{path: root, kind: "directory"})
	d := &discoveryFS{snapshot: s, root: root, output: output, files: map[string]discoveryFile{}}
	for {
		row, err := metadataRow(reader)
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, err
		}
		var kind struct {
			Kind string `json:"kind"`
		}
		if err := json.Unmarshal(row, &kind); err != nil {
			return nil, err
		}
		names := []string{"kind", "path_b64"}
		switch kind.Kind {
		case "directory":
		case "physical_file":
			names = append(names, "raw_size")
		case "symlink":
			names = append(names, "target_b64")
		default:
			return nil, fmt.Errorf("unknown metadata entry")
		}
		values, err := metadataObject(row, names...)
		if err != nil {
			return nil, err
		}
		path, err := decoded(values["path_b64"])
		if err != nil {
			return nil, err
		}
		if err := normalized(path); err != nil {
			return nil, err
		}
		if !within(root, path) {
			return nil, fmt.Errorf("metadata entry outside scope")
		}
		n := node{path: path, kind: kind.Kind}
		if kind.Kind == "physical_file" {
			var size string
			if err := json.Unmarshal(values["raw_size"], &size); err != nil {
				return nil, err
			}
			parsed, err := strconv.ParseUint(size, 10, 64)
			if err != nil || strconv.FormatUint(parsed, 10) != size || parsed > math.MaxInt64 {
				return nil, fmt.Errorf("invalid metadata raw_size")
			}
			n.rawSize = int64(parsed)
		} else if kind.Kind == "symlink" {
			n.target, err = decoded(values["target_b64"])
			if err != nil {
				return nil, err
			}
			target := absolute(n.target, tspath.GetDirectoryPath(path))
			if !within(root, target) {
				if err := ensureMissing(path); err != nil {
					return nil, err
				}
				d.missingExternal = append(d.missingExternal, target)
			}
		}
		s.insert(n)
	}
	s.indexChildren()
	physicalRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		return nil, err
	}
	if physicalRoot != root {
		return nil, fmt.Errorf("metadata scope root is not canonical")
	}
	d.rootHandle, err = os.OpenRoot(root)
	if err != nil {
		return nil, err
	}
	return d, nil
}
func (d *discoveryFS) event(requested string, canonical *string, file *discoveryFile) error {
	event := readEvent{Version: 1, Kind: "read", Path: encoded(requested), Found: file != nil}
	if canonical != nil {
		value := encoded(*canonical)
		event.Canonical = &value
	}
	if file != nil {
		value := strconv.Itoa(len(file.raw))
		event.Size = &value
		event.Hash = &file.hash
	}
	if err := d.output.Encode(event); err != nil {
		return err
	}
	d.reads++
	return nil
}
func (d *discoveryFS) ReadFile(name string) (raw []byte, err error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	defer func() {
		// Only our explicit frozen-metadata miss is ordinary absence. A wrapped
		// OS ENOENT for a recorded physical file means the capture went stale.
		if err != nil && err != fs.ErrNotExist {
			d.failure = err
		}
	}()
	if d.failure != nil {
		return nil, d.failure
	}
	requested := absolute("/"+strings.TrimPrefix(name, "/"), d.cwd)
	if !within(d.root, requested) {
		allowed := false
		for _, target := range d.missingExternal {
			allowed = allowed || within(target, requested)
		}
		if !allowed {
			return nil, fmt.Errorf("raw read outside metadata scope: %s", requested)
		}
		if err := ensureMissing(requested); err != nil {
			return nil, err
		}
		if err := d.event(requested, nil, nil); err != nil {
			return nil, err
		}
		return nil, fs.ErrNotExist
	}
	n, lookupErr := d.lookup(requested)
	if lookupErr != nil && !errors.Is(lookupErr, fs.ErrNotExist) {
		return nil, lookupErr
	}
	if lookupErr != nil || n.kind != "physical_file" {
		if err := d.event(requested, nil, nil); err != nil {
			return nil, err
		}
		return nil, fs.ErrNotExist
	}
	canonical := n.path
	if !within(d.root, canonical) {
		return nil, fmt.Errorf("physical read outside metadata scope")
	}
	file, cached := d.files[d.key(canonical)]
	if !cached {
		physical, err := filepath.EvalSymlinks(canonical)
		if err != nil {
			return nil, err
		}
		if physical != canonical {
			return nil, fmt.Errorf("physical path changed since metadata capture")
		}
		relative, err := filepath.Rel(d.root, canonical)
		if err != nil {
			return nil, err
		}
		// Root.Open prevents symlink escapes at open; no-follow component checks
		// additionally reject changes to metadata-declared physical paths.
		parts := strings.Split(relative, string(filepath.Separator))
		for index := range parts {
			info, err := d.rootHandle.Lstat(filepath.Join(parts[:index+1]...))
			if err != nil {
				return nil, err
			}
			if info.Mode()&os.ModeSymlink != 0 {
				return nil, fmt.Errorf("physical path became a symlink")
			}
		}
		handle, err := d.rootHandle.Open(relative)
		if err != nil {
			return nil, err
		}
		defer handle.Close()
		before, err := handle.Stat()
		if err != nil {
			return nil, err
		}
		if !before.Mode().IsRegular() || before.Size() != n.rawSize {
			return nil, fmt.Errorf("physical metadata changed before read")
		}
		data, err := io.ReadAll(handle)
		if err != nil {
			return nil, err
		}
		after, err := handle.Stat()
		if err != nil {
			return nil, err
		}
		if int64(len(data)) != n.rawSize || after.Size() != before.Size() || !after.ModTime().Equal(before.ModTime()) {
			return nil, fmt.Errorf("physical file changed during read")
		}
		file = discoveryFile{raw: data, hash: fmt.Sprintf("%x", sha256.Sum256(data))}
		d.files[d.key(canonical)] = file
	}
	if err := d.event(requested, &canonical, &file); err != nil {
		return nil, err
	}
	return file.raw, nil
}
func Discover(metadata, inputPath, outputPath string) (err error) {
	absoluteOutput, err := filepath.Abs(outputPath)
	if err != nil {
		return err
	}
	parent, err := filepath.EvalSymlinks(filepath.Dir(absoluteOutput))
	if err != nil {
		return err
	}
	for _, path := range []string{absoluteOutput, parent} {
		for _, part := range strings.Split(path, string(filepath.Separator)) {
			if strings.EqualFold(part, "target") {
				return fmt.Errorf("discovery captures must be outside target")
			}
		}
	}
	output, err := os.OpenFile(outputPath, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	defer output.Close()
	encoder := json.NewEncoder(output)
	defer func() {
		if err != nil {
			_ = encoder.Encode(map[string]any{"version": 1, "kind": "failure", "message": err.Error()})
		}
	}()
	raw, err := os.ReadFile(inputPath)
	if err != nil {
		return err
	}
	// Reuse the strict v1 validator rather than a second permissive Input decoder.
	envelope := append([]byte(`{"version":1,"command":"prepare","id":"0","input":`), raw...)
	envelope = append(envelope, '}')
	command, err := Decode(envelope)
	if err != nil {
		return err
	}
	d, err := loadDiscovery(metadata, command.Input, encoder)
	if err != nil {
		return err
	}
	defer d.rootHandle.Close()
	e := &Engine{input: command.Input, snapshot: d.snapshot, fs: bundled.WrapFS(iovfs.From(d, d.sensitive))}
	if err := e.execute(); err != nil {
		return err
	}
	if d.failure != nil {
		return d.failure
	}
	return encoder.Encode(map[string]any{"version": 1, "kind": "complete", "read_events": strconv.FormatUint(d.reads, 10)})
}
