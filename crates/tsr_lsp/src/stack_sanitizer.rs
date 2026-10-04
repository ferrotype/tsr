//! Pinned telemetry sanitization. Unknown frames are always redacted.

// port: tsc/internal/lsp/stack_sanitizer.go:sanitizeStackTrace
pub fn sanitize_stack_trace(stack: &str) -> String {
    let Some(start) = stack.find("runtime/debug.Stack()") else {
        return String::new();
    };
    let mut result = String::new();
    for (index, line) in stack[start..].split_inclusive('\n').enumerate() {
        if index != 0 {
            result.push('\n');
        }
        let indentation = line
            .bytes()
            .take_while(|c| matches!(c, b' ' | b'\t'))
            .count();
        result.push_str(&line[..indentation]);
        if let Some(start) = line[indentation..].find("TypeScript/tsc/") {
            write_sanitized_module_or_path(&line[indentation + start..], &mut result);
        } else {
            result.push_str("(REDACTED FRAME)");
        }
    }
    defeat_generic_secret_regex(&result)
}
// port: tsc/internal/lsp/stack_sanitizer.go:writeSanitizedModuleOrPath
fn write_sanitized_module_or_path(line: &str, result: &mut String) {
    let line = line.trim();
    let end = line
        .find(" +0x")
        .or_else(|| line.rfind(" in goroutine "))
        .unwrap_or(line.len());
    for (index, segment) in line[..end].split('/').enumerate() {
        if index != 0 {
            result.push_str("|>");
        }
        if segment.ends_with(')') {
            if let Some(open) = segment.rfind('(') {
                result.push_str(&segment[..open]);
                result.push_str("()");
            } else {
                result.push_str("???");
            }
        } else {
            result.push_str(segment);
        }
    }
}
// port: tsc/internal/lsp/stack_sanitizer.go:defeatGenericSecretRegex
pub fn defeat_generic_secret_regex(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    for (at, _) in text.char_indices() {
        if at < cursor {
            continue;
        }
        for word in ["key", "token", "signature", "sig", "pwd"] {
            let end = at + word.len();
            if text
                .as_bytes()
                .get(at..end)
                .is_some_and(|part| part.eq_ignore_ascii_case(word.as_bytes()))
                && text
                    .as_bytes()
                    .get(end)
                    .is_some_and(|next| b"([.|".contains(next))
            {
                result.push_str(&text[cursor..end]);
                result.push_str("X_X");
                cursor = end;
                break;
            }
        }
    }
    result.push_str(&text[cursor..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_pinned_sanitizer_baselines() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../upstream/tsc/testdata/baselines/reference/lsp/stackSanitizer");
        for name in [
            "completionsDebugStackTrace",
            "completionsReleaseStackTrace",
            "genericSecretWorkaround",
        ] {
            let text = std::fs::read_to_string(root.join(format!("{name}.md"))).unwrap();
            let blocks: Vec<_> = text.split("````").collect();
            assert_eq!(
                sanitize_stack_trace(blocks[1].trim_matches('\n')),
                blocks[3].trim_matches('\n'),
                "{name}"
            );
        }
    }
    #[test]
    fn unrecognized_stacks_do_not_leak_paths_or_arguments() {
        assert_eq!(sanitize_stack_trace("/Users/private/secret.rs:123"), "");
        assert_eq!(sanitize_stack_trace("runtime/debug.Stack()\n\t/private/sensitive.go\nTypeScript/tsc/internal/lsp.method(0xdeadbeef) +0x42\n"), "(REDACTED FRAME)\n\t(REDACTED FRAME)\nTypeScript|>tsc|>internal|>lsp.method()");
        assert_eq!(
            defeat_generic_secret_regex(
                "getSignature(x) Token.member sig[2] monkey|x pwd( signatureX"
            ),
            "getSignatureX_X(x) TokenX_X.member sigX_X[2] monkeyX_X|x pwdX_X( signatureX"
        );
    }
}
