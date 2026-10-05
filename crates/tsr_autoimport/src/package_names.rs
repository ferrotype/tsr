//! Dependency admission follows imports in user sources, not transitive imports
//! in packages. Unresolved ambient names admit their declaring package.
use std::collections::BTreeSet;
use tsr_checker::Operation;
use tsr_compiler::{Error, Program};
use tsr_jsstring::JsString;
use tsr_module::Resolver;
use tsr_tspath as path;

#[derive(Default)]
pub(crate) struct PackageNames {
    pub resolved: BTreeSet<JsString>,
    pub deep: BTreeSet<JsString>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Import {
    file: JsString,
    name: JsString,
    resolved: JsString,
    package: JsString,
}

/// The requested-file edit fast path may reuse source exports, but admitting
/// dependency packages must still reflect added and removed import requests.
pub(crate) fn imports(program: &Program) -> BTreeSet<Import> {
    program
        .resolutions()
        .iter()
        .filter(|r| user_import(program, r))
        .map(|r| Import {
            file: r.file.clone(),
            name: r.name.clone(),
            resolved: r.result.resolved_file_name.clone(),
            package: r.result.package_id.name.clone(),
        })
        .collect()
}

fn user_import(program: &Program, resolution: &tsr_compiler::Resolution) -> bool {
    let source = resolution.file.as_bytes();
    !program.is_lib(source)
        && !program.is_external_library(source)
        && !source.windows(14).any(|s| s == b"/node_modules/")
        && !path::is_external_module_name_relative(resolution.name.as_bytes())
}

fn normalized(name: &[u8]) -> JsString {
    JsString::from_bytes(tsr_module::package_name_from_types_package_name(name))
}

// port: tsc/internal/modulespecifiers/util.go:GetPackageNameFromDirectory
fn package_name_from_directory(file: &[u8]) -> &[u8] {
    let Some(at) = file.windows(14).rposition(|s| s == b"/node_modules/") else {
        return b"";
    };
    let name = &file[at + 14..];
    if name.starts_with(b".") {
        return b"";
    }
    let mut slashes = name
        .iter()
        .enumerate()
        .filter_map(|(i, &c)| (c == b'/').then_some(i));
    let Some(first) = slashes.next() else {
        return name;
    };
    if !name.starts_with(b"@") || first + 1 == name.len() {
        return &name[..first];
    }
    &name[..slashes.next().unwrap_or(name.len())]
}

// port: tsc/internal/ls/autoimport/util.go:getResolvedPackageNames
pub(crate) fn collect(
    program: &Program,
    checker: &mut Operation<'_>,
    resolver: &mut Resolver,
) -> Result<PackageNames, Error> {
    let mut result = PackageNames::default();
    let mut unresolved = BTreeSet::new();
    for resolution in program.resolutions() {
        if !user_import(program, resolution) {
            continue;
        }
        let module = &resolution.result;
        if !module.is_resolved() {
            unresolved.insert(resolution.name.clone());
            continue;
        }
        if !module.is_external_library_import {
            continue;
        }
        let scope =
            resolver.package_scope(&path::directory(module.resolved_file_name.as_bytes()))?;
        let name = if !module.package_id.name.is_empty() {
            module.package_id.name.clone()
        } else if let Some(name) = scope
            .as_ref()
            .and_then(|p| p.contents.get("name"))
            .and_then(tsr_module::package_json::Value::as_str)
            .filter(|s| !s.is_empty())
        {
            JsString::from_bytes(name.as_bytes())
        } else {
            JsString::from_bytes(package_name_from_directory(
                module.resolved_file_name.as_bytes(),
            ))
        };
        if name.is_empty() {
            continue;
        }
        let name = normalized(name.as_bytes());
        // A scoped package's first slash is part of its name, not a deep import.
        let request = resolution.name.as_bytes();
        let deep = request
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == b'/')
            .nth(usize::from(request.starts_with(b"@")))
            .is_some_and(|(at, _)| at + 1 < request.len());
        if deep
            && scope
                .as_ref()
                .is_some_and(|p| p.contents.field("exports").is_none())
        {
            result.deep.insert(name.clone());
        }
        result.resolved.insert(name);
    }
    for name in program
        .options()
        .types
        .iter()
        .flatten()
        .filter(|n| n.as_bytes() != b"*")
    {
        result.resolved.insert(normalized(name.as_bytes()));
    }
    for name in unresolved {
        let Some(module) = checker.try_find_ambient_module(name.as_bytes())? else {
            continue;
        };
        let mut declaration = checker.symbol(module)?.value_declaration();
        if declaration.is_none() {
            for candidate in checker.symbol_declarations(module)?.iter().flatten() {
                let Some(file) = program.file_of_node(candidate) else {
                    continue;
                };
                let view = file.bound().view().ast();
                if !tsr_ast::utilities_modules::is_external_module_augmentation(view, candidate)?
                    && !tsr_ast::utilities::is_global_scope_augmentation(&view.node(candidate)?)
                {
                    declaration = Some(candidate);
                    break;
                }
            }
        }
        if let Some(file) = declaration.and_then(|decl| program.file_of_node(decl)) {
            let source = file.bound().view().source_file()?;
            let name = package_name_from_directory(source.file_name());
            if !name.is_empty() {
                result.resolved.insert(normalized(name));
            }
        }
    }
    Ok(result)
}
