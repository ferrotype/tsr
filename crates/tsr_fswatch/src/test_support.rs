use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
pub(crate) struct TempDir(pub(crate) PathBuf);
impl TempDir {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let dir = std::env::temp_dir().join(format!(
                "tsr-fswatch-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Self(std::fs::canonicalize(dir).unwrap()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("create temporary watch directory: {e}"),
            }
        }
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
