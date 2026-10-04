//! Read-only data consumed by the carried Go state-baseline writer. Tokens
//! represent object identity, never a content hash or a recreated Go program.
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use tsr_compiler::{Program, ProgramFile};
use tsr_project::{
    config::{ConfigEntry, ConfigFileRegistry},
    Snapshot,
};

struct Identities<T> {
    entries: BTreeMap<usize, (Weak<T>, usize)>,
}
impl<T> Default for Identities<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}
impl<T> Identities<T> {
    fn id(&mut self, value: &Arc<T>) -> usize {
        let next = self.entries.len() + 1;
        // The weak control block prevents address reuse during this test.
        self.entries
            .entry(Arc::as_ptr(value) as usize)
            .or_insert_with(|| (Arc::downgrade(value), next))
            .1
    }
}
#[derive(Default)]
pub(super) struct Projection {
    programs: Identities<Program>,
    files: Identities<ProgramFile>,
    registries: Identities<ConfigFileRegistry>,
    configs: Identities<ConfigEntry>,
}
fn text(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes)
        .map_err(|_| "state contains a non-Unicode path (ADR 0019 transport limitation)".into())
}
fn strings<'a>(
    values: impl Iterator<Item = &'a tsr_jsstring::JsString>,
) -> Result<Vec<&'a str>, String> {
    values.map(|v| text(v.as_bytes())).collect()
}
impl Projection {
    pub(super) fn read(&mut self, snapshot: &Snapshot) -> Result<Value, String> {
        let mut projects = Vec::new();
        for project in snapshot.projects() {
            let data = project.data().ok_or("project has no program data")?;
            let mut files = Vec::new();
            for file in data.program.files() {
                let view = file.bound().view();
                let source = view.source_file().map_err(|e| e.to_string())?;
                files.push(json!({"id":self.files.id(file), "fileName":text(source.file_name())?, "path":text(source.parse_options().path.as_bytes())?}));
            }
            projects.push(json!({"name":text(data.name.as_bytes())?, "program":self.programs.id(&data.program), "files":files}));
        }
        let fs = snapshot.filesystem().ok_or("missing session filesystem")?;
        let mut open = Vec::new();
        for (path, file) in fs.overlays().iter() {
            let default = snapshot
                .project_for_file(path.as_bytes())
                .and_then(|project| project.data())
                .map(|data| text(data.name.as_bytes()))
                .transpose()?;
            let mut all = Vec::new();
            for project in snapshot.projects() {
                if project.contains_file(path.as_bytes()) {
                    all.push(text(project.data().unwrap().name.as_bytes())?);
                }
            }
            all.sort_unstable();
            open.push(json!({"fileName":text(file.file_name().as_bytes())?, "defaultProject":default.unwrap_or(""), "projects":all}));
        }
        let registry = snapshot.configs().ok_or("missing session configs")?;
        let mut configs = Vec::new();
        for (path, entry) in registry.configs.iter() {
            configs.push(json!({"id":self.configs.id(entry), "path":text(path.as_bytes())?, "fileName":text(entry.file_name.as_bytes())?,
                "retainingProjects":strings(entry.retaining_projects.iter())?, "retainingOpenFiles":strings(entry.retaining_open_files.iter())?, "retainingConfigs":strings(entry.retaining_configs.iter())?}));
        }
        let mut names = Vec::new();
        for (path, entry) in registry.file_names.iter() {
            let ancestors = entry
                .ancestors
                .iter()
                .map(|(a, b)| Ok((text(a.as_bytes())?, text(b.as_bytes())?)))
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            names.push(json!({"path":text(path.as_bytes())?, "nearest":text(entry.nearest.as_bytes())?, "ancestors":ancestors}));
        }
        Ok(
            json!({"version":1,"projects":projects,"openFiles":open,"registry":self.registries.id(registry),"configs":configs,"configFileNames":names}),
        )
    }
}
