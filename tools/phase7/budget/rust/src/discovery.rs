//! Untimed native acquisition over complete, frozen metadata. The regular
//! consumer and production wasm hosts never consult this OS reader.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::Path,
    sync::{Arc, Mutex},
};
use tsr_jsstring::JsString;
use tsr_vfs::{
    Entries, Error, FileContent, FileInfo, FileSystem, MemoryBuilder, MemorySnapshot, SnapshotId,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u8,
    kind: String,
    root_b64: String,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Entry {
    Directory {
        path_b64: String,
    },
    PhysicalFile {
        path_b64: String,
        raw_size: String,
    },
    Symlink {
        path_b64: String,
        target_b64: String,
    },
}
#[derive(Serialize)]
struct ReadEvent<'a> {
    version: u8,
    kind: &'static str,
    path_b64: String,
    canonical_path_b64: Option<String>,
    found: bool,
    raw_size: Option<String>,
    sha256: Option<&'a str>,
}
#[derive(Serialize)]
struct Complete {
    version: u8,
    kind: &'static str,
    read_events: String,
}
#[derive(Serialize)]
struct Failure<'a> {
    version: u8,
    kind: &'static str,
    message: &'a str,
}
struct Log {
    output: BufWriter<File>,
    read_events: u64,
}
impl Log {
    fn write(&mut self, event: &impl Serialize) -> Result<(), String> {
        serde_json::to_writer(&mut self.output, event).map_err(|e| e.to_string())?;
        self.output.write_all(b"\n").map_err(|e| e.to_string())?;
        self.output.flush().map_err(|e| e.to_string())
    }
}
struct RecordedFile {
    content: FileContent,
    sha256: String,
}
struct DiscoveryFs {
    metadata: MemorySnapshot,
    root: Vec<u8>,
    missing_external: Vec<Vec<u8>>,
    files: Mutex<HashMap<JsString, Arc<RecordedFile>>>,
    log: Arc<Mutex<Log>>,
}
pub(crate) fn decode(text: &str) -> Result<Vec<u8>, String> {
    let bytes = STANDARD.decode(text).map_err(|e| e.to_string())?;
    if STANDARD.encode(&bytes) != text {
        return Err("noncanonical base64 path".into());
    }
    Ok(bytes)
}
fn within(root: &[u8], path: &[u8]) -> bool {
    path == root || path.starts_with(root) && (root == b"/" || path.get(root.len()) == Some(&b'/'))
}
fn absolute(path: &[u8]) -> Result<(), String> {
    if !path.starts_with(b"/") || tsr_tspath::normalize(path).as_ref() != path {
        return Err("metadata paths must be normalized absolute POSIX paths".into());
    }
    Ok(())
}
fn ensure_missing(path: &[u8]) -> Result<(), Error> {
    match std::fs::metadata(tsr_vfs::os::native::path(path)) {
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(tsr_vfs::os::native::failure("stat", path, error).into()),
        Ok(_) => Err(Error::OutsideScope),
    }
}
#[cfg(unix)]
fn open_physical(path: &[u8]) -> Result<File, Error> {
    use rustix::fs::{open, openat, Mode, OFlags};
    use std::os::unix::ffi::OsStrExt;
    let failure = |error: rustix::io::Errno| {
        Error::from(tsr_vfs::os::native::failure("open", path, error.into()))
    };
    let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let mut directory = open("/", flags | OFlags::DIRECTORY, Mode::empty()).map_err(failure)?;
    let mut components = path
        .split(|byte| *byte == b'/')
        .filter(|part| !part.is_empty())
        .peekable();
    while let Some(component) = components.next() {
        let last = components.peek().is_none();
        let fd = openat(
            &directory,
            std::ffi::OsStr::from_bytes(component),
            if last {
                flags
            } else {
                flags | OFlags::DIRECTORY
            },
            Mode::empty(),
        )
        .map_err(failure)?;
        if last {
            return Ok(fd.into());
        }
        directory = fd;
    }
    Err(Error::InvalidPath)
}
#[cfg(not(unix))]
fn open_physical(_path: &[u8]) -> Result<File, Error> {
    Err(Error::Unsupported(
        "scoped physical discovery requires Unix no-follow acquisition",
    ))
}
fn io_error(error: impl ToString) -> Error {
    tsr_vfs::iofs::IoError::message(error.to_string()).into()
}
fn line(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, String> {
    let mut bytes = Vec::new();
    let count = reader
        .take(1024 * 1024 + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| e.to_string())?;
    if count == 0 {
        return Ok(None);
    }
    if count > 1024 * 1024 || !bytes.ends_with(b"\n") {
        return Err("oversized or unterminated metadata row".into());
    }
    Ok(Some(bytes))
}
impl DiscoveryFs {
    fn load(
        path: &Path,
        input: &crate::protocol::Input,
        log: Arc<Mutex<Log>>,
    ) -> Result<Self, String> {
        let mut reader = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
        let header: Header =
            serde_json::from_slice(&line(&mut reader)?.ok_or("missing metadata header")?)
                .map_err(|e| e.to_string())?;
        if header.version != 1 || header.kind != "header" {
            return Err("unsupported metadata header".into());
        }
        let root = decode(&header.root_b64)?;
        absolute(&root)?;
        if !within(&root, &input.current_directory) {
            return Err("cwd outside metadata scope".into());
        }
        let mut missing_external = Vec::new();
        let mut builder = MemoryBuilder::new(&input.current_directory, input.case_sensitive);
        builder.insert_directory(&root);
        while let Some(row) = line(&mut reader)? {
            let entry: Entry = serde_json::from_slice(&row).map_err(|e| e.to_string())?;
            let encoded = match &entry {
                Entry::Directory { path_b64 }
                | Entry::PhysicalFile { path_b64, .. }
                | Entry::Symlink { path_b64, .. } => path_b64,
            };
            let path = decode(encoded)?;
            absolute(&path)?;
            if !within(&root, &path) {
                return Err("metadata entry outside scope".into());
            }
            match entry {
                Entry::Directory { .. } => builder.insert_directory(&path),
                Entry::PhysicalFile { raw_size, .. } => {
                    let size = raw_size.parse::<u64>().map_err(|e| e.to_string())?;
                    if size.to_string() != raw_size {
                        return Err("noncanonical metadata size".into());
                    }
                    builder.insert_unavailable_file(&path, size);
                }
                Entry::Symlink { target_b64, .. } => {
                    let target = decode(&target_b64)?;
                    let absolute_target =
                        tsr_tspath::absolute(&target, &tsr_tspath::directory(&path));
                    if !within(&root, &absolute_target) {
                        ensure_missing(&path).map_err(|e| e.to_string())?;
                        missing_external.push(absolute_target);
                    }
                    builder.insert_symlink(&path, &target);
                }
            }
        }
        Ok(Self {
            metadata: builder.finish(),
            root,
            missing_external,
            files: Mutex::default(),
            log,
        })
    }
    fn event(&self, requested: &[u8], file: Option<(&[u8], &RecordedFile)>) -> Result<(), Error> {
        let mut log = self.log.lock().map_err(io_error)?;
        let event = ReadEvent {
            version: 1,
            kind: "read",
            path_b64: STANDARD.encode(requested),
            canonical_path_b64: file.map(|(path, _)| STANDARD.encode(path)),
            found: file.is_some(),
            raw_size: file.map(|(_, file)| file.content.raw.len().to_string()),
            sha256: file.map(|(_, file)| file.sha256.as_str()),
        };
        log.write(&event).map_err(io_error)?;
        log.read_events += 1;
        Ok(())
    }
}
impl FileSystem for DiscoveryFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.metadata.use_case_sensitive_file_names()
    }
    // The OS acquisition is deliberately live; do not publish a resolver cache
    // under an immutable identity until the content archive has been verified.
    fn snapshot_id(&self) -> Option<SnapshotId> {
        None
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        let requested = tsr_tspath::absolute(path, self.metadata.current_directory());
        let canonical = self.metadata.realpath(path)?;
        if !within(&self.root, canonical.as_bytes()) {
            if !self
                .missing_external
                .iter()
                .any(|target| within(target, canonical.as_bytes()))
            {
                return Err(Error::OutsideScope);
            }
            ensure_missing(canonical.as_bytes())?;
            self.event(&requested, None)?;
            return Ok(None);
        }
        let Some(info) = self.metadata.stat(path)?.filter(|info| !info.directory) else {
            self.event(&requested, None)?;
            return Ok(None);
        };
        let cached = self
            .files
            .lock()
            .map_err(io_error)?
            .get(&canonical)
            .cloned();
        let file = if let Some(file) = cached {
            file
        } else {
            let native = tsr_vfs::os::native::path(canonical.as_bytes());
            let physical = std::fs::canonicalize(&native)
                .map_err(|e| tsr_vfs::os::native::failure("realpath", canonical.as_bytes(), e))?;
            if tsr_vfs::os::native::bytes(&physical) != canonical.as_bytes() {
                return Err(io_error("physical path changed since metadata capture"));
            }
            let mut file = open_physical(canonical.as_bytes())?;
            let before = file.metadata().map_err(io_error)?;
            if !before.is_file() || before.len() != info.size {
                return Err(io_error("physical metadata changed before read"));
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(io_error)?;
            let after = file.metadata().map_err(io_error)?;
            if bytes.len() as u64 != info.size
                || after.len() != before.len()
                || after.modified().map_err(io_error)? != before.modified().map_err(io_error)?
            {
                return Err(io_error("physical file changed during read"));
            }
            let recorded = Arc::new(RecordedFile {
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                content: FileContent::physical(bytes),
            });
            let mut files = self.files.lock().map_err(io_error)?;
            let existing = files
                .entry(canonical.clone())
                .or_insert_with(|| recorded.clone());
            if existing.sha256 != recorded.sha256 {
                return Err(io_error("physical file changed between concurrent reads"));
            }
            existing.clone()
        };
        self.event(&requested, Some((canonical.as_bytes(), &file)))?;
        Ok(Some(file.content.clone()))
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        self.metadata.stat(path)
    }
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        self.metadata.entries(path)
    }
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        self.metadata.realpath(path)
    }
}

