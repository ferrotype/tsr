//! The five flags registered by the pin's runLSP, with flag.FlagSet parsing.
//! This deliberately is not a general compiler command-line parser.
use tsr_jsstring::{go_quote, JsString};

pub(super) const USAGE: &str = "Usage of lsp:\n  -clientProcessId int\n    \tuse the given PID for the parent process watchdog\n  -pipe string\n    \tuse named pipe for communication\n  -pprofDir string\n    \tGenerate pprof CPU/memory profiles to the given directory.\n  -socket string\n    \tuse socket for communication\n  -stdio\n    \tuse stdio for communication\n";

#[derive(Default, Debug)]
pub(super) struct Flags {
    pub stdio: bool,
    pub parent: isize,
    pub pprof: Vec<u8>,
}
pub(super) fn parse(args: &[JsString]) -> Result<Flags, String> {
    let mut result = Flags::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let bytes = arg.as_bytes();
        if bytes == b"--" || !bytes.starts_with(b"-") || bytes == b"-" {
            break;
        }
        let name = bytes.strip_prefix(b"--").unwrap_or(&bytes[1..]);
        if matches!(name.first(), None | Some(b'-' | b'=')) {
            return Err(format!(
                "bad flag syntax: {}",
                String::from_utf8_lossy(bytes)
            ));
        }
        let split = name.iter().position(|c| *c == b'=');
        let (name, mut value) =
            split.map_or((name, None), |at| (&name[..at], Some(&name[at + 1..])));
        if name == b"stdio" {
            result.stdio = match value {
                None | Some(b"true" | b"TRUE" | b"True" | b"1" | b"t" | b"T") => true,
                Some(b"false" | b"FALSE" | b"False" | b"0" | b"f" | b"F") => false,
                Some(value) => {
                    return Err(format!(
                        "invalid boolean value {} for -stdio: parse error",
                        go_quote(value)
                    ))
                }
            };
            continue;
        }
        if matches!(name, b"h" | b"help") {
            return Err(String::new());
        }
        if ![
            b"pipe".as_slice(),
            b"socket",
            b"clientProcessId",
            b"pprofDir",
        ]
        .contains(&name)
        {
            return Err(format!(
                "flag provided but not defined: -{}",
                String::from_utf8_lossy(name)
            ));
        }
        if value.is_none() {
            value = args.next().map(JsString::as_bytes);
        }
        let value = value
            .ok_or_else(|| format!("flag needs an argument: -{}", String::from_utf8_lossy(name)))?;
        match name {
            b"clientProcessId" => {
                result.parent = go_int(value).map_err(|reason| {
                    format!(
                        "invalid value {} for flag -clientProcessId: {reason}",
                        go_quote(value)
                    )
                })?
            }
            b"pprofDir" => result.pprof = value.to_vec(),
            _ => {}
        }
    }
    Ok(result)
}

// strconv.ParseInt(s, 0, strconv.IntSize), as used by flag.intValue.Set.
// Preserve syntax-versus-overflow precedence and validate underscores after
// the unsigned accumulation, just as the pin's toolchain does.
fn go_int(mut input: &[u8]) -> Result<isize, &'static str> {
    let negative = input.first() == Some(&b'-');
    if matches!(input.first(), Some(b'+' | b'-')) {
        input = &input[1..];
    }
    if input.is_empty() {
        return Err("parse error");
    }
    let mut prefix = 0;
    let base = if input[0] == b'0' {
        prefix = 1;
        match input
            .get(1)
            .map(u8::to_ascii_lowercase)
            .filter(|_| input.len() >= 3)
        {
            Some(b'x') => {
                prefix = 2;
                16
            }
            Some(b'b') => {
                prefix = 2;
                2
            }
            Some(b'o') => {
                prefix = 2;
                8
            }
            _ => 8,
        }
    } else {
        10
    };
    let mut value = 0usize;
    for &byte in &input[prefix..] {
        let digit = match byte.to_ascii_lowercase() {
            b'_' => continue,
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'z' => byte.to_ascii_lowercase() - b'a' + 10,
            _ => return Err("parse error"),
        };
        if usize::from(digit) >= base {
            return Err("parse error");
        }
        value = value
            .checked_mul(base)
            .and_then(|v| v.checked_add(usize::from(digit)))
            .ok_or("value out of range")?;
    }
    let mut previous_digit = prefix > 0;
    for &byte in &input[prefix..] {
        if byte == b'_' && !previous_digit {
            return Err("parse error");
        }
        previous_digit = byte != b'_';
    }
    if !previous_digit {
        return Err("parse error");
    }
    if value > isize::MAX as usize + usize::from(negative) {
        return Err("value out of range");
    }
    Ok(if negative {
        (value as isize).wrapping_neg()
    } else {
        value as isize
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn go_int_bases_separators_and_bounds() {
        for (input, expected) in [
            ("0", 0),
            ("0x10", 16),
            ("0X_Ff", 255),
            ("0b1_01", 5),
            ("0o17", 15),
            ("017", 15),
            ("0_7", 7),
            ("-0x10", -16),
            ("+1_234", 1234),
        ] {
            assert_eq!(go_int(input.as_bytes()), Ok(expected), "{input}");
        }
        for input in [
            "", "-", "+", "0x", "08", "_1", "1_", "0x_", "1__2", "0b2", " 1", "1.0",
        ] {
            assert_eq!(go_int(input.as_bytes()), Err("parse error"), "{input}");
        }
        for value in [isize::MIN, isize::MAX] {
            assert_eq!(go_int(value.to_string().as_bytes()), Ok(value));
        }
        assert_eq!(
            go_int((isize::MAX as u128 + 1).to_string().as_bytes()),
            Err("value out of range")
        );
    }
}
