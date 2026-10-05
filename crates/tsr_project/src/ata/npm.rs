use super::{check_context, AtaError};
use std::sync::{Condvar, Mutex};
use tsr_ipc::Context;
use tsr_jsstring::JsString;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpmError {
    pub message: String,
    pub output: Vec<u8>,
}
impl std::fmt::Display for NpmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for NpmError {}

/// Executes npm without a shell. Implementations must observe cancellation and
/// reap their child before returning. Ordinary tests implement this locally.
pub trait NpmExecutor: Send + Sync {
    fn npm_install(
        &self,
        context: &Context,
        cwd: &[u8],
        args: &[JsString],
    ) -> Result<Vec<u8>, NpmError>;
}

pub struct NpmThrottle {
    active: Mutex<usize>,
    available: Condvar,
    limit: usize,
}
impl NpmThrottle {
    /// A zero-capacity source semaphore blocks forever; reject that invalid
    /// host configuration at construction rather than creating an unusable API.
    pub fn new(limit: usize) -> Self {
        assert!(limit > 0, "npm throttle limit must be positive");
        Self {
            active: Mutex::new(0),
            available: Condvar::new(),
            limit,
        }
    }
    fn acquire(&self, context: &Context) -> Result<Permit<'_>, AtaError> {
        let mut active = self.active.lock().expect("npm throttle lock");
        while *active == self.limit {
            check_context(context)?;
            active = self
                .available
                .wait_timeout(active, std::time::Duration::from_millis(20))
                .expect("npm throttle lock")
                .0;
        }
        check_context(context)?;
        *active += 1;
        Ok(Permit(self))
    }
}
struct Permit<'a>(&'a NpmThrottle);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.0.active.lock().expect("npm throttle lock") -= 1;
        self.0.available.notify_one();
    }
}

/// All non-canceled batches run, including after a sibling failed. The first
/// observed error is retained; every running callback is joined before return.
// port: tsc/internal/project/ata/ata.go:installNpmPackages
pub fn install_npm_packages(
    context: &Context,
    packages: &[JsString],
    throttle: &NpmThrottle,
    install: &(impl Fn(&[JsString]) -> Result<(), AtaError> + Sync),
) -> Result<(), AtaError> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut end = 0;
    let mut size = 100;
    for package in packages {
        size += package.as_bytes().len() + 1;
        if size < 8000 {
            end += 1;
        } else {
            batches.push(&packages[start..end]);
            start = end;
            size = 100 + package.as_bytes().len() + 1;
            end += 1;
        }
    }
    if start < packages.len() {
        batches.push(&packages[start..end]);
    }
    let first_error = Mutex::new(None);
    std::thread::scope(|scope| {
        for batch in batches {
            let error_slot = &first_error;
            scope.spawn(move || {
                let result = throttle.acquire(context).and_then(|_permit| install(batch));
                if let Err(error) = result {
                    error_slot
                        .lock()
                        .expect("npm batch error lock")
                        .get_or_insert(error);
                }
            });
        }
    });
    check_context(context)?;
    first_error
        .into_inner()
        .expect("npm batch error lock")
        .map_or(Ok(()), Err)
}
