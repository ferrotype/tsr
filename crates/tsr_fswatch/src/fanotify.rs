#![forbid(unsafe_code)]
//! Safe parsing of the kernel's variable-size fanotify FID records.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct HandleKey {
    pub(crate) fsid: [i32; 2],
    pub(crate) handle_type: i32,
    pub(crate) handle: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DfidName {
    pub(crate) key: HandleKey,
    pub(crate) name: Vec<u8>,
}
// port: tsc/internal/fswatch/fanotify_linux.go:parseFanotifyFidRecord
pub(crate) fn parse_record(data: &[u8], has_name: bool) -> Option<DfidName> {
    if data.len() < 20 {
        return None;
    }
    let fsid = [
        i32::from_ne_bytes(data[4..8].try_into().ok()?),
        i32::from_ne_bytes(data[8..12].try_into().ok()?),
    ];
    let size = u32::from_ne_bytes(data[12..16].try_into().ok()?) as usize;
    let handle_type = i32::from_ne_bytes(data[16..20].try_into().ok()?);
    let end = 20usize.checked_add(size)?;
    let handle = data.get(20..end)?.to_vec();
    let name = if has_name {
        let name = data.get(end..)?;
        name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())].to_vec()
    } else {
        Vec::new()
    };
    Some(DfidName {
        key: HandleKey {
            fsid,
            handle_type,
            handle,
        },
        name,
    })
}
// port: tsc/internal/fswatch/fanotify_linux.go:parseFanotifyDfidNames
pub(crate) fn parse_dfid_names(mut data: &[u8]) -> (Option<DfidName>, Option<DfidName>) {
    let (mut primary, mut rename) = (None, None);
    while data.len() >= 4 {
        let length = usize::from(u16::from_ne_bytes([data[2], data[3]]));
        if length < 4 || length > data.len() {
            break;
        }
        match data[0] {
            2 | 10 => {
                if let Some(record) = parse_record(&data[..length], true) {
                    primary = Some(record)
                }
            }
            12 => {
                if let Some(record) = parse_record(&data[..length], true) {
                    rename = Some(record)
                }
            }
            3 if primary.is_none() => primary = parse_record(&data[..length], false),
            _ => {}
        }
        if primary.is_some() && rename.is_some() {
            break;
        }
        data = &data[length..];
    }
    (primary, rename)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record(kind: u8, name: &[u8]) -> Vec<u8> {
        let mut data = vec![kind, 0];
        data.extend_from_slice(&((24 + name.len() + 1) as u16).to_ne_bytes());
        data.extend_from_slice(&7i32.to_ne_bytes());
        data.extend_from_slice(&9i32.to_ne_bytes());
        data.extend_from_slice(&4u32.to_ne_bytes());
        data.extend_from_slice(&2i32.to_ne_bytes());
        data.extend_from_slice(b"fid!");
        data.extend_from_slice(name);
        data.push(0);
        data
    }
    #[test]
    fn parses_rename_and_fid_fallback() {
        let mut data = record(10, b"old");
        data.extend(record(12, b"new"));
        let (a, b) = parse_dfid_names(&data);
        assert_eq!(a.unwrap().name, b"old");
        assert_eq!(b.unwrap().name, b"new");
        let (a, b) = parse_dfid_names(&record(3, b"ignored"));
        assert_eq!(a.unwrap().name, b"");
        assert!(b.is_none());
    }
    #[test]
    fn malformed_lengths_are_bounded() {
        for size in 0..24 {
            assert!(parse_record(&vec![0; size], true).is_none() || size >= 20);
        }
        let mut data = record(2, b"x");
        data[12..16].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(parse_record(&data, true).is_none());
        data[2..4].copy_from_slice(&0u16.to_ne_bytes());
        assert_eq!(parse_dfid_names(&data), (None, None));
    }
}
