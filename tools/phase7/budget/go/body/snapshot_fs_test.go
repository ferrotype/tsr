package budget

import (
	"errors"
	"io/fs"
	"runtime"
	"testing"
	"time"
	"unsafe"

	"github.com/microsoft/TypeScript/tsc/internal/vfs/iovfs"
)

func TestImmutableSourceSharesBackingAndMatchesPinnedDecoding(t *testing.T) {
	for _, raw := range [][]byte{
		{}, []byte("hello"), {0xff, 'x'},
		{0xef, 0xbb, 0xbf, 'x'}, {0xef, 0xbb, 0xbf},
		{0xff, 0xfe, 'x', 0}, {0xfe, 0xff, 0, 'x'},
		{0xff, 0xfe, 0x00, 0xd8, 'a', 0, 0xff},
	} {
		data := Bytes(raw)
		s, err := newSnapshot(&Input{CurrentDirectory: Bytes("/p"), CaseSensitive: true,
			Entries: []Entry{{Kind: "physical_file", Path: Bytes("/p/a.ts"), Bytes: &data}}})
		if err != nil {
			t.Fatal(err)
		}
		got, ok := immutableSnapshotFS(s).ReadFile("/p/a.ts")
		want, wantOK := iovfs.From(s, true).ReadFile("/p/a.ts")
		if got != want || ok != wantOK {
			t.Fatalf("decode %x: %q %v != %q %v", raw, got, ok, want, wantOK)
		}
		utf16 := len(raw) >= 2 && ((raw[0] == 0xff && raw[1] == 0xfe) || (raw[0] == 0xfe && raw[1] == 0xff))
		if len(got) > 0 && !utf16 {
			backing := s.entries[s.key("/p/a.ts")].data
			if len(raw) >= 3 && raw[0] == 0xef && raw[1] == 0xbb && raw[2] == 0xbf {
				backing = backing[3:]
			}
			if unsafe.StringData(got) != unsafe.SliceData(backing) {
				t.Fatal("compiler source does not share immutable snapshot backing")
			}
		}
		// io/fs callers may mutate their result without changing retained text.
		owned, err := fs.ReadFile(s, "p/a.ts")
		if err != nil {
			t.Fatal(err)
		}
		for i := range owned {
			owned[i] = '?'
		}
		s = nil
		runtime.GC()
		if got != want {
			t.Fatal("retained source changed after mutating io/fs result and dropping host")
		}
	}
}

func TestSnapshotWritesArePermissionErrors(t *testing.T) {
	s, err := newSnapshot(&Input{CurrentDirectory: Bytes("/p"), CaseSensitive: true})
	if err != nil {
		t.Fatal(err)
	}
	f := immutableSnapshotFS(s)
	for _, err := range []error{
		f.WriteFile("/p/a.ts", "x"), f.AppendFile("/p/a.ts", "x"),
		f.Remove("/p"), f.Chtimes("/p", time.Time{}, time.Time{}),
		s.MkdirAll("p", 0755),
	} {
		if !errors.Is(err, fs.ErrPermission) {
			t.Fatalf("write must fail without panic: %v", err)
		}
	}
	if f.FileExists("/p/a.ts") || !f.DirectoryExists("/p") {
		t.Fatal("failed write mutated the snapshot")
	}
}
