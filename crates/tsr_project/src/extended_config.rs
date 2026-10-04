//! Extended config entries are shared between snapshots, with content and
//! transitive-extends invalidation. The snapshot owns each key only once.
use crate::owner_cache::OwnerCache;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use tsr_jsstring::JsString;
use tsr_tsoptions::{ExtendedConfigCacheEntry, ExtendedConfigProvider, ParseConfigHost};
use tsr_vfs::Error;

struct Entry {
    config: Arc<ExtendedConfigCacheEntry>,
    hash: u128,
}
#[derive(Default)]
pub struct ExtendedConfigCache {
    entries: OwnerCache<JsString, Arc<Entry>>,
}
impl ExtendedConfigCache {
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub struct ConfigOwnership {
    cache: Arc<ExtendedConfigCache>,
    owner: u64,
    keys: Mutex<BTreeSet<JsString>>,
}
impl ConfigOwnership {
    pub fn new(cache: Arc<ExtendedConfigCache>, owner: u64) -> Self {
        Self {
            cache,
            owner,
            keys: Mutex::default(),
        }
    }
    /// Called while the preceding snapshot is still retained. Adding the same
    /// key several times must not increase the cache's ownership count.
    pub fn inherit(&self, previous: &Self) {
        assert!(Arc::ptr_eq(&self.cache, &previous.cache));
        let keys = previous.keys.lock().expect("config owners").clone();
        for key in &keys {
            self.cache.entries.add_owner(key, self.owner);
        }
        self.keys.lock().expect("config owners").extend(keys);
    }
    pub fn retain(&self, keep: impl Fn(&JsString) -> bool) {
        self.keys.lock().expect("config owners").retain(|key| {
            if keep(key) {
                return true;
            }
            self.cache.entries.release(key, self.owner);
            false
        });
    }
}
impl Drop for ConfigOwnership {
    fn drop(&mut self) {
        for key in self.keys.get_mut().expect("config owners").iter() {
            self.cache.entries.release(key, self.owner);
        }
    }
}
impl ExtendedConfigProvider for ConfigOwnership {
    // port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.GetExtendedConfig
    fn get_extended_config(
        &self,
        name: &[u8],
        path: JsString,
        stack: &[JsString],
        host: &dyn ParseConfigHost,
    ) -> Result<Arc<ExtendedConfigCacheEntry>, Error> {
        let content = host.fs().read_file(name)?;
        let text = content
            .as_ref()
            .map_or(&[][..], |file| file.text.as_bytes());
        let entry = self.cache.entries.load_and_acquire(
            &path,
            self.owner,
            |entry| Ok(entry.hash == 0 || entry.hash != hash(&entry.config, text, host)?),
            || {
                let config = Arc::new(tsr_tsoptions::parse_extended_with_cache(
                    name,
                    path.clone(),
                    stack,
                    host,
                    Some(self),
                )?);
                let hash = hash(&config, text, host)?;
                Ok::<_, Error>(Arc::new(Entry { config, hash }))
            },
        )?;
        self.keys.lock().expect("config owners").insert(path);
        // A hit bypasses recursive parsing. Keep the cached transitive entries
        // alive for this snapshot too, before the preceding snapshot is dropped.
        for name in entry.config.extended_file_names() {
            let key = tsr_tspath::to_path(
                name.as_bytes(),
                host.current_directory(),
                host.fs().use_case_sensitive_file_names(),
            );
            if !self.keys.lock().expect("config owners").contains(&key)
                && self.cache.entries.try_add_owner(&key, self.owner)
            {
                self.keys.lock().expect("config owners").insert(key);
            }
        }
        Ok(entry.config.clone())
    }
}

// port: tsc/internal/project/extendedconfigcache.go:hash
fn hash(
    config: &ExtendedConfigCacheEntry,
    text: &[u8],
    host: &dyn ParseConfigHost,
) -> Result<u128, Error> {
    let mut hash = xxhash_rust::xxh3::Xxh3::new();
    hash.update(text);
    for name in config.extended_file_names() {
        let Some(content) = host.fs().read_file(name.as_bytes())? else {
            return Ok(0);
        };
        hash.update(content.text.as_bytes());
    }
    Ok(hash.digest128())
}
