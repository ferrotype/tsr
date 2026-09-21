use super::helpers::{chmod, io_error, mkdir, sys, write, Env};
use serde_json::{json, Value};
use tsr_vfs::os::native;
pub fn fixture(e: &Env, name: &str) -> Result<Value, String> {
    // The table is fixture construction, in the same order as the native probe.
    let items: &[(&str, &str, &str)] = match name {
        "empty" => &[],
        "kinds" => &[
            ("dir", "real_dir", ""),
            ("file", "real_file.ts", "one"),
            ("file", "empty.ts", ""),
            ("symlink", "link_to_file", "real_file.ts"),
            ("symlink", "link_to_dir", "real_dir"),
            ("symlink", "dangling", "no_such_target"),
            ("hardlink", "hard.ts", "real_file.ts"),
            ("fifo", "fifo", ""),
            ("dir", "denied", ""),
            ("file", "denied/hidden.ts", "hidden"),
            ("chmod000", "denied", ""),
        ],
        "entries" => &[
            ("dir", "target", ""),
            ("dir", "link", ""),
            ("file", "target/file1", "hello"),
            ("file", "target/file2", "world"),
            ("dir", "target/dir1", ""),
            ("dir", "target/dir2", ""),
            ("symlink", "link/file1", "target/file1"),
            ("symlink", "link/file2", "target/file2"),
            ("symlink", "link/dir1", "target/dir1"),
            ("symlink", "link/dir2", "target/dir2"),
            ("symlink", "link/dangling", "target/gone"),
            ("fifo", "link/fifo", ""),
            ("hardlink", "link/hard", "target/file1"),
            ("file", "plain.ts", "plain"),
            ("dir", "empty_dir", ""),
            ("dir", "blocked", ""),
            ("file", "blocked/inside.ts", "inside"),
            ("chmod000", "blocked", ""),
        ],
        "links" => &[
            ("dir", "target", ""),
            ("file", "target/file", "hello"),
            ("symlink", "link", "target"),
            ("symlink", "link2", "link"),
            ("symlink", "link3", "link2"),
            ("symlink", "dangling", "nowhere"),
            ("symlink", "loop_a", "loop_b"),
            ("symlink", "loop_b", "loop_a"),
            ("symlink", "self", ""),
        ],
        "tree" => &[
            ("dir", "tree/a", ""),
            ("dir", "tree/c/c1", ""),
            ("file", "tree/a/a1", "1"),
            ("file", "tree/a/a2", "2"),
            ("file", "tree/b", "b"),
            ("file", "tree/c/c1/c2", "c"),
            ("symlink", "tree/a/linkc", "tree/c"),
            ("dir", "tree/adenied", ""),
            ("file", "tree/adenied/hidden", "h"),
            ("chmod000", "tree/adenied", ""),
            ("dir", "other/x", ""),
            ("file", "other/x/y", "y"),
            ("file", "other/z", "z"),
            ("symlink", "linkdir", "tree/a"),
        ],
        _ => return Err(format!("unknown fixture {name}")),
    };
    let mut rows = Vec::new();
    for (kind, rel, value) in items {
        let p = e.path(rel);
        let error = match *kind {
            "dir" => sys("mkdir", mkdir(&p, 0o777)),
            "file" => sys("open", write(&p, value.as_bytes(), 0o666)),
            "chmod000" => sys("chmod", chmod(&p, 0)),
            "symlink" => sys(
                "symlink",
                std::os::unix::fs::symlink(native::path(&e.path(value)), native::path(&p)),
            ),
            "hardlink" => sys(
                "link",
                std::fs::hard_link(native::path(&e.path(value)), native::path(&p)),
            ),
            "fifo" => {
                // Fixture-only creation. macOS has no mkfifoat, and the
                // workspace forbids raw FFI; use its system utility.
                let status = std::process::Command::new("mkfifo")
                    .arg(native::path(&p))
                    .status()
                    .map_err(|e| format!("run fixture mkfifo: {e}"))?;
                if !status.success() {
                    return Err(format!("fixture mkfifo failed: {status}"));
                }
                io_error(None)
            }
            _ => unreachable!(),
        };
        rows.push(json!([rel, kind, error]));
    }
    Ok(json!(rows))
}
