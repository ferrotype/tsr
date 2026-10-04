/// Go's Perl classes and word boundaries are ASCII even in a Unicode regexp.
/// Quoted literals use RE2's `\Q…\E` form, which the Rust engine does not parse.
fn lexical_pattern(pattern: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = pattern.char_indices().peekable();
    let mut class = false;
    let mut class_start = false;
    while let Some((at, ch)) = chars.next() {
        if ch == '\\' {
            let (_, escaped) = chars.next()?;
            if escaped == 'Q' {
                if class {
                    return None;
                }
                let start = chars.peek().map_or(pattern.len(), |(n, _)| *n);
                let tail = &pattern[start..];
                let end = tail.find("\\E").map_or(pattern.len(), |n| start + n);
                out.push_str(&regex::escape(&pattern[start..end]));
                while chars
                    .peek()
                    .is_some_and(|(at, _)| *at < end.saturating_add(2))
                {
                    chars.next();
                }
                class_start = false;
                continue;
            }
            if matches!(escaped, '1'..='7')
                && !chars.peek().is_some_and(|(_, c)| matches!(c, '0'..='7'))
            {
                return None;
            }
            let ascii = match escaped {
                'd' => Some("0-9"),
                'w' => Some("A-Za-z0-9_"),
                's' => Some("\\t\\n\\f\\r "),
                _ => None,
            };
            if let Some(set) = ascii {
                if class {
                    out.push_str(set);
                } else {
                    out.push('[');
                    out.push_str(set);
                    out.push(']');
                }
            } else if matches!(escaped, 'D' | 'W' | 'S') {
                let set = match escaped {
                    'D' => "0-9",
                    'W' => "A-Za-z0-9_",
                    _ => "\\t\\n\\f\\r ",
                };
                out.push_str("[^");
                out.push_str(set);
                out.push(']');
            } else if matches!(escaped, 'b' | 'B') && !class {
                out.push_str(if escaped == 'b' {
                    "(?-u:\\b)"
                } else {
                    "(?-u:\\B)"
                });
            } else {
                // Go rejects escapes for unknown ASCII letters, even when the
                // Rust parser assigns them another meaning (e.g. \e).
                if escaped.is_ascii_alphabetic() && !"afnrtvAxzPp".contains(escaped) {
                    return None;
                }
                out.push('\\');
                out.push(escaped);
                if matches!(escaped, 'p' | 'P' | 'x')
                    && chars.peek().is_some_and(|(_, c)| *c == '{')
                {
                    let (_, open) = chars.next()?;
                    out.push(open);
                    loop {
                        let (_, ch) = chars.next()?;
                        out.push(ch);
                        if ch == '}' {
                            break;
                        }
                    }
                }
            }
            class_start = false;
            continue;
        }
        if class {
            if ch == ']' && !class_start {
                class = false;
                out.push(ch);
            } else if ch == '^' && class_start {
                out.push(ch);
            } else {
                // RE2 treats nested [, &&, -- and ~~ literally, not as the
                // Rust engine's Unicode class set operations. POSIX classes
                // retain their shared syntax.
                if ch == '[' && pattern[at..].starts_with("[:") {
                    let end = pattern[at..].find(":]")? + at + 2;
                    out.push_str(&pattern[at..end]);
                    while chars.peek().is_some_and(|(at, _)| *at < end) {
                        chars.next();
                    }
                } else {
                    if matches!(ch, '[' | ']' | '&' | '~') {
                        out.push('\\');
                    }
                    out.push(ch);
                }
                class_start = false;
            }
            continue;
        }
        if ch == '[' {
            class = true;
            class_start = true;
        }
        if ch == '(' && pattern[at..].starts_with("(?") {
            let tail = &pattern[at + 2..];
            if let Some(name) = tail.strip_prefix("P<").or_else(|| tail.strip_prefix('<')) {
                let end = name.find('>')?;
                let name = &name[..end];
                if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    return None;
                }
                let end = at + 2 + tail.find('>')?;
                while chars.peek().is_some_and(|(i, _)| *i <= end) {
                    chars.next();
                }
                // Captures are unobservable to the exclusion matcher. Go
                // permits duplicate names and names starting with digits.
                out.push_str("(?:");
                continue;
            }
            let end = tail.find([':', ')'])?;
            let terminator = tail.as_bytes()[end] as char;
            let mut flags = std::collections::BTreeMap::new();
            let mut negative = false;
            let mut saw = false;
            for ch in tail[..end].chars() {
                if ch == '-' {
                    if negative {
                        return None;
                    }
                    negative = true;
                    saw = false;
                } else if "imsU".contains(ch) {
                    flags.insert(ch, !negative);
                    saw = true;
                } else {
                    return None;
                }
            }
            if negative && !saw {
                return None;
            }
            let yes = flags
                .iter()
                .filter_map(|(&c, &yes)| yes.then_some(c))
                .collect::<String>();
            let no = flags
                .iter()
                .filter_map(|(&c, &yes)| (!yes).then_some(c))
                .collect::<String>();
            if terminator == ':' || !flags.is_empty() {
                out.push_str("(?");
                out.push_str(&yes);
                if !no.is_empty() {
                    out.push('-');
                    out.push_str(&no);
                }
                out.push(terminator);
            }
            while chars.peek().is_some_and(|(i, _)| *i <= at + 2 + end) {
                chars.next();
            }
            continue;
        }
        if ch == '{' && !repeat_prefix(&pattern[at + 1..]) {
            out.push('\\');
        }
        out.push(ch);
    }
    Some(out)
}

