//! The six flags of the pin's `api` flag set, parsed as Go's `flag` package
//! parses them: one or two dashes, `-name value` or `-name=value`, booleans
//! without a value.
//! port: tsc/cmd/tsc/api.go:parseAPIFlags
use tsr_jsstring::{go_quote, JsString};

pub(super) const USAGE: &str = "Usage of api:\n  -async\n    \tuse JSON-RPC protocol instead of MessagePack (for async API)\n  -callbacks string\n    \tcomma-separated list of FS callbacks to enable (readFile,fileExists,directoryExists,getAccessibleEntries,realpath)\n  -cwd string\n    \tcurrent working directory (default current directory)\n  -pipe string\n    \tuse named pipe or Unix domain socket for communication instead of stdio\n  -runExternalCode\n    \tallow projects to execute configured external plugins\n  -timing\n    \tcollect per-request server processing time, folded into the client's timing snapshot\n";

#[derive(Default, Debug, PartialEq, Eq)]
pub(super) struct Flags {
    pub cwd: Option<JsString>,
    pub pipe: String,
    pub callbacks: String,
    pub r#async: bool,
    pub timing: bool,
    pub run_external_code: bool,
}

fn boolean(name: &str, value: Option<&[u8]>) -> Result<bool, String> {
    match value {
        None | Some(b"true" | b"TRUE" | b"True" | b"1" | b"t" | b"T") => Ok(true),
        Some(b"false" | b"FALSE" | b"False" | b"0" | b"f" | b"F") => Ok(false),
        Some(value) => Err(format!(
            "invalid boolean value {} for -{name}: parse error",
            go_quote(value)
        )),
    }
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
        let (name, value) = split.map_or((name, None), |at| (&name[..at], Some(&name[at + 1..])));
        match name {
            b"h" | b"help" => return Err(String::new()),
            b"async" => result.r#async = boolean("async", value)?,
            b"timing" => result.timing = boolean("timing", value)?,
            b"runExternalCode" => result.run_external_code = boolean("runExternalCode", value)?,
            b"cwd" | b"pipe" | b"callbacks" => {
                let value = match value {
                    Some(value) => value.to_vec(),
                    None => args
                        .next()
                        .ok_or_else(|| {
                            format!("flag needs an argument: -{}", String::from_utf8_lossy(name))
                        })?
                        .as_bytes()
                        .to_vec(),
                };
                match name {
                    b"cwd" => result.cwd = Some(JsString::from_bytes(value)),
                    b"pipe" => result.pipe = String::from_utf8_lossy(&value).into_owned(),
                    _ => result.callbacks = String::from_utf8_lossy(&value).into_owned(),
                }
            }
            other => {
                return Err(format!(
                    "flag provided but not defined: -{}",
                    String::from_utf8_lossy(other)
                ))
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<JsString> {
        text.split_whitespace()
            .map(|arg| JsString::from_bytes(arg.as_bytes()))
            .collect()
    }

    #[test]
    fn parses_the_client_spawn_arguments() {
        let flags = parse(&args(
            "--async --cwd /work --runExternalCode --timing --callbacks=readFile,fileExists",
        ))
        .unwrap();
        assert_eq!(
            flags,
            Flags {
                cwd: Some(JsString::from_bytes(b"/work".as_slice())),
                pipe: String::new(),
                callbacks: "readFile,fileExists".into(),
                r#async: true,
                timing: true,
                run_external_code: true,
            }
        );
        assert_eq!(parse(&args("-pipe /tmp/sock")).unwrap().pipe, "/tmp/sock");
        assert!(!parse(&args("-async=false")).unwrap().r#async);
    }

    #[test]
    fn rejects_what_the_pin_rejects() {
        assert_eq!(
            parse(&args("-cwd")).unwrap_err(),
            "flag needs an argument: -cwd"
        );
        assert_eq!(
            parse(&args("-nope")).unwrap_err(),
            "flag provided but not defined: -nope"
        );
        assert_eq!(
            parse(&args("-timing=maybe")).unwrap_err(),
            "invalid boolean value \"maybe\" for -timing: parse error"
        );
        assert_eq!(parse(&args("-h")).unwrap_err(), "");
    }
}
