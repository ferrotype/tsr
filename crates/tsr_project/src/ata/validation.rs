//! npm name validation operates on bytes, as Go's length and QueryEscape do.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameValidationResult {
    NameOk,
    EmptyName,
    NameTooLong,
    NameStartsWithDot,
    NameStartsWithUnderscore,
    NameContainsNonUriSafeCharacters,
}

// port: tsc/internal/project/ata/validatepackagename.go:ValidatePackageName
pub fn validate_package_name(name: &[u8]) -> (NameValidationResult, &[u8], bool) {
    validate_package_name_worker(name, true)
}

// port: tsc/internal/project/ata/validatepackagename.go:validatePackageNameWorker
fn validate_package_name_worker(name: &[u8], scoped: bool) -> (NameValidationResult, &[u8], bool) {
    use NameValidationResult::{
        EmptyName, NameContainsNonUriSafeCharacters, NameOk, NameStartsWithDot,
        NameStartsWithUnderscore, NameTooLong,
    };
    let result = if name.is_empty() {
        EmptyName
    } else if name.len() > 214 {
        NameTooLong
    } else if name[0] == b'.' {
        NameStartsWithDot
    } else if name[0] == b'_' {
        NameStartsWithUnderscore
    } else {
        if scoped {
            if let Some(tail) = name.strip_prefix(b"@") {
                if let Some(slash) = tail.iter().position(|&b| b == b'/') {
                    let (scope, package) = (&tail[..slash], &tail[slash + 1..]);
                    if !scope.is_empty() && !package.is_empty() && !package.contains(&b'/') {
                        for (part, is_scope) in [(scope, true), (package, false)] {
                            let (result, _, _) = validate_package_name_worker(part, false);
                            if result != NameOk {
                                return (result, part, is_scope);
                            }
                        }
                        return (NameOk, b"", false);
                    }
                }
            }
        }
        if name
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.~".contains(b))
        {
            NameOk
        } else {
            NameContainsNonUriSafeCharacters
        }
    };
    (result, b"", false)
}

// port: tsc/internal/project/ata/validatepackagename.go:renderPackageNameValidationFailure
pub fn render_package_name_validation_failure(
    typing: &[u8],
    result: NameValidationResult,
    name: &[u8],
    is_scope_name: bool,
) -> String {
    let kind = if is_scope_name { "Scope" } else { "Package" };
    // Replacement is confined to human-readable logging, never package identity.
    let name = String::from_utf8_lossy(if name.is_empty() { typing } else { name });
    let typing = String::from_utf8_lossy(typing);
    let reason = match result {
        NameValidationResult::EmptyName => "cannot be empty",
        NameValidationResult::NameTooLong => "should be less than 214 characters",
        NameValidationResult::NameStartsWithDot => "cannot start with '.'",
        NameValidationResult::NameStartsWithUnderscore => "cannot start with '_'",
        NameValidationResult::NameContainsNonUriSafeCharacters => {
            "contains non URI safe characters"
        }
        NameValidationResult::NameOk => panic!("Unexpected Ok result"),
    };
    format!("'{typing}':: {kind} name '{name}' {reason}")
}
