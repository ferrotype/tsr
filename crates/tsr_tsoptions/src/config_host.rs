//! Config parsing receives an explicit filesystem and a module-resolution
//! callback. The callback avoids a tsoptions/module dependency cycle.
use tsr_jsstring::JsString;
use tsr_vfs::{Error, FileSystem};
pub trait ParseConfigHost: Sync {
    fn fs(&self) -> &dyn FileSystem;
    fn current_directory(&self) -> &[u8];
    fn resolve_config(
        &self,
        name: &[u8],
        containing_file: &[u8],
    ) -> Result<Option<JsString>, Error>;
    fn resolve_content_mapper(
        &self,
        containing_file: &[u8],
        package: &[u8],
    ) -> Result<crate::config_mappers::MapperResolution, Error>;
}
