//! Iterative, callback-controlled walks for immutable read hosts.
use crate::{Error, FileInfo, FileSystem};
use tsr_jsstring::JsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalkControl {
    Continue,
    SkipDir,
    SkipAll,
}
#[derive(Clone, Debug)]
pub struct WalkEntry {
    pub name: JsString,
    pub info: FileInfo,
    pub symlink: bool,
}
pub type WalkCallback<'a> =
    dyn FnMut(&[u8], Option<&WalkEntry>, Option<Error>) -> Result<WalkControl, Error> + 'a;

pub(crate) fn walk<F: FileSystem + ?Sized>(
    fs: &F,
    root: &[u8],
    visit: &mut WalkCallback<'_>,
) -> Result<(), Error> {
    let mut stack = Vec::new();
    let mut next = Some((root.to_vec(), false));
    loop {
        let (path, symlink) = if let Some(next) = next.take() {
            next
        } else {
            let mut found = None;
            while let Some(children) = stack.last_mut() {
                if let Some(child) = Iterator::next(children) {
                    found = Some(child);
                    break;
                }
                stack.pop();
            }
            let Some(found) = found else {
                return Ok(());
            };
            found
        };
        let info = fs
            .stat(&path)
            .and_then(|entry| entry.ok_or(Error::Io(std::io::ErrorKind::NotFound)));
        let failure = info.as_ref().err().cloned();
        let entry = info.ok().map(|info| WalkEntry {
            name: JsString::from_bytes(path.rsplit(|&byte| byte == b'/').next().unwrap_or(&path)),
            info,
            symlink,
        });
        let control = visit(&path, entry.as_ref(), failure)?;
        match control {
            WalkControl::SkipAll => return Ok(()),
            WalkControl::SkipDir => {
                if !entry.as_ref().is_some_and(|e| e.info.directory) {
                    stack.pop();
                }
                continue;
            }
            WalkControl::Continue => {}
        }
        let Some(entry) = entry.filter(|entry| entry.info.directory && !entry.symlink) else {
            continue;
        };
        let children = match fs.entries(&path) {
            Ok(children) => children,
            Err(error) => {
                match visit(&path, Some(&entry), Some(error))? {
                    WalkControl::SkipAll => return Ok(()),
                    WalkControl::SkipDir | WalkControl::Continue => {}
                }
                continue;
            }
        };
        let mut names = children.files.unwrap_or_default();
        names.extend(children.directories.unwrap_or_default());
        names.sort();
        let rows: Vec<_> = names
            .into_iter()
            .map(|name| {
                let link = children
                    .symlinks
                    .as_ref()
                    .is_some_and(|links| links.contains(&name));
                let mut child = path.clone();
                if !child.ends_with(b"/") {
                    child.push(b'/');
                }
                child.extend_from_slice(name.as_bytes());
                (child, link)
            })
            .collect();
        stack.push(rows.into_iter());
    }
}

/// An explicitly retained callback. Unlike a borrowed visitor, this may be
/// kept by a recorder after traversal; captures must own their dependencies.
pub type OwnedWalkCallback = std::sync::Arc<
    dyn Fn(&[u8], Option<&WalkEntry>, Option<Error>) -> Result<WalkControl, Error> + Send + Sync,
>;
