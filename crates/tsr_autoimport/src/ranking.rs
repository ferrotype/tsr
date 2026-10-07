//! Ranking retains module resolution provenance until protocol serialization.
use crate::{fix::Fix, Preferences};
use std::cmp::Ordering;
use tsr_checker::{Error, ModuleSpecifierKind as Kind};
use tsr_compiler::Program;
use tsr_core::Tristate;

pub struct Ranking<'a> {
    importing_file: &'a [u8],
    prefer_non_relative: bool,
    uri_style: Tristate,
}
impl<'a> Ranking<'a> {
    pub fn new(
        program: &'a Program,
        source: tsr_ast::NodeId,
        preferences: &Preferences,
    ) -> Result<Self, Error> {
        let file = program
            .file_of_node(source)
            .ok_or(Error::MissingLink("ranking source"))?;
        let view = file.bound().view().ast();
        let read = view.source_file(source)?;
        // The pinned Program's fallback tristate is never assigned after its
        // zero initialization. Inspect this file's imports before that fallback.
        let mut uri_style = Tristate::UNKNOWN;
        for &import in read.imports()?.iter().flatten() {
            let text = view.node_text(import)?;
            let name = text.as_bytes();
            if tsr_core::node_modules::node_core_module(name)
                && !tsr_core::node_modules::exclusively_prefixed_node_core_module(name)
            {
                uri_style = if name.starts_with(b"node:") {
                    Tristate::TRUE
                } else {
                    Tristate::FALSE
                };
                break;
            }
        }
        Ok(Self {
            importing_file: read.file_name(),
            prefer_non_relative: matches!(
                preferences.module_specifier.as_deref(),
                Some("non-relative" | "project-relative")
            ),
            uri_style,
        })
    }
    pub fn rank(&self, a: &Fix, b: &Fix) -> Ordering {
        a.kind
            .0
            .cmp(&b.kind.0)
            .then_with(|| self.rank_specifiers(a, b))
    }
    fn rank_specifiers(&self, a: &Fix, b: &Fix) -> Ordering {
        if self.prefer_non_relative {
            let order = (a.module_specifier_kind == Kind::Relative)
                .cmp(&(b.module_specifier_kind == Kind::Relative));
            if order != Ordering::Equal {
                return order;
            }
        }
        if a.module_specifier_kind == Kind::Ambient && b.module_specifier_kind == Kind::Ambient {
            let order = a
                .module_specifier
                .starts_with("node:")
                .cmp(&b.module_specifier.starts_with("node:"));
            if self.uri_style == Tristate::TRUE && order != Ordering::Equal {
                return order.reverse();
            }
            if self.uri_style == Tristate::FALSE && order != Ordering::Equal {
                return order;
            }
        }
        if a.module_specifier_kind == Kind::Relative && b.module_specifier_kind == Kind::Relative {
            let order = self
                .possibly_reexports_importing_file(a)
                .cmp(&self.possibly_reexports_importing_file(b));
            if order != Ordering::Equal {
                return order;
            }
        }
        a.module_specifier
            .bytes()
            .filter(|&byte| byte == b'/')
            .count()
            .cmp(
                &b.module_specifier
                    .bytes()
                    .filter(|&byte| byte == b'/')
                    .count(),
            )
    }
    fn possibly_reexports_importing_file(&self, fix: &Fix) -> bool {
        let file = fix.module_file_name.as_bytes();
        let Some(slash) = file.iter().rposition(|&byte| byte == b'/') else {
            return false;
        };
        fix.is_re_export
            && matches!(
                &file[slash + 1..],
                b"index.js" | b"index.jsx" | b"index.d.ts" | b"index.ts" | b"index.tsx"
            )
            && self.importing_file.starts_with(&file[..=slash])
    }
    pub fn compare(&self, a: &Fix, b: &Fix) -> Ordering {
        self.rank(a, b)
            .then_with(|| {
                b.module_specifier
                    .starts_with("./")
                    .cmp(&a.module_specifier.starts_with("./"))
            })
            .then_with(|| a.module_specifier.cmp(&b.module_specifier))
            .then_with(|| a.import_kind.0.cmp(&b.import_kind.0))
    }
}

#[cfg(test)]
#[path = "ranking_tests.rs"]
mod tests;
