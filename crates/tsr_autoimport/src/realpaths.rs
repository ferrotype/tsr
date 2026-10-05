//! Canonical package identities, scoped to one export extraction. A changed
//! filesystem gets a new instance rather than reusing cached symlink targets.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tsr_jsstring::JsString;
use tsr_vfs::{Error, FileSystem};

pub(crate) struct PackagePaths {
    host: Arc<dyn FileSystem>,
    directory: JsString,
    real_directory: JsString,
    dependencies: Mutex<HashMap<Vec<u8>, JsString>>,
}
impl PackagePaths {
    // port: tsc/internal/ls/autoimport/util.go:getPackageRealpathFuncs
    pub(crate) fn new(host: Arc<dyn FileSystem>, directory: &[u8]) -> Result<Self, Error> {
        Ok(Self {
            real_directory: host.realpath(directory)?,
            directory: JsString::from_bytes(directory),
            host,
            dependencies: Mutex::new(HashMap::new()),
        })
    }
    pub(crate) fn to_realpath(&self, file: &[u8]) -> Result<JsString, Error> {
        if self.directory != self.real_directory {
            if let Some(rest) = file.strip_prefix(self.directory.as_bytes()) {
                return Ok(JsString::from_bytes(
                    [self.real_directory.as_bytes(), rest].concat(),
                ));
            }
        }
        let directory = tsr_module::parse_node_module_from_path(file, false);
        if directory.is_empty() {
            return Ok(JsString::from_bytes(file));
        }
        // Do not invoke an arbitrary filesystem under the cache lock.
        let known = self.dependencies.lock().unwrap().get(&directory).cloned();
        let real = if let Some(real) = known {
            real
        } else {
            let real = self.host.realpath(&directory)?;
            self.dependencies
                .lock()
                .unwrap()
                .insert(directory.clone(), real.clone());
            real
        };
        Ok(JsString::from_bytes(
            [real.as_bytes(), &file[directory.len()..]].concat(),
        ))
    }
    pub(crate) fn to_symlink(&self, file: &[u8]) -> JsString {
        if self.directory != self.real_directory {
            if let Some(rest) = file.strip_prefix(self.real_directory.as_bytes()) {
                return JsString::from_bytes([self.directory.as_bytes(), rest].concat());
            }
        }
        JsString::from_bytes(file)
    }
    pub(crate) fn wrapped(self: &Arc<Self>) -> Arc<dyn FileSystem> {
        let paths = self.clone();
        Arc::new(tsr_vfs::wrapped::WrappedFs::new(
            self.host.clone(),
            tsr_vfs::wrapped::Replacements {
                realpath: Some(Arc::new(move |file| paths.to_realpath(file))),
                ..Default::default()
            },
        ))
    }
}
