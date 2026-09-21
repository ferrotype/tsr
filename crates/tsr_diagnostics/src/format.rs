//! Diagnostic substitution and `%v` argument rendering at the pinned boundary.

/// port: tsc/internal/diagnostics/diagnostics.go:Format
pub fn format(text: &[u8], args: &[&[u8]]) -> Vec<u8> {
    try_format(text, args).unwrap_or_else(|error| panic!("{error}"))
}
/// Fallible adapter for Rust APIs whose invariant failures travel as Results.
pub fn try_format(text: &[u8], args: &[&[u8]]) -> Result<Vec<u8>, &'static str> {
    if args.is_empty() {
        return Ok(text.to_vec());
    }
    let args: Vec<_> = args.iter().map(|arg| valid_argument(arg)).collect();
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if text[at] == b'{' {
            let start = at + 1;
            let mut end = start;
            while text.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            if end > start && text.get(end) == Some(&b'}') {
                let index = std::str::from_utf8(&text[start..end])
                    .unwrap()
                    .parse::<isize>()
                    .ok()
                    .and_then(|index| usize::try_from(index).ok())
                    .filter(|&index| index < args.len())
                    .ok_or("Invalid formatting placeholder")?;
                out.extend_from_slice(&args[index]);
                at = end + 1;
                continue;
            }
        }
        out.push(text[at]);
        at += 1;
    }
    Ok(out)
}

// strings.ToValidUTF8 collapses adjacent invalid sequences into one replacement.
// Rust's from_utf8_lossy replaces each sequence separately.
pub fn valid_argument(mut bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut invalid = false;
    loop {
        match std::str::from_utf8(bytes) {
            Ok(_) => {
                out.extend_from_slice(bytes);
                return out;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                out.extend_from_slice(&bytes[..valid]);
                if valid != 0 {
                    invalid = false;
                }
                if !invalid {
                    out.extend_from_slice("\u{fffd}".as_bytes());
                    invalid = true;
                }
                let Some(len) = error.error_len() else {
                    return out;
                };
                bytes = &bytes[valid + len..];
            }
        }
    }
}

/// Explicit dynamic values corresponding to Go's diagnostic `...any` inputs.
/// Custom application values can supply their Go-compatible display as Bytes.
#[derive(Clone, Debug)]
pub enum Argument {
    Bytes(Vec<u8>),
    Int(i64),
    Uint(u64),
    Float(f64),
    Bool(bool),
    Null,
    List(Vec<Argument>),
}
impl Argument {
    pub fn render(&self) -> Vec<u8> {
        match self {
            Self::Bytes(value) => value.clone(),
            Self::Int(value) => value.to_string().into_bytes(),
            Self::Uint(value) => value.to_string().into_bytes(),
            Self::Float(value) => float_text(*value).into_bytes(),
            Self::Bool(value) => value.to_string().into_bytes(),
            Self::Null => b"<nil>".to_vec(),
            Self::List(values) => {
                let mut out = vec![b'['];
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        out.push(b' ');
                    }
                    out.extend(value.render());
                }
                out.push(b']');
                out
            }
        }
    }
}
fn float_text(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Inf"
        } else {
            "+Inf"
        }
        .into();
    }
    let exponent = format!("{value:e}");
    let (mantissa, power) = exponent.split_once('e').unwrap();
    let power: i32 = power.parse().unwrap();
    if (-4..6).contains(&power) {
        value.to_string()
    } else {
        format!("{mantissa}e{power:+03}")
    }
}
/// port: tsc/internal/diagnostics/diagnostics.go:StringifyArgs
pub fn stringify_args(args: &[Argument]) -> Option<Vec<Vec<u8>>> {
    if args.is_empty() {
        None
    } else {
        Some(args.iter().map(Argument::render).collect())
    }
}
