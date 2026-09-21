use super::{
    fixtures,
    helpers::{
        array, chmod, error, guarded, hex, in_dir, int, io_error, mkdir, mode, remove, size, sys,
        text, unhex, write, Env,
    },
    walks, writes,
};
use crate::api::subject;
use serde_json::{json, Value};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use tsr_vfs::{
    os::{self, native},
    FileSystem,
};
pub fn run(request: &Value) -> Result<Value, String> {
    let sub = subject(request);
    let e = Env::new()?;
    let fs = os::fs();
    let mut handles: Vec<&os::OsFs> = vec![];
    let mut remembered = 0;
    let mut walker = walks::State::default();
    let mut rows = vec![];
    for a in array(request, "actions")? {
        let op = text(a, "op")?;
        // Refuse mutations escaping this case's root before the panic observer.
        for key in [
            "path", "target", "link", "nested", "created", "probed", "name", "left", "right",
            "swapped",
        ] {
            if let Some(s) = a[key].as_str() {
                if Path::new(s).is_absolute() || s.split('/').any(|p| p == "..") {
                    return Err(format!("unsafe relative fixture {key}"));
                }
            }
        }
        if matches!(
            op,
            "write_literal"
                | "append_literal"
                | "write_with_flag_literal"
                | "write_ensuring_dir_literal"
                | "chtimes_literal"
        ) {
            let s = text(a, "literal")?;
            if s.starts_with('/') || s.contains(':') || s.split('/').any(|p| p == "..") {
                return Err("unsafe literal mutation".into());
            }
        }
        if let Some(h) = a.get("handle") {
            let index = h.as_u64().ok_or("invalid handle")? as usize;
            if index >= handles.len() {
                return Err("unacquired FS handle".into());
            }
        }
        rows.push(guarded(op, || {
            let path = || text(a, "path").map(|s| e.path(s));
            Ok(match op {
                "fixture" => fixtures::fixture(&e, text(a, "fixture")?)?,
                "host" => json!([
                    cfg!(windows),
                    cfg!(target_os = "linux"),
                    cfg!(target_arch = "wasm32")
                ]),
                "build" => json!([cfg!(target_os = "linux")]),
                "umask" => {
                    let m = rustix::process::umask(rustix::fs::Mode::empty());
                    rustix::process::umask(m);
                    json!([format!("0o{:03o}", m.as_raw_mode())])
                }
                "swap_case" | "swap_case_round_trip" => {
                    let b = unhex(text(a, "input_hex")?)?;
                    let once = os::swap_case(&b);
                    if op == "swap_case" {
                        json!([hex(&once), once.len(), b.len()])
                    } else {
                        let twice = os::swap_case(&once);
                        json!([hex(&twice), twice == b])
                    }
                }
                "use_case_sensitive_file_names" => json!([fs.use_case_sensitive_file_names()]),
                "swap_case_of_executable" => match native::executable() {
                    Err(e) => json!(["executable_error", io_error(Some(&e))]),
                    Ok(exe) => {
                        let s = os::swap_case(&exe);
                        let stat = std::fs::metadata(native::path(&s));
                        let missing = stat
                            .as_ref()
                            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
                        json!(["ok", s != exe, sys("stat", stat), missing])
                    }
                },
                "cross_check_on_disk" => {
                    let p = e.path(text(a, "probed")?);
                    let result = write(&e.path(text(a, "created")?), b"probe", 0o666);
                    if let Err(err) = result {
                        json!(["write_error", sys::<()>("open", Err(err))])
                    } else {
                        json!([
                            "ok",
                            std::fs::metadata(native::path(&p)).is_ok(),
                            fs.file_exists(&p).map_err(|e| e.to_string())?
                        ])
                    }
                }
                "args_class" => {
                    let args = native::args();
                    let own = std::env::args_os()
                        .map(|s| native::bytes(Path::new(&s)))
                        .collect::<Vec<_>>();
                    json!([
                        !args.is_empty(),
                        args.first().is_some_and(|a| !a.is_empty()),
                        args == own
                    ])
                }
                "args_repeat_identical" => {
                    let a = native::args();
                    let b = native::args();
                    json!([a == b, a.len() == b.len()])
                }
                "executable_class" => match native::executable() {
                    Err(e) => json!(["error", io_error(Some(&e))]),
                    Ok(exe) => {
                        let original = std::fs::metadata(native::path(&exe));
                        let resolved = native::realpath(&exe);
                        let same = resolved
                            .as_ref()
                            .ok()
                            .and_then(|p| std::fs::metadata(native::path(p)).ok())
                            .zip(original.as_ref().ok())
                            .is_some_and(|(a, b)| a.dev() == b.dev() && a.ino() == b.ino());
                        let raw = std::env::current_exe();
                        json!([
                            "ok",
                            native::path(&exe).is_absolute(),
                            original.is_ok(),
                            native::path(&exe).file_name().is_some(),
                            [
                                resolved.is_ok(),
                                same,
                                raw.is_ok_and(|p| native::bytes(&p) == exe)
                            ]
                        ])
                    }
                },
                "executable_stable_across_chdir" => {
                    let a = native::executable();
                    let b = in_dir(&e.root, native::executable);
                    json!([a.is_ok(), b.is_ok(), a == b])
                }
                "acquire" => {
                    let f = os::fs();
                    let same = handles.first().is_some_and(|old| std::ptr::eq(*old, f));
                    handles.push(f);
                    json!([handles.len() - 1, true, same])
                }
                "write" if sub == "osvfs.FS" => json!([error(
                    handles[int(a, "handle")? as usize]
                        .write_file(&path()?, text(a, "content")?.as_bytes())
                        .err()
                )]),
                "read" | "read_literal" => {
                    let p = if op == "read" {
                        path()?
                    } else {
                        text(a, "literal")?.as_bytes().to_vec()
                    };
                    let f = if sub == "osvfs.FS" {
                        handles[int(a, "handle")? as usize]
                    } else {
                        fs
                    };
                    let data = f.read_file(&p).map_err(|e| e.to_string())?;
                    let b = data.as_ref().map_or(b"".as_slice(), |d| d.raw.as_ref());
                    if sub == "osvfs.ReadFile" && op == "read" {
                        json!([hex(b), data.is_some(), b.len()])
                    } else {
                        json!([hex(b), data.is_some()])
                    }
                }
                "write_bytes" => {
                    let b = unhex(text(a, "content_hex")?)?;
                    json!([sys("open", write(&path()?, &b, 0o666)), b.len()])
                }
                "chmod" => json!([sys("chmod", chmod(&path()?, int(a, "mode")? as u32))]),
                "symlink" | "make_backslash_name" => {
                    let (target, p) = if op == "symlink" {
                        (e.path(text(a, "target")?), path()?)
                    } else {
                        (e.path("real_file.ts"), e.path(text(a, "name")?))
                    };
                    let err = sys(
                        "symlink",
                        std::os::unix::fs::symlink(native::path(&target), native::path(&p)),
                    );
                    if op == "symlink" {
                        json!([err])
                    } else {
                        json!([err, text(a, "name")?.contains('\\')])
                    }
                }
                "stat" | "stat_literal" | "stat_root_trailing_slash" => {
                    let p = match op {
                        "stat" => path()?,
                        "stat_literal" => text(a, "literal")?.as_bytes().to_vec(),
                        _ => [e.root.as_slice(), b"/"].concat(),
                    };
                    match fs.stat(&p).map_err(|e| e.to_string())? {
                        None => json!(["nil"]),
                        Some(i) => match op {
                            "stat" => json!([
                                "info",
                                e.name(text(a, "path")?, i.name.as_bytes()),
                                size(i.mode, i.size),
                                i.directory,
                                mode(i.mode),
                                !i.mod_time.is_zero()
                            ]),
                            "stat_literal" => json!([
                                "info",
                                e.place(i.name.as_bytes()),
                                size(i.mode, i.size),
                                i.directory
                            ]),
                            _ => json!(["info", i.directory, !i.name.is_empty()]),
                        },
                    }
                }
                "exists" | "exists_literal" | "exists_root_trailing_slash" => {
                    let p = match op {
                        "exists" => path()?,
                        "exists_literal" => text(a, "literal")?.as_bytes().to_vec(),
                        _ => [e.root.as_slice(), b"/"].concat(),
                    };
                    let file = fs.file_exists(&p).map_err(|e| e.to_string())?;
                    let dir = fs.directory_exists(&p).map_err(|e| e.to_string())?;
                    if op == "exists_root_trailing_slash" {
                        json!([
                            file,
                            dir,
                            fs.directory_exists(&e.root).map_err(|e| e.to_string())?
                        ])
                    } else if sub == "osvfs.Remove" {
                        json!([
                            std::fs::symlink_metadata(native::path(&p)).is_ok(),
                            file,
                            dir
                        ])
                    } else {
                        json!([file, dir])
                    }
                }
                "remove_and_recheck" => {
                    let p = path()?;
                    let a = fs.file_exists(&p).map_err(|e| e.to_string())?;
                    let err = sys("remove", remove(&p));
                    json!([a, err, fs.file_exists(&p).map_err(|e| e.to_string())?])
                }
                "entries" | "entries_literal" => {
                    let p = if op == "entries" {
                        path()?
                    } else {
                        text(a, "literal")?.as_bytes().to_vec()
                    };
                    let f = if sub == "osvfs.FS" {
                        handles[int(a, "handle")? as usize]
                    } else {
                        fs
                    };
                    let en = f.entries(&p).map_err(|e| e.to_string())?;
                    let files = crate::fs_trace::strings(en.files.as_deref().unwrap_or_default());
                    let dirs =
                        crate::fs_trace::strings(en.directories.as_deref().unwrap_or_default());
                    let syms = en.symlinks.as_ref().map_or(vec![], |s| {
                        s.iter()
                            .map(|s| String::from_utf8_lossy(s.as_bytes()).into_owned())
                            .collect::<Vec<_>>()
                    });
                    if sub == "osvfs.GetAccessibleEntries" && op == "entries" {
                        json!([
                            files,
                            dirs,
                            syms,
                            en.symlinks.is_some(),
                            files.len(),
                            dirs.len(),
                            syms.len()
                        ])
                    } else {
                        json!([files, dirs, syms, en.symlinks.is_some()])
                    }
                }
                "predicate" | "predicate_literal" => {
                    let p = if op == "predicate" {
                        path()?
                    } else {
                        text(a, "literal")?.as_bytes().to_vec()
                    };
                    let direct = native::is_symlink_or_reparse_point(&p);
                    let wrapped = os::is_reparse_point(&p);
                    if op == "predicate" {
                        json!([direct, wrapped, direct == wrapped])
                    } else {
                        json!([direct, wrapped])
                    }
                }
                "realpath"
                | "realpath_literal"
                | "realpath_trailing_slash"
                | "realpath_relative"
                    if sub.starts_with("nativepath.") =>
                {
                    let p = match op {
                        "realpath_literal" => text(a, "literal")?.as_bytes().to_vec(),
                        "realpath_relative" => text(a, "path")?.as_bytes().to_vec(),
                        "realpath_trailing_slash" => [path()?.as_slice(), b"/"].concat(),
                        _ => path()?,
                    };
                    let r = if op == "realpath_relative" {
                        in_dir(&e.root, || native::realpath(&p))
                    } else {
                        native::realpath(&p)
                    };
                    let got = r.as_ref().map_or(b"".as_slice(), Vec::as_slice);
                    if op == "realpath_relative" {
                        json!([
                            e.place(got),
                            io_error(r.as_ref().err()),
                            native::path(got).is_absolute()
                        ])
                    } else {
                        json!([e.place(got), io_error(r.as_ref().err())])
                    }
                }
                "realpath_long" => {
                    let depth = int(a, "depth")?;
                    let segment = text(a, "segment")?;
                    if !(0..=100).contains(&depth) || segment.contains('/') {
                        return Err("unsafe long path".into());
                    }
                    let mut p = e.root.clone();
                    for _ in 0..depth {
                        p.push(b'/');
                        p.extend_from_slice(segment.as_bytes());
                    }
                    if let Err(err) = mkdir(&p, 0o777) {
                        json!(["setup_error", sys::<()>("mkdir", Err(err))])
                    } else {
                        let r = native::realpath(&p);
                        let p = r.as_ref().map_or(b"".as_slice(), Vec::as_slice);
                        json!([
                            "ok",
                            io_error(r.as_ref().err()),
                            p.len() > 256,
                            p.ends_with(segment.as_bytes())
                        ])
                    }
                }
                "realpath" | "realpath_direct" | "realpath_literal" => {
                    let p = if op == "realpath_literal" {
                        text(a, "literal")?.as_bytes().to_vec()
                    } else {
                        path()?
                    };
                    let got = if op == "realpath" {
                        fs.realpath(&p)
                            .map_err(|e| e.to_string())?
                            .as_bytes()
                            .to_vec()
                    } else {
                        os::realpath(&p)
                    };
                    if op == "realpath" {
                        remembered += 1;
                        json!([e.place(&got), got == p, !got.contains(&b'\\')])
                    } else if op == "realpath_literal" && got == p {
                        json!([format!("literal:{}", String::from_utf8_lossy(&p)), true])
                    } else {
                        json!([e.place(&got), got == p])
                    }
                }
                "realpath_equal" | "realpath_case_swapped" => {
                    let (l, r) = if op == "realpath_equal" {
                        ("left", "right")
                    } else {
                        ("path", "swapped")
                    };
                    let left = fs
                        .realpath(&e.path(text(a, l)?))
                        .map_err(|e| e.to_string())?;
                    let right = fs
                        .realpath(&e.path(text(a, r)?))
                        .map_err(|e| e.to_string())?;
                    json!([
                        e.place(left.as_bytes()),
                        e.place(right.as_bytes()),
                        left == right
                    ])
                }
                "stable_repeat" => {
                    let p = path()?;
                    let a = fs.realpath(&p).map_err(|e| e.to_string())?;
                    let err = sys("remove", remove(&p));
                    let b = fs.realpath(&p).map_err(|e| e.to_string())?;
                    json!([e.place(a.as_bytes()), err, e.place(b.as_bytes()), a == b])
                }
                "remembered_count" => json!([remembered]),
                "location_repeat_identical" => json!([
                    os::global_typings_cache_location() == os::global_typings_cache_location()
                ]),
                "version_tail" => {
                    let p = os::global_typings_cache_location();
                    let expected = text(a, "expected_suffix")?;
                    json!([
                        p.ends_with(format!("/{expected}").as_bytes()),
                        expected.matches('.').count(),
                        expected
                    ])
                }
                "location_shape" => {
                    let p = os::global_typings_cache_location();
                    let cache = if cfg!(target_os = "macos") {
                        std::env::var_os("HOME")
                            .filter(|h| !h.is_empty())
                            .map(|h| std::path::PathBuf::from(h).join("Library/Caches"))
                    } else {
                        std::env::var_os("XDG_CACHE_HOME")
                            .filter(|h| !h.is_empty())
                            .map(std::path::PathBuf::from)
                            .or_else(|| {
                                std::env::var_os("HOME")
                                    .filter(|h| !h.is_empty())
                                    .map(|h| std::path::PathBuf::from(h).join(".cache"))
                            })
                    };
                    let parts = p.split(|b| *b == b'/').collect::<Vec<_>>();
                    json!([
                        !p.is_empty(),
                        cache.is_some(),
                        cache
                            .as_ref()
                            .is_some_and(|c| p.starts_with(&native::bytes(c))),
                        p.starts_with(&native::bytes(&std::env::temp_dir())),
                        !p.contains(&b'\\'),
                        !p.windows(2).any(|w| w == b"//"),
                        String::from_utf8_lossy(parts[parts.len() - 2]),
                        String::from_utf8_lossy(parts[parts.len() - 1]),
                        parts.len(),
                        p.starts_with(b"/")
                    ])
                }
                _ if sub == "osvfs.WalkDir" || sub == "osvfs.WalkBinding" => {
                    walks::action(&e, a, &mut walker)?
                }
                _ => writes::action(&e, a)?,
            })
        })?);
    }
    Ok(json!({"ordered":rows}))
}
