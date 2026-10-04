//! Pinned organize-import ordering, including its locale-independent Unicode
//! approximation. Tables come from Go's Unicode and x/text versions.
use crate::{organize_unicode as unicode, Result};
use std::cmp::Ordering;
use tsr_ast::{AstView, NodeId, SyntaxKind as K};

#[derive(Clone, Debug, Default)]
pub struct OrganizeOptions {
    pub sort: String,
    pub ignore_case: Option<bool>,
    pub unicode: bool,
    pub numeric: bool,
    pub accents: Option<bool>,
    pub case_first: String,
    pub type_order: String,
}
#[derive(Clone, Copy)]
enum Mode {
    Ordinal,
    Natural,
    Unicode,
}
#[derive(Clone, Copy)]
pub(crate) struct Comparer {
    mode: Mode,
    ignore: bool,
    numeric: bool,
    accents: bool,
    upper: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeOrder {
    Last,
    Inline,
    First,
}
#[derive(Clone, Copy)]
pub(crate) struct Comparers {
    pub modules: Comparer,
    pub names: Comparer,
    pub types: TypeOrder,
}
impl Comparer {
    pub fn compare(self, a: &[u8], b: &[u8]) -> Ordering {
        match self.mode {
            Mode::Ordinal => {
                if self.ignore {
                    tsr_jsstring::compare::compare_case_insensitive_eslint(a, b)
                } else {
                    a.cmp(b)
                }
            }
            Mode::Natural | Mode::Unicode => {
                let natural = matches!(self.mode, Mode::Natural);
                let numeric = natural || self.numeric;
                compare_keys(&natural_key(a), &natural_key(b), numeric)
                    .then_with(|| {
                        if !natural && self.accents {
                            compare_keys(
                                &tsr_jsstring::helpers::to_lower_go(a),
                                &tsr_jsstring::helpers::to_lower_go(b),
                                numeric,
                            )
                        } else {
                            Ordering::Equal
                        }
                    })
                    .then_with(|| {
                        if self.ignore {
                            Ordering::Equal
                        } else {
                            compare_case(a, b, natural || self.upper)
                        }
                    })
                    .then_with(|| a.cmp(b))
            }
        }
    }
    pub fn modules(self, a: &[u8], b: &[u8]) -> Ordering {
        a.is_empty()
            .cmp(&b.is_empty())
            .then_with(|| {
                tsr_tspath::is_external_module_name_relative(a)
                    .cmp(&tsr_tspath::is_external_module_name_relative(b))
            })
            .then_with(|| self.compare(a, b))
    }
}
impl Comparers {
    pub fn specifiers(self, view: AstView<'_>, a: NodeId, b: NodeId) -> Ordering {
        let a = view.node(a).expect("retained import specifier");
        let b = view.node(b).expect("retained import specifier");
        let type_order = match self.types {
            TypeOrder::First => b.is_type_only().cmp(&a.is_type_only()),
            TypeOrder::Last => a.is_type_only().cmp(&b.is_type_only()),
            TypeOrder::Inline => Ordering::Equal,
        };
        type_order.then_with(|| {
            self.names.compare(
                view.node_text(a.name().expect("specifier name"))
                    .expect("specifier text")
                    .as_bytes(),
                view.node_text(b.name().expect("specifier name"))
                    .expect("specifier text")
                    .as_bytes(),
            )
        })
    }
}
impl OrganizeOptions {
    fn explicit(&self) -> bool {
        !self.sort.is_empty() && self.sort != "auto"
    }
    fn comparer(&self, ignore: bool) -> Comparer {
        let (mode, ignore) = match self.sort.as_str() {
            "ordinal" => (Mode::Ordinal, false),
            "ordinalIgnoreCase" => (Mode::Ordinal, true),
            "natural" => (Mode::Natural, false),
            "naturalIgnoreCase" => (Mode::Natural, true),
            _ => (
                if self.unicode {
                    Mode::Unicode
                } else {
                    Mode::Ordinal
                },
                ignore,
            ),
        };
        Comparer {
            mode,
            ignore,
            numeric: self.numeric,
            accents: self.accents != Some(false),
            upper: self.case_first == "upper",
        }
    }
    // port: tsc/internal/ls/lsutil/organizeimports.go:GetDetectionLists
    #[allow(
        clippy::if_not_else,
        reason = "Keep the pin’s ordinary-import and mixed-type inference branches in source order"
    )]
    pub(crate) fn detect(&self, view: AstView<'_>, groups: &[Vec<NodeId>]) -> Result<Comparers> {
        let comparers = if self.explicit() || self.ignore_case.is_some() {
            vec![self.comparer(self.ignore_case.unwrap_or(false))]
        } else {
            vec![self.comparer(true), self.comparer(false)]
        };
        let types = match self.type_order.as_str() {
            "first" => vec![TypeOrder::First],
            "inline" => vec![TypeOrder::Inline],
            "last" => vec![TypeOrder::Last],
            _ => vec![TypeOrder::Last, TypeOrder::Inline, TypeOrder::First],
        };
        let mut module_groups = Vec::new();
        let mut named = Vec::new();
        let mut mixed = false;
        for group in groups {
            let mut names = Vec::new();
            for &node in group {
                let read = view.node(node)?;
                names.push(
                    read.module_specifier()
                        .map(|n| view.node_text(n).map(|s| s.as_bytes().to_vec()))
                        .transpose()?
                        .unwrap_or_default(),
                );
                let Some(clause) = read
                    .data_source()
                    .as_import_declaration()
                    .and_then(|d| d.import_clause())
                else {
                    continue;
                };
                let Some(bindings) = view
                    .node(clause)?
                    .data_source()
                    .as_import_clause()
                    .and_then(|d| d.named_bindings())
                else {
                    continue;
                };
                if view.node(bindings)?.kind() != K::NamedImports {
                    continue;
                }
                let elements: Vec<_> = view
                    .node_slice(view.node(bindings)?.elements(view)?)?
                    .iter()
                    .flatten()
                    .collect();
                if elements.is_empty() {
                    continue;
                }
                let mut yes = false;
                let mut no = false;
                for &element in &elements {
                    if view.node(element)?.is_type_only() {
                        yes = true;
                    } else {
                        no = true;
                    }
                }
                mixed |= yes && no;
                named.push(elements);
            }
            module_groups.push(names);
        }
        let best_module = best_comparer(&comparers, |compare| {
            module_groups
                .iter()
                .map(|g| {
                    g.windows(2)
                        .filter(|p| compare.compare(&p[0], &p[1]).is_gt())
                        .count()
                })
                .sum()
        });
        let mut result = Comparers {
            modules: best_module,
            names: self.comparer(false),
            types: types[0],
        };
        if self.explicit() || self.ignore_case.is_some() {
            result.modules = comparers[0];
            result.names = comparers[0];
        }
        if named.is_empty() {
            return Ok(result);
        }
        if !mixed {
            let best = best_comparer(&comparers, |compare| {
                named
                    .iter()
                    .map(|g| {
                        g.windows(2)
                            .filter(|p| {
                                let name = |id| {
                                    view.node_text(view.node(id).unwrap().name().unwrap())
                                        .unwrap()
                                };
                                compare
                                    .compare(name(p[0]).as_bytes(), name(p[1]).as_bytes())
                                    .is_gt()
                            })
                            .count()
                    })
                    .sum()
            });
            if !self.explicit() && self.ignore_case.is_none() {
                result.names = best;
            }
        } else {
            let mut best: Option<(usize, Comparer, TypeOrder)> = None;
            // Type-order priority wins ties, then comparer detection order.
            for &types in &types {
                for &names in &comparers {
                    let compare = Comparers {
                        modules: result.modules,
                        names,
                        types,
                    };
                    let diff = named
                        .iter()
                        .map(|g| {
                            g.windows(2)
                                .filter(|p| compare.specifiers(view, p[0], p[1]).is_gt())
                                .count()
                        })
                        .sum();
                    if best.as_ref().is_none_or(|b| diff < b.0) {
                        best = Some((diff, names, types));
                    }
                }
            }
            if let Some((_, names, types)) = best {
                result.names = names;
                result.types = types;
            }
        }
        Ok(result)
    }
}
fn best_comparer(comparers: &[Comparer], diff: impl Fn(Comparer) -> usize) -> Comparer {
    let mut best = comparers[0];
    let mut best_diff = usize::MAX;
    for &comparer in comparers {
        let n = diff(comparer);
        if n < best_diff {
            best = comparer;
            best_diff = n;
        }
    }
    best
}
fn properties(r: u32) -> (u32, u8, bool, bool) {
    unicode::PROPERTIES
        .binary_search_by_key(&r, |p| p.0)
        .map_or((r, 0, false, false), |i| {
            let p = unicode::PROPERTIES[i];
            (p.1, p.2, p.3, p.4)
        })
}
fn runes(mut text: &[u8]) -> Vec<u32> {
    let mut result = Vec::new();
    while !text.is_empty() {
        let (r, n) = tsr_jsstring::wtf8::decode_utf8(text);
        result.push(r as u32);
        text = &text[n..];
    }
    result
}
// port: tsc/internal/ls/lsutil/organizeimports.go:naturalCollationKey
fn natural_key(text: &[u8]) -> Vec<u8> {
    let mut normalized: Vec<u32> = Vec::new();
    let mut nonstarters = 0;
    for rune in runes(text) {
        let hangul;
        let identity = [rune];
        let decomposition = if (0xac00..=0xd7a3).contains(&rune) {
            let index = rune - 0xac00;
            hangul = [
                0x1100 + index / 588,
                0x1161 + index % 588 / 28,
                0x11a7 + index % 28,
            ];
            &hangul[..if index % 28 == 0 { 2 } else { 3 }]
        } else if let Ok(i) = unicode::DECOMPOSITIONS.binary_search_by_key(&rune, |d| d.0) {
            unicode::DECOMPOSITIONS[i].1
        } else {
            &identity
        };
        for &r in decomposition {
            let ccc = properties(r).1;
            if ccc == 0 {
                nonstarters = 0;
            } else {
                // x/text's stream-safe NFD inserts CGJ after 30 nonstarters.
                if nonstarters == 30 {
                    normalized.push(0x34f);
                    nonstarters = 0;
                }
                nonstarters += 1;
            }
            normalized.push(r);
            let mut index = normalized.len() - 1;
            if ccc != 0 {
                while index > 0 && properties(normalized[index - 1]).1 > ccc {
                    normalized.swap(index - 1, index);
                    index -= 1;
                }
            }
        }
    }
    let mut output = Vec::new();
    for r in normalized {
        let (lower, _, mark, _) = properties(r);
        if !mark {
            let mut buf = [0; 4];
            output.extend_from_slice(
                char::from_u32(lower)
                    .expect("Unicode scalar")
                    .encode_utf8(&mut buf)
                    .as_bytes(),
            );
        }
    }
    output
}
// port: tsc/internal/ls/lsutil/organizeimports.go:compareOrganizeImportsCase
fn compare_case(a: &[u8], b: &[u8], upper_first: bool) -> Ordering {
    let a = runes(a);
    let b = runes(b);
    for (&a, &b) in a.iter().zip(&b) {
        let a = properties(a).3;
        let b = properties(b).3;
        if a != b {
            return if upper_first { b.cmp(&a) } else { a.cmp(&b) };
        }
    }
    a.len().cmp(&b.len())
}
// port: tsc/internal/ls/lsutil/organizeimports.go:compareStringsNumeric
fn compare_keys(mut a: &[u8], mut b: &[u8], numeric: bool) -> Ordering {
    if !numeric {
        return a.cmp(b);
    }
    while !a.is_empty() && !b.is_empty() {
        if a[0].is_ascii_digit() && b[0].is_ascii_digit() {
            let al = a.iter().take_while(|b| b.is_ascii_digit()).count();
            let bl = b.iter().take_while(|b| b.is_ascii_digit()).count();
            let strip = |s: &[u8]| s.iter().position(|&b| b != b'0').unwrap_or(s.len() - 1);
            let ad = &a[strip(&a[..al])..al];
            let bd = &b[strip(&b[..bl])..bl];
            let order = ad
                .len()
                .cmp(&bd.len())
                .then_with(|| ad.cmp(bd))
                .then_with(|| a[..al].cmp(&b[..bl]));
            if !order.is_eq() {
                return order;
            }
            a = &a[al..];
            b = &b[bl..];
        } else {
            let (ar, al) = tsr_jsstring::wtf8::decode_utf8(a);
            let (br, bl) = tsr_jsstring::wtf8::decode_utf8(b);
            if ar != br {
                return ar.cmp(&br);
            }
            a = &a[al..];
            b = &b[bl..];
        }
    }
    a.len().cmp(&b.len())
}
