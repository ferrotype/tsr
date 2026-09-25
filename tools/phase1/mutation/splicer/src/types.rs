//! The workspace's own type definitions, as far as operators need them: unit
//! enum variants, integer newtypes and type aliases, found by name.
//!
//! A return type is written as a path; its last segment names the definition.
//! Only files that textually define a wanted name are parsed, and a name
//! defined more than once with different shapes resolves to nothing, so an
//! operator is never chosen from a guess.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use syn::visit::{self, Visit};
use syn::{Fields, Visibility};

use crate::index::norm;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumDef {
    /// Unit variants in declaration order.
    pub unit_variants: Vec<String>,
    /// Every variant is a unit variant and the enum is not `#[non_exhaustive]`:
    /// a match over the unit variants is exhaustive.
    pub exhaustive_units: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewtypeDef {
    /// The normalized type of the single tuple field.
    pub field: String,
    /// The field is readable from any crate (`pub`), or only from `krate`.
    pub public: bool,
    pub krate: String,
}

#[derive(Default, Debug)]
pub struct TypeIndex {
    enums: BTreeMap<String, Vec<EnumDef>>,
    newtypes: BTreeMap<String, Vec<NewtypeDef>>,
    aliases: BTreeMap<String, Vec<String>>,
    /// Names defined as anything else (a struct with named fields, a generic
    /// type, a trait): a same-named enum elsewhere is then ambiguous.
    others: BTreeSet<String>,
}

/// The integer primitives the operators mutate.
pub const INTEGER_PRIMITIVES: &[&str] = &[
    "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize",
];

/// The last path segment of a type, without generic arguments.
pub fn type_head(ty: &str) -> &str {
    let path = ty.split('<').next().unwrap_or(ty);
    path.rsplit("::").next().unwrap_or(path)
}

/// Capitalized identifiers in a normalized type: the names a definition
/// lookup may need.
pub fn type_names(ty: &str) -> BTreeSet<String> {
    ty.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| word.chars().next().is_some_and(char::is_uppercase))
        .map(str::to_owned)
        .collect()
}

struct Collector<'a> {
    wanted: &'a BTreeSet<String>,
    krate: &'a str,
    index: &'a mut TypeIndex,
}

fn non_exhaustive(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|attr| attr.path().is_ident("non_exhaustive"))
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        let name = item.ident.to_string();
        if self.wanted.contains(&name) {
            if item.generics.params.is_empty() {
                let unit_variants: Vec<String> = item
                    .variants
                    .iter()
                    .filter(|variant| matches!(variant.fields, Fields::Unit))
                    .map(|variant| variant.ident.to_string())
                    .collect();
                let exhaustive_units =
                    unit_variants.len() == item.variants.len() && !non_exhaustive(&item.attrs);
                self.index.enums.entry(name).or_default().push(EnumDef {
                    unit_variants,
                    exhaustive_units,
                });
            } else {
                self.index.others.insert(name);
            }
        }
        visit::visit_item_enum(self, item);
    }

    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        let name = item.ident.to_string();
        if self.wanted.contains(&name) {
            match &item.fields {
                Fields::Unnamed(fields)
                    if fields.unnamed.len() == 1 && item.generics.params.is_empty() =>
                {
                    let field = &fields.unnamed[0];
                    let public = matches!(field.vis, Visibility::Public(_));
                    let crate_visible = public
                        || matches!(&field.vis, Visibility::Restricted(restricted)
                            if restricted.path.is_ident("crate"));
                    if crate_visible {
                        self.index
                            .newtypes
                            .entry(name)
                            .or_default()
                            .push(NewtypeDef {
                                field: norm(&field.ty),
                                public,
                                krate: self.krate.to_owned(),
                            });
                    } else {
                        self.index.others.insert(name);
                    }
                }
                _ => {
                    self.index.others.insert(name);
                }
            }
        }
        visit::visit_item_struct(self, item);
    }

    fn visit_item_type(&mut self, item: &'ast syn::ItemType) {
        let name = item.ident.to_string();
        if self.wanted.contains(&name) {
            if item.generics.params.is_empty() {
                self.index
                    .aliases
                    .entry(name)
                    .or_default()
                    .push(norm(&item.ty));
            } else {
                self.index.others.insert(name);
            }
        }
        visit::visit_item_type(self, item);
    }

    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        let name = item.ident.to_string();
        if self.wanted.contains(&name) {
            self.index.others.insert(name);
        }
        visit::visit_item_trait(self, item);
    }
}

/// Whether `text` may define one of `wanted`: some `enum`, `struct`, `type` or
/// `trait` keyword is followed by a wanted name. One pass per keyword.
fn defines_any(text: &str, wanted: &BTreeSet<String>) -> bool {
    ["enum ", "struct ", "type ", "trait "]
        .iter()
        .any(|keyword| {
            text.match_indices(keyword).any(|(at, _)| {
                let rest = text[at + keyword.len()..].trim_start();
                let end = rest
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                wanted.contains(&rest[..end])
            })
        })
}

