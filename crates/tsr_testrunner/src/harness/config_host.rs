//! Shared config fixture host using the production module resolver.
use std::sync::Arc;
use tsr_jsstring::JsString;
use tsr_tsoptions::ParseConfigHost;
use tsr_vfs::FileSystem;

pub(super) struct Host {
    pub(super) fs: Arc<dyn FileSystem>,
    pub(super) cwd: JsString,
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "This conversion consumes the error supplied by Result::map_err"
)]
fn module_error(error: tsr_module::Error) -> tsr_vfs::Error {
    match error {
        tsr_module::Error::Host(error) => error,
        tsr_module::Error::MutableHost => {
            tsr_vfs::Error::Unsupported("config resolver requires an immutable host")
        }
        tsr_module::Error::Unsupported(reason) => tsr_vfs::Error::Unsupported(reason),
        tsr_module::Error::MalformedPackageJson(_) => {
            tsr_vfs::Error::Unsupported("malformed package JSON")
        }
    }
}
impl ParseConfigHost for Host {
    fn fs(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
    fn current_directory(&self) -> &[u8] {
        self.cwd.as_bytes()
    }
    fn resolve_config(
        &self,
        name: &[u8],
        containing: &[u8],
    ) -> Result<Option<JsString>, tsr_vfs::Error> {
        let result =
            tsr_module::resolve_config(name, containing, self.fs.clone(), self.cwd.as_bytes())
                .map_err(module_error)?;
        Ok((!result.resolved_file_name.is_empty()).then_some(result.resolved_file_name))
    }
    fn resolve_content_mapper(
        &self,
        containing: &[u8],
        package: &[u8],
    ) -> Result<tsr_tsoptions::config_mappers::MapperResolution, tsr_vfs::Error> {
        tsr_module::resolve_content_mapper_manifest(
            &self.fs,
            self.cwd.as_bytes(),
            containing,
            package,
        )
        .map_err(module_error)
    }
}
