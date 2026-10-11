package budget

import (
	"bytes"
	"fmt"
	"io"
	"io/fs"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

// An fs.FS backing production iovfs: its ReadFile performs the pinned physical
// decoding. There is no OS FS or test fixture wrapper beneath this snapshot.
type snapshot struct {
	cwd       string
	sensitive bool
	entries   map[string]node
	children  map[string][]fs.DirEntry
	errorMu   sync.Mutex
	failure   error
}
type node struct {
	path, kind, target string
	data               []byte
	rawSize            int64
}

func (s *snapshot) key(path string) string { return string(tspath.ToPath(path, s.cwd, s.sensitive)) }
func absolute(path, cwd string) string     { return tspath.GetNormalizedAbsolutePath(path, cwd) }
func (s *snapshot) insert(n node) {
	n.path = absolute(n.path, s.cwd)
	for p := tspath.GetDirectoryPath(n.path); ; p = tspath.GetDirectoryPath(p) {
		if _, ok := s.entries[s.key(p)]; !ok {
			s.entries[s.key(p)] = node{path: p, kind: "directory"}
		}
		if tspath.GetDirectoryPath(p) == p {
			break
		}
	}
	s.entries[s.key(n.path)] = n
}
func (s *snapshot) resolve(path string) (string, error) {
	path = absolute(path, s.cwd)
	for range 40 {
		changed := false
		// Root-to-leaf prefixes match MemorySnapshot's ancestor resolution.
		prefixes := []string{path}
		for p := tspath.GetDirectoryPath(path); ; p = tspath.GetDirectoryPath(p) {
			prefixes = append(prefixes, p)
			if tspath.GetDirectoryPath(p) == p {
				break
			}
		}
		slices.Reverse(prefixes)
		for _, p := range prefixes {
			if n, ok := s.entries[s.key(p)]; ok && n.kind == "symlink" {
				path = tspath.NormalizePath(absolute(n.target, tspath.GetDirectoryPath(p)) + path[len(p):])
				changed = true
				break
			}
		}
		if !changed {
			return path, nil
		}
	}
	return "", fmt.Errorf("snapshot symlink cycle")
}
func (s *snapshot) lookup(path string) (node, error) {
	p, err := s.resolve(path)
	if err != nil {
		s.recordError(err)
		return node{}, err
	}
	n, ok := s.entries[s.key(p)]
	if !ok {
		return node{}, fs.ErrNotExist
	}
	return n, nil
}
func (s *snapshot) Realpath(name string) (string, error) {
	p, err := s.resolve("/" + strings.TrimPrefix(name, "/"))
	if err != nil {
		s.recordError(err)
		return "", err
	}
	if n, ok := s.entries[s.key(p)]; ok {
		p = n.path
	}
	return strings.TrimPrefix(p, "/"), nil
}
func (s *snapshot) Open(name string) (fs.File, error) {
	if !fs.ValidPath(name) {
		return nil, fs.ErrInvalid
	}
	n, err := s.lookup("/" + name)
	if err != nil {
		return nil, err
	}
	if n.kind == "unavailable_file" {
		err := fmt.Errorf("snapshot file content is unavailable: %s", name)
		s.recordError(err)
		return nil, err
	}
	f := &opened{node: n, reader: bytes.NewReader(n.data)}
	if n.kind == "directory" {
		f.entries = s.children[s.key(n.path)]
	}
	return f, nil
}
func (s *snapshot) Stat(name string) (fs.FileInfo, error) {
	if !fs.ValidPath(name) {
		return nil, fs.ErrInvalid
	}
	n, err := s.lookup("/" + name)
	if err != nil {
		return nil, err
	}
	return entryInfo{n}, nil
}
func (s *snapshot) recordError(err error) {
	s.errorMu.Lock()
	defer s.errorMu.Unlock()
	if s.failure == nil {
		s.failure = err
	}
}
func (s *snapshot) error() error { s.errorMu.Lock(); defer s.errorMu.Unlock(); return s.failure }

type entryInfo struct{ n node }

func (e entryInfo) Name() string { return tspath.GetBaseFileName(e.n.path) }
func (e entryInfo) Size() int64  { return e.n.rawSize }
func (e entryInfo) Mode() fs.FileMode {
	if e.n.kind == "directory" {
		return fs.ModeDir | 0555
	}
	if e.n.kind == "symlink" {
		return fs.ModeSymlink | 0444
	}
	return 0444
}
func (e entryInfo) ModTime() time.Time         { return time.Time{} }
func (e entryInfo) IsDir() bool                { return e.n.kind == "directory" }
func (e entryInfo) Sys() any                   { return nil }
func (e entryInfo) Type() fs.FileMode          { return e.Mode().Type() }
func (e entryInfo) Info() (fs.FileInfo, error) { return e, nil }

type opened struct {
	node    node
	reader  *bytes.Reader
	entries []fs.DirEntry
	offset  int
}

func (f *opened) Stat() (fs.FileInfo, error) { return entryInfo{f.node}, nil }
func (f *opened) Close() error               { return nil }
func (f *opened) Read(p []byte) (int, error) {
	if f.node.kind == "directory" {
		return 0, fs.ErrInvalid
	}
	return f.reader.Read(p)
}
func (f *opened) ReadDir(n int) ([]fs.DirEntry, error) {
	if f.node.kind != "directory" {
		return nil, fs.ErrInvalid
	}
	if n > 0 && f.offset == len(f.entries) {
		return nil, io.EOF
	}
	end := len(f.entries)
	if n > 0 && end > f.offset+n {
		end = f.offset + n
	}
	result := f.entries[f.offset:end]
	f.offset = end
	return result, nil
}
func newSnapshot(input *Input) (*snapshot, error) {
	cwd := string(input.CurrentDirectory)
	if !strings.HasPrefix(cwd, "/") || tspath.NormalizePath(cwd) != cwd {
		return nil, fmt.Errorf("prototype requires normalized absolute POSIX cwd")
	}
	s := &snapshot{cwd: cwd, sensitive: input.CaseSensitive, entries: map[string]node{}}
	s.insert(node{path: cwd, kind: "directory"})
	for _, e := range input.Entries {
		if !strings.HasPrefix(string(e.Path), "/") || tspath.NormalizePath(string(e.Path)) != string(e.Path) {
			return nil, fmt.Errorf("prototype requires normalized absolute POSIX entry path")
		}
		n := node{path: string(e.Path), kind: e.Kind}
		switch e.Kind {
		case "directory":
			if e.Bytes != nil || e.Target != nil {
				return nil, fmt.Errorf("directory payload")
			}
		case "physical_file":
			if e.Bytes == nil || e.Target != nil {
				return nil, fmt.Errorf("file payload")
			}
			n.data = bytes.Clone(*e.Bytes)
			n.rawSize = int64(len(n.data))
		case "symlink":
			if e.Target == nil || e.Bytes != nil {
				return nil, fmt.Errorf("symlink payload")
			}
			n.target = string(*e.Target)
		default:
			return nil, fmt.Errorf("unsupported snapshot entry")
		}
		s.insert(n)
	}
	s.indexChildren()
	return s, nil
}
func (s *snapshot) indexChildren() {
	s.children = map[string][]fs.DirEntry{}
	for _, entry := range s.entries {
		parent := tspath.GetDirectoryPath(entry.path)
		if parent != entry.path {
			key := s.key(parent)
			s.children[key] = append(s.children[key], entryInfo{entry})
		}
	}
	for _, children := range s.children {
		slices.SortFunc(children, func(a, b fs.DirEntry) int { return strings.Compare(a.Name(), b.Name()) })
	}
}