fn capture_path(output: &Path) -> Result<(), String> {
    let forbidden = |path: &Path| {
        path.components().any(|component| {
            component
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case("target")
        })
    };
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent.canonicalize().map_err(|e| e.to_string())?;
    if forbidden(output) || forbidden(&parent) {
        return Err("discovery output must be outside every target ancestor".into());
    }
    Ok(())
}

pub fn run(metadata: &Path, input: &Path, output: &Path) -> Result<(), String> {
    capture_path(output)?;
    let log = Arc::new(Mutex::new(Log {
        output: BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output)
                .map_err(|e| e.to_string())?,
        ),
        read_events: 0,
    }));
    let result = (|| {
        let input: crate::protocol::Input =
            serde_json::from_reader(File::open(input).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let fs = Arc::new(DiscoveryFs::load(metadata, &input, log.clone())?);
        let fs: Arc<dyn FileSystem> = Arc::new(tsr_embed::BundledFs::new(fs));
        crate::engine::discover(input, fs)
    })();
    let mut log = log.lock().map_err(|e| e.to_string())?;
    match &result {
        Ok(()) => {
            let count = log.read_events.to_string();
            log.write(&Complete {
                version: 1,
                kind: "complete",
                read_events: count,
            })?;
        }
        Err(message) => log.write(&Failure {
            version: 1,
            kind: "failure",
            message,
        })?,
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        root: std::path::PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "phase7-rust-discovery-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            Self { root }
        }
        fn bytes(&self) -> Vec<u8> {
            tsr_vfs::os::native::bytes(&self.root)
        }
        fn input(&self) -> crate::protocol::Input {
            serde_json::from_value(json!({"current_directory":self.bytes(), "case_sensitive":true,"entries":[],"configuration":{"config_path":tsr_vfs::os::native::bytes(&self.root.join("tsconfig.json")),"arguments":[]},"operation":"check_emit"})).unwrap()
        }
        fn manifest(&self, entries: &[Value]) -> std::path::PathBuf {
            let path = self.root.join("metadata.ndjson");
            let mut file = File::create(&path).unwrap();
            writeln!(
                file,
                "{}",
                json!({"version":1,"kind":"header","root_b64":STANDARD.encode(self.bytes())})
            )
            .unwrap();
            for entry in entries {
                writeln!(file, "{entry}").unwrap();
            }
            path
        }
        fn file(&self, name: &str, bytes: &[u8]) -> Value {
            let path = self.root.join(name);
            std::fs::write(&path, bytes).unwrap();
            json!({"kind":"physical_file","path_b64":STANDARD.encode(tsr_vfs::os::native::bytes(&path)),"raw_size":bytes.len().to_string()})
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn real_config_discovery_logs_raw_loaded_closure_and_complete_only() {
        let fixture = Fixture::new();
        let source = b"\xef\xbb\xbfexport const value: number = 'invalid';";
        let manifest = fixture.manifest(&[
            fixture.file(
                "tsconfig.json",
                br#"{"compilerOptions":{"noLib":true},"files":["main.ts"]}"#,
            ),
            fixture.file("main.ts", source),
            fixture.file("unread.bin", &[0, 255, 42]),
        ]);
        let input = fixture.root.join("input.json");
        // The discovery input contains no physical transport payload.
        let request = json!({"current_directory":fixture.bytes(), "case_sensitive":true,"entries":[],"configuration":{"config_path":tsr_vfs::os::native::bytes(&fixture.root.join("tsconfig.json")),"arguments":[]},"operation":"check_emit"});
        std::fs::write(&input, serde_json::to_vec(&request).unwrap()).unwrap();
        let output = fixture.root.join("events.ndjson");
        run(&manifest, &input, &output).unwrap();
        let events: Vec<Value> = std::fs::read_to_string(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.last().unwrap()["kind"], "complete");
        assert_eq!(
            events.last().unwrap()["read_events"],
            (events.len() - 1).to_string()
        );
        let main = STANDARD.encode(tsr_vfs::os::native::bytes(&fixture.root.join("main.ts")));
        let unread = STANDARD.encode(tsr_vfs::os::native::bytes(&fixture.root.join("unread.bin")));
        assert!(events
            .iter()
            .any(|event| event["canonical_path_b64"] == main
                && event["sha256"] == format!("{:x}", Sha256::digest(source))));
        assert!(!events
            .iter()
            .any(|event| event["canonical_path_b64"] == unread));
        assert!(
            run(&manifest, &input, &output).is_err(),
            "discovery never overwrites prior evidence"
        );
    }

    #[test]
    fn capture_refuses_target_ancestors() {
        let fixture = Fixture::new();
        let target = fixture.root.join("TaRgEt");
        std::fs::create_dir(&target).unwrap();
        assert!(capture_path(&target.join("events.ndjson")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, fixture.root.join("alias")).unwrap();
            assert!(capture_path(&fixture.root.join("alias/events.ndjson")).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn scoped_aliases_and_external_dangling_links_preserve_missing_and_fail_on_change() {
        let fixture = Fixture::new();
        let physical = fixture.file("main.ts", b"\xef\xbb\xbfx");
        std::os::unix::fs::symlink("main.ts", fixture.root.join("alias.ts")).unwrap();
        let outside = fixture.root.with_extension("missing");
        std::os::unix::fs::symlink(&outside, fixture.root.join("dangling.ts")).unwrap();
        let manifest = fixture.manifest(&[
            physical,
            json!({"kind":"symlink","path_b64":STANDARD.encode(tsr_vfs::os::native::bytes(&fixture.root.join("alias.ts"))),"target_b64":STANDARD.encode(b"main.ts")}),
            json!({"kind":"symlink","path_b64":STANDARD.encode(tsr_vfs::os::native::bytes(&fixture.root.join("dangling.ts"))),"target_b64":STANDARD.encode(tsr_vfs::os::native::bytes(&outside))}),
        ]);
        let log = Arc::new(Mutex::new(Log {
            output: BufWriter::new(File::create(fixture.root.join("log")).unwrap()),
            read_events: 0,
        }));
        let fs = DiscoveryFs::load(&manifest, &fixture.input(), log).unwrap();
        assert_eq!(fs.snapshot_id(), None);
        let source = fs
            .read_file(&tsr_vfs::os::native::bytes(&fixture.root.join("alias.ts")))
            .unwrap()
            .unwrap();
        assert_eq!(source.raw.as_ref(), b"\xef\xbb\xbfx");
        assert_eq!(source.text.as_bytes(), b"x");
        assert!(fs
            .read_file(&tsr_vfs::os::native::bytes(
                &fixture.root.join("dangling.ts")
            ))
            .unwrap()
            .is_none());
        assert!(matches!(
            fs.read_file(b"/unrecorded-outside"),
            Err(Error::OutsideScope)
        ));
        std::fs::write(&outside, b"appeared").unwrap();
        let result = fs.read_file(&tsr_vfs::os::native::bytes(
            &fixture.root.join("dangling.ts"),
        ));
        std::fs::remove_file(&outside).unwrap();
        assert!(matches!(result, Err(Error::OutsideScope)));
    }
}
