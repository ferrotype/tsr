use crate::{
    filesystem::Host,
    wire::{self, wire, Json},
};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::collections::BTreeMap;
const MAX_PLUGINS: usize = 32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Initialize {
    version: u32,
    case_sensitive: bool,
    base: BTreeMap<String, String>,
    symlinks: BTreeMap<String, String>,
    callbacks: Vec<String>,
    pub(crate) options: Json,
    pub(crate) plugins: Vec<Registration>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Registration {
    pub(crate) name: String,
    pub(crate) options: Json,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginState {
    Registered,
    Opening,
    Open,
    Retired,
}
impl PluginState {
    fn text(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Opening => "opening",
            Self::Open => "open",
            Self::Retired => "retired",
        }
    }
}
pub(crate) struct Plugin {
    pub(crate) registration: Registration,
    pub(crate) state: PluginState,
}
pub(crate) struct Configuration {
    pub(crate) host: Host,
    pub(crate) options: Json,
    pub(crate) plugins: Vec<Plugin>,
}
impl Configuration {
    pub(crate) fn from_wire(wire: Initialize) -> Result<Self, String> {
        if wire.version != 2 {
            return Err("unsupported test-host version".into());
        }
        if !wire::object(&wire.options) {
            return Err("options must be an object".into());
        }
        if wire.plugins.len() > MAX_PLUGINS {
            return Err("too many registered plugins".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for plugin in &wire.plugins {
            if plugin.name.is_empty()
                || !names.insert(&plugin.name)
                || !wire::object(&plugin.options)
            {
                return Err("plugins require unique nonempty names and object options".into());
            }
        }
        let host = Host::new(
            wire.case_sensitive,
            &wire.base,
            wire.symlinks,
            &wire.callbacks,
        )?;
        let plugins = wire
            .plugins
            .into_iter()
            .map(|registration| Plugin {
                registration,
                state: PluginState::Registered,
            })
            .collect();
        Ok(Self {
            host,
            options: wire.options,
            plugins,
        })
    }
    pub(crate) fn wire(&self) -> Json {
        self.wire_with_options(&self.options, false)
    }

    pub(crate) fn wire_with_options(&self, options: &RawValue, reserve_states: bool) -> Json {
        let plugins: Vec<_> = self
            .plugins
            .iter()
            .map(|plugin| {
                let state = if reserve_states {
                    PluginState::Registered
                } else {
                    plugin.state
                };
                wire!({"name": plugin.registration.name, "options": plugin.registration.options,
                   "state": state.text()})
            })
            .collect();
        wire!({"version": 2, "caseSensitive": self.host.case_sensitive,
               "options": options, "plugins": plugins})
    }
}