// regexp/syntax.parseRepeat treats malformed counts as literal text.
fn repeat_prefix(tail: &str) -> bool {
    fn number(input: &str) -> Option<&str> {
        let n = input.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 || n > 1 && input.starts_with('0') {
            None
        } else {
            Some(&input[n..])
        }
    }
    let Some(mut rest) = number(tail) else {
        return false;
    };
    if let Some(after) = rest.strip_prefix(',') {
        rest = if after.starts_with('}') {
            after
        } else {
            let Some(after) = number(after) else {
                return false;
            };
            after
        };
    }
    rest.starts_with('}')
}

use regex_syntax::{
    ast::{self, Ast},
    hir::{self, ClassUnicode, ClassUnicodeRange},
};
use std::fmt::Write;

pub(super) fn compile(pattern: &str, insensitive: bool) -> Option<regex::bytes::Regex> {
    let pattern = lexical_pattern(pattern)?;
    let ast = ast::parse::ParserBuilder::new()
        .nest_limit(1000)
        .octal(true)
        .build()
        .parse(&pattern)
        .ok()?;
    let mut rendered = String::new();
    let mut insensitive = insensitive;
    render(&ast, &mut insensitive, 1000, &mut rendered)?;
    regex::bytes::RegexBuilder::new(&rendered)
        .nest_limit(1100)
        .build()
        .ok()
}