impl TypeIndex {
    /// Indexes the definitions of `wanted` (and, transitively, of the alias
    /// targets they name) under `root/crates/*/src`.
    pub fn build(root: &Path, wanted: &BTreeSet<String>) -> Result<Self, String> {
        let mut index = Self::default();
        let mut files = Vec::new();
        let crates = root.join("crates");
        if crates.is_dir() {
            let mut dirs: Vec<_> = std::fs::read_dir(&crates)
                .map_err(|error| format!("{}: {error}", crates.display()))?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect();
            dirs.sort();
            for dir in dirs {
                let src = dir.join("src");
                if src.is_dir() {
                    let krate = dir
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let mut found = Vec::new();
                    crate::plan::rust_files(&src, &mut found)?;
                    files.extend(found.into_iter().map(|path| (krate.clone(), path)));
                }
            }
        }
        let mut texts = Vec::with_capacity(files.len());
        for (krate, path) in files {
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            texts.push((krate, path, text));
        }
        let mut pending: BTreeSet<String> = wanted.clone();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        // Alias targets may name further definitions; a few rounds suffice.
        for _ in 0..4 {
            let round: BTreeSet<String> = pending.difference(&seen).cloned().collect();
            if round.is_empty() {
                break;
            }
            seen.extend(round.iter().cloned());
            for (krate, path, text) in &texts {
                if !defines_any(text, &round) {
                    continue;
                }
                let file = syn::parse_file(text)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                Collector {
                    wanted: &round,
                    krate,
                    index: &mut index,
                }
                .visit_file(&file);
            }
            pending = index
                .aliases
                .values()
                .flatten()
                .flat_map(|target| type_names(target))
                .collect();
        }
        Ok(index)
    }

    fn unique<'s, T: PartialEq>(
        &self,
        map: &'s BTreeMap<String, Vec<T>>,
        name: &str,
    ) -> Option<&'s T> {
        if self.others.contains(name) || self.defined_kinds(name) > 1 {
            return None;
        }
        let defs = map.get(name)?;
        defs.iter().all(|def| *def == defs[0]).then(|| &defs[0])
    }

    fn defined_kinds(&self, name: &str) -> usize {
        usize::from(self.enums.contains_key(name))
            + usize::from(self.newtypes.contains_key(name))
            + usize::from(self.aliases.contains_key(name))
    }

    /// The type an alias names, followed through further aliases; the type
    /// itself when it is no alias. Generic types are never resolved.
    pub fn resolve(&self, ty: &str) -> String {
        let mut current = ty.to_owned();
        for _ in 0..4 {
            if current.contains('<') {
                break;
            }
            match self.unique(&self.aliases, type_head(&current)) {
                Some(target) => current.clone_from(target),
                None => break,
            }
        }
        current
    }

    /// The enum `ty` names, when it has at least two unit variants.
    pub fn enum_of(&self, ty: &str) -> Option<&EnumDef> {
        if ty.contains('<') {
            return None;
        }
        self.unique(&self.enums, type_head(&self.resolve(ty)))
            .filter(|def| def.unit_variants.len() >= 2)
    }

    /// Whether `ty` is a one-field tuple struct around an integer whose field a
    /// site in `krate` can read and write.
    pub fn integer_newtype(&self, ty: &str, krate: &str) -> bool {
        if ty.contains('<') {
            return false;
        }
        self.unique(&self.newtypes, type_head(&self.resolve(ty)))
            .is_some_and(|def| {
                INTEGER_PRIMITIVES.contains(&self.resolve(&def.field).as_str())
                    && (def.public || def.krate == krate)
            })
    }

    /// Whether `ty` is an integer primitive, directly or through aliases.
    pub fn is_integer(&self, ty: &str) -> bool {
        INTEGER_PRIMITIVES.contains(&self.resolve(ty).as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{type_names, TypeIndex};
    use std::collections::BTreeSet;

    #[test]
    fn definitions_resolve_only_when_unambiguous() {
        let root = std::env::temp_dir().join(format!("phase1-types-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in [
            (
                "crates/a/src/lib.rs",
                "pub enum State { A, B, C(u8) }\n#[non_exhaustive] pub enum Open { X, Y }\n\
                 pub enum Plain { P, Q, R }\npub struct Kind(pub i32);\npub struct Hidden(u16);\n\
                 pub struct Crate(pub(crate) u8);\npub type Mode = Kind;\npub type Flags = u32;\n\
                 pub enum Twice { A, B }\npub enum One { Only }\n",
            ),
            ("crates/b/src/lib.rs", "pub enum Twice { A, C }\n"),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let wanted: BTreeSet<String> = [
            "State", "Open", "Plain", "Kind", "Hidden", "Crate", "Mode", "Flags", "Twice", "One",
            "Missing",
        ]
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
        let index = TypeIndex::build(&root, &wanted).unwrap();
        let state = index.enum_of("State").unwrap();
        assert_eq!(state.unit_variants, ["A", "B"]);
        assert!(!state.exhaustive_units, "a data variant");
        assert!(!index.enum_of("Open").unwrap().exhaustive_units);
        assert!(index.enum_of("crate::Plain").unwrap().exhaustive_units);
        assert!(
            index.enum_of("Twice").is_none(),
            "two different definitions"
        );
        assert!(index.enum_of("One").is_none(), "a single unit variant");
        assert!(index.enum_of("Missing").is_none());
        assert!(index.integer_newtype("Kind", "b"));
        assert!(index.integer_newtype("Mode", "b"), "through the alias");
        assert!(!index.integer_newtype("Hidden", "a"), "private field");
        assert!(index.integer_newtype("Crate", "a"));
        assert!(!index.integer_newtype("Crate", "b"));
        assert!(index.is_integer("Flags"));
        assert!(!index.is_integer("Kind"));
        assert_eq!(index.resolve("Mode"), "Kind");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            type_names("Result<(&'a[NodeId],Option<a::Mode>),Error>")
                .into_iter()
                .collect::<Vec<_>>(),
            ["Error", "Mode", "NodeId", "Option", "Result"]
        );
    }
}
