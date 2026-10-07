//! A resolved content mapper's identities (`contentmapper.go`). The mapper
//! itself is `tsr_tsoptions`' resolved `ContentMapper`, whose declaration and
//! manifest the config parse fills in.
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tsoptions::config_mappers::ContentMapper;
use tsr_tsoptions::options_json::option_json;

/// The extensions a mapper's output may have.
const SUPPORTED_VIRTUAL_EXTENSIONS: &[&[u8]] = &[
    b".js", b".jsx", b".mjs", b".cjs", b".ts", b".tsx", b".mts", b".cts", b".json",
];

/// port: tsc/internal/contentmapper/contentmapper.go:IsSupportedVirtualExtension
pub fn is_supported_virtual_extension(extension: &[u8]) -> bool {
    SUPPORTED_VIRTUAL_EXTENSIONS.contains(&extension)
}

/// The best available user-facing name, including when manifest resolution
/// failed, including editor-contributed mappers.
/// port: tsc/internal/contentmapper/contentmapper.go:Mapper.DiagnosticName
pub fn diagnostic_name(mapper: &ContentMapper) -> &JsString {
    if !mapper.manifest.name.is_empty() {
        &mapper.manifest.name
    } else if !mapper.package.is_empty() {
        &mapper.package
    } else {
        &mapper.contribution_id
    }
}

/// The mapper's `name@version`, its name when it declares no version, or
/// empty before it resolves to a name.
/// port: tsc/internal/contentmapper/contentmapper.go:Mapper.Identity
pub fn identity(mapper: &ContentMapper) -> JsString {
    let manifest = manifest_identity(mapper);
    if mapper.contribution_id.is_empty() {
        return manifest;
    }
    let mut value = mapper.contribution_id.as_bytes().to_vec();
    value.extend_from_slice(b" (");
    value.extend_from_slice(manifest.as_bytes());
    value.push(b')');
    JsString::from_bytes(value)
}

/// port: tsc/internal/contentmapper/contentmapper.go:Mapper.manifestIdentity
fn manifest_identity(mapper: &ContentMapper) -> JsString {
    let manifest = &mapper.manifest;
    if manifest.name.is_empty() || manifest.version.is_empty() {
        return manifest.name.clone();
    }
    let mut identity = manifest.name.as_bytes().to_vec();
    identity.push(b'@');
    identity.extend_from_slice(manifest.version.as_bytes());
    JsString::from_bytes(identity)
}

/// A fingerprint of everything besides a file's content that determines its
/// transform: the mapper's identity, its options and the compiler options its
/// manifest declares. Big-endian, as the pinned `xxh3.Uint128.Bytes`.
/// port: tsc/internal/contentmapper/contentmapper.go:Mapper.TransformIdentity
pub fn transform_identity(mapper: &ContentMapper, options: Option<&CompilerOptions>) -> [u8; 16] {
    let declared = marshal_declared_options(mapper, options).unwrap_or_default();
    let options_json = declared_json(&declared);
    let identity = identity(mapper);
    let mapper_options = mapper.options.as_deref().unwrap_or_default();
    let mut buffer =
        Vec::with_capacity(identity.len() + 2 + mapper_options.len() + options_json.len());
    buffer.extend_from_slice(identity.as_bytes());
    buffer.push(0);
    buffer.extend_from_slice(mapper_options);
    buffer.push(0);
    buffer.extend_from_slice(&options_json);
    xxhash_rust::xxh3::xxh3_128(&buffer).to_be_bytes()
}

/// Lower-case hexadecimal, as `fmt.Sprintf("%x", bytes)`.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// The compiler options the mapper declares it depends on, in declared order,
/// unset ones left out.
/// port: tsc/internal/contentmapper/contentmapper.go:Mapper.MarshalDeclaredOptions
pub fn marshal_declared_options(
    mapper: &ContentMapper,
    options: Option<&CompilerOptions>,
) -> Result<Vec<(JsString, Vec<u8>)>, tsr_json::Error> {
    let mut out: Vec<(JsString, Vec<u8>)> = Vec::new();
    let (Some(options), Some(names)) = (options, mapper.manifest.compiler_options.as_deref())
    else {
        return Ok(out);
    };
    for name in names {
        let Ok(text) = std::str::from_utf8(name.as_bytes()) else {
            continue;
        };
        let Some(raw) = option_json(options, text)? else {
            continue;
        };
        // An ordered map's Set keeps a repeated key at its first position.
        if let Some(entry) = out.iter_mut().find(|(key, _)| key == name) {
            entry.1 = raw;
        } else {
            out.push((name.clone(), raw));
        }
    }
    Ok(out)
}

/// The JSON object of declared options, in order.
pub fn declared_json(declared: &[(JsString, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"{".to_vec();
    for (index, (name, raw)) in declared.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        out.extend(tsr_json::marshal(name, tsr_json::Options::default()).unwrap_or_default());
        out.push(b':');
        out.extend_from_slice(raw);
    }
    out.push(b'}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsr_tsoptions::config_mappers::MapperManifest;

    fn mapper(name: &str, version: &str, declared: &[&str]) -> ContentMapper {
        ContentMapper {
            package: JsString::from_bytes(b"pkg".as_slice()),
            manifest: MapperManifest {
                name: JsString::from_bytes(name.as_bytes()),
                version: JsString::from_bytes(version.as_bytes()),
                compiler_options: Some(
                    declared
                        .iter()
                        .map(|n| JsString::from_bytes(n.as_bytes()))
                        .collect(),
                ),
                ..MapperManifest::default()
            },
            ..ContentMapper::default()
        }
    }

    #[test]
    fn identity_hashes_are_the_pinned_xxh3_128_bytes() {
        // Digests from github.com/zeebo/xxh3 at the pin (Hash128(...).Bytes()).
        for (input, expected) in [
            (&b""[..], "99aa06d3014798d86001c324468d497f"),
            (b"abc", "06b05ab6733a618578af5f94892f3950"),
            (
                b"mapper@1.0.0\0\0{\"target\":99}",
                "576357e0f709b48d96b12cbf3b158e47",
            ),
        ] {
            assert_eq!(
                hex(&xxhash_rust::xxh3::xxh3_128(input).to_be_bytes()),
                expected
            );
        }
    }

    #[test]
    fn identities_and_declared_options() {
        assert_eq!(identity(&mapper("m", "1.0.0", &[])).as_bytes(), b"m@1.0.0");
        assert_eq!(identity(&mapper("m", "", &[])).as_bytes(), b"m");
        assert_eq!(diagnostic_name(&mapper("", "", &[])).as_bytes(), b"pkg");
        let mut options = CompilerOptions {
            target: tsr_core::ScriptTarget::ESNEXT,
            ..CompilerOptions::default()
        };
        let declared = marshal_declared_options(
            &mapper("m", "", &["target", "jsx", "target"]),
            Some(&options),
        )
        .unwrap();
        assert_eq!(declared_json(&declared), br#"{"target":99}"#);
        assert_eq!(declared_json(&[]), b"{}");
        assert!(is_supported_virtual_extension(b".mts"));
        assert!(!is_supported_virtual_extension(b".coffee"));
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        let first = transform_identity(&mapper("m", "", &["target"]), Some(&options));
        options.target = tsr_core::ScriptTarget::ES2015;
        assert_ne!(
            first,
            transform_identity(&mapper("m", "", &["target"]), Some(&options))
        );
    }
}
