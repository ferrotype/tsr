use crate::DocumentUri;
use tsr_jsstring::JsString;

impl DocumentUri {
    /// Files are decoded to bytes, not a Rust String: percent escapes can
    /// represent arbitrary file-name bytes. Other schemes remain escaped.
    // port: tsc/internal/lsp/lsproto/lsp.go:DocumentUri.FileName
    pub fn file_name(&self) -> JsString {
        let uri = self.0.as_str();
        if uri.starts_with("bundled:///") {
            return JsString::from_bytes(uri.as_bytes());
        }
        if let Some(rest) = uri.strip_prefix("file://") {
            return file_uri(rest).unwrap_or_else(|| panic!("invalid file URI: {uri}"));
        }
        let (scheme, mut path) = uri
            .split_once(':')
            .unwrap_or_else(|| panic!("invalid URI: {uri}"));
        let mut authority = "ts-nul-authority";
        if let Some(rest) = path.strip_prefix("//") {
            (authority, path) = rest
                .split_once('/')
                .unwrap_or_else(|| panic!("invalid URI: {uri}"));
        }
        JsString::from_bytes(format!("^/{scheme}/{authority}/{path}").as_bytes())
    }
    // port: tsc/internal/lsp/lsproto/lsp.go:DocumentUri.Path
    pub fn path(&self, case_sensitive: bool) -> JsString {
        tsr_tspath::to_path(self.file_name().as_bytes(), b"", case_sensitive)
    }
    // port: tsc/internal/ls/lsconv/converters.go:FileNameToDocumentURI
    pub fn from_file_name(file_name: &[u8]) -> Self {
        if file_name.starts_with(b"bundled:///") {
            return Self(
                String::from_utf8(file_name.to_vec()).expect("bundled URI must be Unicode"),
            );
        }
        if let Some(rest) = file_name.strip_prefix(b"^/") {
            let file = std::str::from_utf8(rest).expect("non-file URI must be Unicode");
            let (scheme, rest) = file.split_once('/').expect("invalid file name");
            let (authority, path) = rest.split_once('/').expect("invalid file name");
            return Self(if authority == "ts-nul-authority" {
                format!("{scheme}:{path}")
            } else {
                format!("{scheme}://{authority}/{path}")
            });
        }
        let (volume, rest, _) = tsr_tspath::split_volume_path(file_name);
        let mut uri = String::from("file://");
        if !volume.is_empty() {
            uri.push('/');
            escape(&volume, &mut uri);
        }
        let rest = rest.strip_prefix(b"//").unwrap_or(rest);
        for (index, part) in rest.split(|b| *b == b'/').enumerate() {
            if index != 0 {
                uri.push('/');
            }
            escape(part, &mut uri);
        }
        Self(uri)
    }
}
fn escape(bytes: &[u8], output: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
}
fn unescape(bytes: &[u8], host: bool, zone: bool) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'%' {
            let high = char::from(*bytes.get(i + 1)?).to_digit(16)?;
            let low = char::from(*bytes.get(i + 2)?).to_digit(16)?;
            let byte = (high * 16 + low) as u8;
            if host && byte < 128 && byte != b'%' {
                return None;
            }
            if zone && byte != b'%' && byte != b' ' && !valid_host_byte(byte) {
                return None;
            }
            output.push(byte);
            i += 3;
        } else {
            if (host || zone) && byte < 128 && !valid_host_byte(byte) {
                return None;
            }
            output.push(byte);
            i += 1;
        }
    }
    Some(output)
}
fn valid_host_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-_.~!$&'()*+,;=:[]<>\"".contains(&byte)
}
fn file_uri(rest: &str) -> Option<JsString> {
    let rest = if let Some((rest, fragment)) = rest.split_once('#') {
        unescape(fragment.as_bytes(), false, false)?;
        rest
    } else {
        rest
    };
    if rest.bytes().any(|b| b < 32 || b == 127) {
        return None;
    }
    let rest = rest.split_once('?').map_or(rest, |(rest, _)| rest);
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, ""), |(host, _)| (host, &rest[host.len()..]));
    let host = if let Some((user, host)) = authority.rsplit_once('@') {
        if !user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:%@".contains(&b))
        {
            return None;
        }
        unescape(user.as_bytes(), false, false)?;
        host
    } else {
        authority
    };
    let host = decode_host(host)?;
    let path = unescape(path.as_bytes(), false, false)?;
    if host.is_empty() {
        Some(fix_windows_uri_path(&path))
    } else {
        let mut result = b"//".to_vec();
        result.extend(host);
        result.extend(path);
        Some(JsString::from_bytes(result))
    }
}
fn decode_host(host: &str) -> Option<Vec<u8>> {
    if host.rfind('[').is_some_and(|index| index != 0) {
        return None;
    }
    let port = |p: &str| {
        p.is_empty()
            || p.strip_prefix(':')
                .is_some_and(|v| v.bytes().all(|b| b.is_ascii_digit()))
    };
    if let Some(rest) = host.strip_prefix('[') {
        let end = rest.rfind(']')?;
        let (address, suffix) = rest.split_at(end);
        if !port(&suffix[1..]) {
            return None;
        }
        let (address, zone) = address
            .split_once("%25")
            .map_or((address, None), |(a, z)| (a, Some(z)));
        address.parse::<std::net::Ipv6Addr>().ok()?;
        let mut result = format!("[{address}").into_bytes();
        if let Some(zone) = zone {
            if zone.is_empty() {
                return None;
            }
            result.push(b'%');
            result.extend(unescape(zone.as_bytes(), false, true)?);
        }
        result.extend(suffix.as_bytes());
        Some(result)
    } else {
        if let Some(index) = host.rfind(':') {
            if !port(&host[index..]) {
                return None;
            }
        }
        unescape(host.as_bytes(), true, false)
    }
}
// port: tsc/internal/lsp/lsproto/lsp.go:fixWindowsURIPath
fn fix_windows_uri_path(path: &[u8]) -> JsString {
    if let Some(rest) = path.strip_prefix(b"/") {
        let (mut volume, rest, exists) = tsr_tspath::split_volume_path(rest);
        if exists {
            volume.extend(rest);
            return JsString::from_bytes(volume);
        }
    }
    JsString::from_bytes(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_names_match_the_pinned_uri_probe() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("uri-cases.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let uri = case["uri"].as_str().unwrap();
            let result = std::panic::catch_unwind(|| DocumentUri::from(uri).file_name());
            if let Some(expected) = case["file_hex"].as_str() {
                let expected: Vec<u8> = expected
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|hex| u8::from_str_radix(std::str::from_utf8(hex).unwrap(), 16).unwrap())
                    .collect();
                assert_eq!(result.unwrap().as_bytes(), expected, "{uri}");
            } else {
                assert!(result.is_err(), "{uri}");
            }
        }
    }
    // source: tsc/internal/ls/lsconv/converters_test.go:TestDocumentURIToFileName
    #[test]
    fn document_paths_preserve_scheme_escape_and_drive_rules() {
        for (uri, path) in [
            ("file:///path/to/file.ts", "/path/to/file.ts"),
            ("file://server/share/file.ts", "//server/share/file.ts"),
            ("file:///D%3A/work/utils.ts", "d:/work/utils.ts"),
            ("file:///path/to/file.ts#section", "/path/to/file.ts"),
            ("file://shares/files/c%23/p.cs", "//shares/files/c#/p.cs"),
            ("file:///c:/test %25/path", "c:/test %/path"),
            ("file:///_:/path", "/_:/path"),
            (
                "file://localhost/c%24/GitDevelopment/express",
                "//localhost/c$/GitDevelopment/express",
            ),
            (
                "file:///c%3A/test%20with%20%2525/c%23code",
                "c:/test with %25/c#code",
            ),
            (
                "untitled:Untitled-1#fragment",
                "^/untitled/ts-nul-authority/Untitled-1#fragment",
            ),
            (
                "untitled:C:/Users/me/abc.txt",
                "^/untitled/ts-nul-authority/C:/Users/me/abc.txt",
            ),
            (
                "untitled://wsl%2Bubuntu/home/file.ts",
                "^/untitled/wsl%2Bubuntu/home/file.ts",
            ),
            ("bundled:///libs/lib.d.ts", "bundled:///libs/lib.d.ts"),
        ] {
            assert_eq!(
                DocumentUri::from(uri).file_name().as_bytes(),
                path.as_bytes(),
                "{uri}"
            );
        }
    }
    // source: tsc/internal/ls/lsconv/converters_test.go:TestFileNameToDocumentURI
    #[test]
    fn file_uri_roundtrips_arbitrary_bytes_and_dynamic_schemes() {
        for (path, uri) in [
            (
                "d:/work/(test)/comp.tsx",
                "file:///d%3A/work/%28test%29/comp.tsx",
            ),
            ("//shares/files/c#/p.cs", "file://shares/files/c%23/p.cs"),
            ("c:/test %/path", "file:///c%3A/test%20%25/path"),
            ("/_:/path", "file:///_%3A/path"),
            (
                "^/untitled/ts-nul-authority/Untitled-1",
                "untitled:Untitled-1",
            ),
            (
                "^/untitled/wsl%2Bubuntu/home/file.ts",
                "untitled://wsl%2Bubuntu/home/file.ts",
            ),
        ] {
            assert_eq!(DocumentUri::from_file_name(path.as_bytes()).0, uri);
        }
        let bytes = b"/bad-\xff\xed\xa0\x80-file";
        assert_eq!(
            DocumentUri::from_file_name(bytes).file_name().as_bytes(),
            bytes
        );
        for uri in [
            "file:///bad%",
            "file://bad host/x",
            "file://host:abc/x",
            "file://[invalid]/x",
            "file:///ok#bad%",
            "untitled://missing-path",
        ] {
            assert!(
                std::panic::catch_unwind(|| DocumentUri::from(uri).file_name()).is_err(),
                "{uri}"
            );
        }
    }
}
