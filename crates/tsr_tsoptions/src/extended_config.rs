//! Parsed extended configs scoped to one host. Callers invalidate the cache
//! when changing that host; a cache can never be reused with another host.
use crate::{
    ConfigValue, ExtendedConfigCacheEntry, ParseConfigHost, ParsedCommandLine, ReadConfigResult,
    TsConfigSourceFile,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_vfs::Error;

pub struct ExtendedConfigCache<'host> {
    host: &'host dyn ParseConfigHost,
    entries: Mutex<HashMap<JsString, Arc<ExtendedConfigCacheEntry>>>,
}
impl<'host> ExtendedConfigCache<'host> {
    pub fn new(host: &'host dyn ParseConfigHost) -> Self {
        Self {
            host,
            entries: Mutex::new(HashMap::new()),
        }
    }
    pub fn clear(&mut self) {
        self.entries.get_mut().expect("config cache lock").clear();
    }
    /// Recursive parsing runs outside the map lock. Racing initializers publish
    /// one winner; no initializer can expose a partly populated entry.
    pub fn get_extended_config(
        &self,
        name: &[u8],
        path: JsString,
        stack: &[JsString],
    ) -> Result<Arc<ExtendedConfigCacheEntry>, Error> {
        if let Some(entry) = self
            .entries
            .lock()
            .expect("config cache lock")
            .get(&path)
            .cloned()
        {
            return Ok(entry);
        }
        let entry = Arc::new(crate::config_parse::parse_extended_with_cache(
            name,
            path.clone(),
            stack,
            self.host,
            Some(self),
        )?);
        Ok(self
            .entries
            .lock()
            .expect("config cache lock")
            .entry(path)
            .or_insert(entry)
            .clone())
    }
    pub fn parse_source_file(
        &self,
        source: TsConfigSourceFile,
        base: &[u8],
        existing: &CompilerOptions,
        existing_raw: &ConfigValue,
        name: &[u8],
    ) -> Result<ParsedCommandLine, Error> {
        crate::config_parse::parse_source_with_cache(
            source,
            self.host,
            base,
            existing,
            existing_raw,
            name,
            Some(self),
        )
    }
    pub fn parse_json(
        &self,
        raw: ConfigValue,
        base: &[u8],
        existing: &CompilerOptions,
        name: &[u8],
        stack: &[JsString],
    ) -> Result<ParsedCommandLine, Error> {
        crate::config_parse::parse_raw_with_cache(
            raw,
            self.host,
            base,
            existing,
            name,
            stack,
            Some(self),
        )
    }
    pub fn read_config_file(
        &self,
        name: &[u8],
        options: &CompilerOptions,
        raw: &ConfigValue,
    ) -> Result<ReadConfigResult, Error> {
        let name = tsr_tspath::absolute(name, self.host.current_directory());
        let path = tsr_tspath::to_path(
            &name,
            self.host.current_directory(),
            self.host.fs().use_case_sensitive_file_names(),
        );
        crate::config_read::read_with_cache(&name, path, options, raw, self.host, Some(self))
    }
}
