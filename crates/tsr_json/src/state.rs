use crate::Kind;
use std::collections::HashSet;

/// Small objects share one decoded-name allocation. Large objects switch to a
/// hash set, matching the pin's bounded linear-search policy. Names outlive any
/// flush of the output buffer; raw escape spellings do not affect equality.
#[derive(Clone, Debug, Default)]
pub(crate) struct Names {
    bytes: Vec<u8>,
    ends: Vec<usize>,
    large: Option<HashSet<Vec<u8>>>,
}
impl Names {
    pub fn insert(&mut self, name: &[u8]) -> bool {
        if self.large.is_none() && (self.ends.len() > 64 || self.bytes.len() > 1024) {
            let mut start = 0;
            self.large = Some(
                self.ends
                    .iter()
                    .map(|&end| {
                        let name = self.bytes[start..end].to_vec();
                        start = end;
                        name
                    })
                    .collect(),
            );
        }
        if let Some(large) = &mut self.large {
            if !large.insert(name.to_vec()) {
                return false;
            }
        } else {
            let mut start = 0;
            for &end in &self.ends {
                if &self.bytes[start..end] == name {
                    return false;
                }
                start = end;
            }
        }
        self.bytes.extend_from_slice(name);
        self.ends.push(self.bytes.len());
        true
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Frame {
    pub kind: Kind,
    pub count: usize,
    pub name: Vec<u8>,
    pub names: Names,
}
impl Frame {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            count: 0,
            name: Vec::new(),
            names: Names::default(),
        }
    }
    pub fn expects_name(&self) -> bool {
        self.kind == Kind::BeginObject && self.count.is_multiple_of(2)
    }
}
pub(crate) fn append_pointer(out: &mut String, name: &[u8]) {
    out.push('/');
    // Names have already been decoded with the selected UTF-8 policy.
    for c in String::from_utf8_lossy(name).chars() {
        match c {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            _ => out.push(c),
        }
    }
}
pub(crate) fn pointer(stack: &[Frame], next: bool) -> String {
    let mut out = String::new();
    for (i, f) in stack.iter().enumerate() {
        let last = i + 1 == stack.len();
        if f.kind == Kind::BeginObject {
            if f.count == 0 || last && next && f.expects_name() {
                continue;
            }
            append_pointer(&mut out, &f.name);
        } else if f.count > 0 || last && next {
            append_pointer(
                &mut out,
                if last && next {
                    f.count
                } else {
                    f.count.saturating_sub(1)
                }
                .to_string()
                .as_bytes(),
            );
        }
    }
    out
}
