package budget

import (
	"io/fs"
	"strings"
	"unsafe"

	"github.com/microsoft/TypeScript/tsc/internal/vfs"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/iovfs"
)

// The compiler's string-facing filesystem may share immutable snapshot backing.
// In contrast, io/fs.ReadFile callers own mutable bytes: Open continues to copy
// into the caller's buffer, and the snapshot does not implement ReadFileFS.
type snapshotFS struct {
	vfs.FS
	snapshot *snapshot
}

func immutableSnapshotFS(s *snapshot) vfs.FS {
	return &snapshotFS{FS: iovfs.From(s, s.sensitive), snapshot: s}
}

func (s *snapshotFS) ReadFile(path string) (string, bool) {
	if !strings.HasPrefix(path, "/") || !fs.ValidPath(strings.TrimPrefix(path, "/")) {
		return s.FS.ReadFile(path)
	}
	n, err := s.snapshot.lookup(path)
	if err != nil {
		return "", false
	}
	if n.kind != "physical_file" {
		// Preserve unavailable-content errors as well as directory behavior.
		return s.FS.ReadFile(path)
	}
	data := n.data
	if len(data) >= 2 && ((data[0] == 0xff && data[1] == 0xfe) || (data[0] == 0xfe && data[1] == 0xff)) {
		// Use the pinned decoder for UTF-16, including malformed/odd input.
		return s.FS.ReadFile(path)
	}
	if len(data) >= 3 && data[0] == 0xef && data[1] == 0xbb && data[2] == 0xbf {
		data = data[3:]
	}
	// Snapshot preparation privately owns these bytes; after publication no
	// method mutates or exports them. The string retains the backing allocation
	// after the host is dropped, just as the pin's Common.ReadFile string does.
	return unsafe.String(unsafe.SliceData(data), len(data)), true
}
