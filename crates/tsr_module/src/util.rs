//! Shared package identities and path interpretation used by resolution clients.
use tsr_jsstring::JsString;
use tsr_tspath as path;

/// port: tsc/internal/module/util.go:UnmangleScopedPackageName
pub fn unmangle_scoped_package_name(name: &[u8]) -> Vec<u8> {
    name.windows(2)
        .position(|bytes| bytes == b"__")
        .map_or_else(
            || name.to_vec(),
            |split| [b"@".as_slice(), &name[..split], b"/", &name[split + 2..]].concat(),
        )
}
/// port: tsc/internal/module/util.go:GetPackageNameFromTypesPackageName
pub fn package_name_from_types_package_name(name: &[u8]) -> Vec<u8> {
    name.strip_prefix(b"@types/")
        .map_or_else(|| name.to_vec(), unmangle_scoped_package_name)
}

/// port: tsc/internal/module/util.go:ParseNodeModuleFromPath
pub fn parse_node_module_from_path(resolved: &[u8], is_folder: bool) -> Vec<u8> {
    let file = path::normalize(resolved);
    let Some(start) = file.windows(14).rposition(|part| part == b"/node_modules/") else {
        return Vec::new();
    };
    let start = start + 14;
    let mut end = next_separator(&file, start, is_folder);
    if file[start] == b'@' {
        end = next_separator(&file, end, is_folder);
    }
    file[..end].to_vec()
}
/// port: tsc/internal/module/resolver.go:moveToNextDirectorySeparatorIfAvailable
fn next_separator(file: &[u8], previous: usize, is_folder: bool) -> usize {
    file.get(previous + 1..)
        .and_then(|tail| tail.iter().position(|&byte| byte == b'/'))
        .map_or(if is_folder { file.len() } else { previous }, |next| {
            previous + 1 + next
        })
}

impl crate::PackageId {
    /// port: tsc/internal/module/types.go:PackageId.PackageName
    pub fn package_name(&self) -> JsString {
        if self.sub_module_name.is_empty() {
            return self.name.clone();
        }
        JsString::from_bytes([self.name.as_bytes(), b"/", self.sub_module_name.as_bytes()].concat())
    }
    /// Byte-preserving counterpart of Go's String method.
    /// port: tsc/internal/module/types.go:PackageId.String
    pub fn text(&self) -> JsString {
        JsString::from_bytes(
            [
                self.package_name().as_bytes(),
                b"@",
                self.version.as_bytes(),
                self.peer_dependencies.as_bytes(),
            ]
            .concat(),
        )
    }
}

/// port: tsc/internal/module/util.go:TryGetJSExtensionForFile
pub fn js_extension_for_file(file: &[u8], options: &tsr_core::CompilerOptions) -> &'static [u8] {
    match path::try_get_extension_from_path(file) {
        b".tsx" if options.jsx == tsr_core::JsxEmit::PRESERVE => b".jsx",
        b".ts" | b".d.ts" | b".tsx" => b".js",
        ext @ (b".js" | b".jsx" | b".json") => ext,
        b".d.mts" | b".mts" | b".mjs" => b".mjs",
        b".d.cts" | b".cts" | b".cjs" => b".cjs",
        _ => b"",
    }
}
