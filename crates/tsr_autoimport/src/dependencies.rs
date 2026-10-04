//! Reads of the auxiliary export programs are watch dependencies even when
//! their files are not included in the user's compiler program.
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use tsr_jsstring::JsString;
use tsr_vfs::{
    wrapped::{Replacements, WrappedFs},
    FileSystem,
};

#[derive(Clone, Debug, Default)]
pub struct Dependencies {
    pub files: BTreeSet<JsString>,
    pub directories: BTreeSet<JsString>,
}
impl Dependencies {
    pub fn affected(&self, path: &[u8], case_sensitive: bool) -> bool {
        let key = tsr_tspath::to_path(path, b"", case_sensitive);
        let normalize = |p: &JsString| tsr_tspath::to_path(p.as_bytes(), b"", case_sensitive);
        let within = |child: &[u8], parent: &[u8]| {
            child == parent
                || child
                    .strip_prefix(parent)
                    .is_some_and(|rest| rest.starts_with(b"/"))
        };
        self.files
            .iter()
            .any(|p| within(normalize(p).as_bytes(), key.as_bytes()))
            || self.directories.iter().any(|p| {
                let p = normalize(p);
                within(key.as_bytes(), p.as_bytes()) || within(p.as_bytes(), key.as_bytes())
            })
    }
}

#[derive(Default)]
pub struct DependencyTracker(Arc<Mutex<Dependencies>>);
impl DependencyTracker {
    pub fn wrap(&self, host: Arc<dyn FileSystem>) -> Arc<dyn FileSystem> {
        let files = self.0.clone();
        let file_host = host.clone();
        let reads = self.0.clone();
        let read_host = host.clone();
        let directories = self.0.clone();
        let directory_host = host.clone();
        let missing_directories = self.0.clone();
        let missing_host = host.clone();
        let realpaths = self.0.clone();
        let realpath_host = host.clone();
        Arc::new(WrappedFs::new(
            host,
            Replacements {
                directory_exists: Some(Arc::new(move |path| {
                    if path.ends_with(b"/node_modules")
                        || path
                            .windows(b"/node_modules/".len())
                            .any(|s| s == b"/node_modules/")
                    {
                        missing_directories
                            .lock()
                            .unwrap()
                            .directories
                            .insert(JsString::from_bytes(path));
                    }
                    missing_host.directory_exists(path)
                })),
                realpath: Some(Arc::new(move |path| {
                    let real = realpath_host.realpath(path)?;
                    let directory = realpath_host.directory_exists(path)?;
                    let mut dependencies = realpaths.lock().unwrap();
                    let set = if directory {
                        &mut dependencies.directories
                    } else {
                        &mut dependencies.files
                    };
                    set.insert(JsString::from_bytes(path));
                    set.insert(real.clone());
                    Ok(real)
                })),
                file_exists: Some(Arc::new(move |path| {
                    files
                        .lock()
                        .unwrap()
                        .files
                        .insert(JsString::from_bytes(path));
                    file_host.file_exists(path)
                })),
                read_file_result: Some(Arc::new(move |path| {
                    reads
                        .lock()
                        .unwrap()
                        .files
                        .insert(JsString::from_bytes(path));
                    read_host.read_file_result(path)
                })),
                entries: Some(Arc::new(move |path| {
                    directories
                        .lock()
                        .unwrap()
                        .directories
                        .insert(JsString::from_bytes(path));
                    directory_host.entries(path)
                })),
                ..Default::default()
            },
        ))
    }
    pub fn snapshot(&self) -> Dependencies {
        self.0.lock().unwrap().clone()
    }
}