fn render(ast: &Ast, insensitive: &mut bool, budget: u32, out: &mut String) -> Option<()> {
    match ast {
        Ast::Literal(lit) => write_class(
            out,
            &fold(
                ranges(&[(u32::from(lit.c), u32::from(lit.c))]),
                *insensitive,
            ),
        ),
        Ast::ClassUnicode(class) => write_class(out, &unicode_class(class, *insensitive)?),
        Ast::ClassBracketed(class) => write_class(out, &bracketed(class, *insensitive)?),
        Ast::Flags(flags) => {
            *insensitive = flags
                .flags
                .flag_state(ast::Flag::CaseInsensitive)
                .unwrap_or(*insensitive);
            let flags = remaining_flags(&flags.flags);
            if !flags.is_empty() {
                write!(out, "(?{flags})").unwrap();
            }
        }
        Ast::Group(group) => {
            let mut local = *insensitive;
            let flags = group.flags().map_or_else(String::new, |f| {
                local = f.flag_state(ast::Flag::CaseInsensitive).unwrap_or(local);
                remaining_flags(f)
            });
            write!(out, "(?{flags}:").unwrap();
            render(&group.ast, &mut local, budget, out)?;
            out.push(')');
        }
        Ast::Concat(concat) => {
            for item in &concat.asts {
                render(item, insensitive, budget, out)?;
            }
        }
        Ast::Alternation(alt) => {
            for (n, item) in alt.asts.iter().enumerate() {
                if n > 0 {
                    out.push('|');
                }
                render(item, insensitive, budget, out)?;
            }
        }
        Ast::Repetition(rep) => {
            use ast::{
                RepetitionKind::Range,
                RepetitionRange::{AtLeast, Bounded, Exactly},
            };
            let count = match rep.op.kind {
                Range(Exactly(n) | AtLeast(n) | Bounded(_, n)) => n,
                _ => 1,
            };
            if count > budget {
                return None;
            }
            out.push_str("(?:");
            // A zero repeat masks nested repeats in Go's repeatIsValid.
            render(
                &rep.ast,
                insensitive,
                budget.checked_div(count).unwrap_or(1000),
                out,
            )?;
            out.push(')');
            match rep.op.kind {
                ast::RepetitionKind::ZeroOrOne => out.push('?'),
                ast::RepetitionKind::ZeroOrMore => out.push('*'),
                ast::RepetitionKind::OneOrMore => out.push('+'),
                Range(Exactly(n)) => write!(out, "{{{n}}}").unwrap(),
                Range(AtLeast(n)) => write!(out, "{{{n},}}").unwrap(),
                Range(Bounded(a, b)) => write!(out, "{{{a},{b}}}").unwrap(),
            }
            if !rep.greedy {
                out.push('?');
            }
        }
        _ => write!(out, "{ast}").unwrap(),
    }
    Some(())
}
fn remaining_flags(flags: &ast::Flags) -> String {
    let mut yes = String::new();
    let mut no = String::new();
    for (flag, text) in [
        (ast::Flag::MultiLine, 'm'),
        (ast::Flag::DotMatchesNewLine, 's'),
        (ast::Flag::SwapGreed, 'U'),
        (ast::Flag::Unicode, 'u'),
    ] {
        match flags.flag_state(flag) {
            Some(true) => yes.push(text),
            Some(false) => no.push(text),
            None => (),
        }
    }
    if !no.is_empty() {
        yes.push('-');
        yes.push_str(&no);
    }
    yes
}
fn ranges(input: &[(u32, u32)]) -> ClassUnicode {
    ClassUnicode::new(input.iter().flat_map(|&(lo, hi)| {
        // Rust scalars exclude surrogate code points. Go's decoder likewise
        // never yields them from an input string (it yields RuneError instead).
        [(lo, hi.min(0xd7ff)), (lo.max(0xe000), hi)]
            .into_iter()
            .filter_map(|(lo, hi)| {
                (lo <= hi).then(|| {
                    Some(ClassUnicodeRange::new(
                        char::from_u32(lo)?,
                        char::from_u32(hi)?,
                    ))
                })?
            })
    }))
}
fn fold(mut class: ClassUnicode, insensitive: bool) -> ClassUnicode {
    if insensitive {
        let input: Vec<_> = class
            .iter()
            .map(|r| (u32::from(r.start()), u32::from(r.end())))
            .collect();
        class.union(&ranges(
            &tsr_jsstring::simple_fold_additions(&input)
                .into_iter()
                .map(|r| (r, r))
                .collect::<Vec<_>>(),
        ));
    }
    class
}
fn unicode_class(class: &ast::ClassUnicode, insensitive: bool) -> Option<ClassUnicode> {
    let name = match &class.kind {
        ast::ClassUnicodeKind::Named(n) => n.clone(),
        ast::ClassUnicodeKind::OneLetter(c) => c.to_string(),
        ast::ClassUnicodeKind::NamedValue { .. } => return None,
    };
    let (name, reverse) = name
        .strip_prefix('^')
        .map_or((name.as_str(), false), |n| (n, true));
    let table = if name == "Any" {
        &[(0, 0x0010_ffff)][..]
    } else {
        let index = super::regexp_unicode_generated::PROPERTIES
            .binary_search_by_key(&name, |&(key, _)| key)
            .ok()?;
        super::regexp_unicode_generated::PROPERTIES[index].1
    };
    let mut result = fold(ranges(table), insensitive);
    if class.negated != reverse {
        result.negate();
    }
    Some(result)
}
fn bracketed(class: &ast::ClassBracketed, insensitive: bool) -> Option<ClassUnicode> {
    let mut result = class_set(&class.kind, insensitive)?;
    if class.negated {
        result.negate();
    }
    Some(result)
}
fn class_set(set: &ast::ClassSet, insensitive: bool) -> Option<ClassUnicode> {
    match set {
        ast::ClassSet::Item(item) => class_item(item, insensitive),
        ast::ClassSet::BinaryOp(_) => None,
    }
}
fn class_item(item: &ast::ClassSetItem, insensitive: bool) -> Option<ClassUnicode> {
    use ast::ClassSetItem as I;
    Some(match item {
        I::Empty(_) => ClassUnicode::empty(),
        I::Literal(l) => fold(ranges(&[(u32::from(l.c), u32::from(l.c))]), insensitive),
        I::Range(r) => fold(
            ranges(&[(u32::from(r.start.c), u32::from(r.end.c))]),
            insensitive,
        ),
        I::Bracketed(b) => bracketed(b, insensitive)?,
        I::Unicode(u) => unicode_class(u, insensitive)?,
        I::Union(u) => {
            let mut result = ClassUnicode::empty();
            for item in &u.items {
                result.union(&class_item(item, insensitive)?);
            }
            result
        }
        I::Ascii(a) => {
            let mut positive = a.clone();
            positive.negated = false;
            let pattern = format!(
                "{}",
                ast::Ast::class_bracketed(ast::ClassBracketed {
                    span: a.span,
                    negated: false,
                    kind: ast::ClassSet::Item(I::Ascii(positive))
                })
            );
            let hir = regex_syntax::Parser::new().parse(&pattern).ok()?;
            let hir::HirKind::Class(hir::Class::Unicode(c)) = hir.kind() else {
                return None;
            };
            let mut c = fold(c.clone(), insensitive);
            if a.negated {
                c.negate();
            }
            c
        }
        // Perl classes have already been translated by lexical_pattern.
        I::Perl(_) => return None,
    })
}
fn write_class(out: &mut String, class: &ClassUnicode) {
    if class.iter().next().is_none() {
        out.push_str("[^\\x{0}-\\x{10ffff}]");
        return;
    }
    out.push('[');
    for r in class.iter() {
        write!(out, "\\x{{{:x}}}", u32::from(r.start())).unwrap();
        if r.start() != r.end() {
            write!(out, "-\\x{{{:x}}}", u32::from(r.end())).unwrap();
        }
    }
    out.push(']');
}
