//! The pinned Go Unicode categories shared by regexp matching and language-service text.

pub fn property_ranges(name: &str) -> Option<&'static [(u32, u32)]> {
    let properties = super::go_unicode_generated::PROPERTIES;
    let index = properties
        .binary_search_by_key(&name, |&(key, _)| key)
        .ok()?;
    Some(properties[index].1)
}

/// Go's `unicode.IsDigit` is the decimal-digit category, not every numeric rune.
pub fn is_digit(ch: char) -> bool {
    let ranges = property_ranges("Nd").expect("Go Unicode defines the Nd category");
    let rune = u32::from(ch);
    let index = ranges.partition_point(|&(lo, _)| lo <= rune);
    index != 0 && rune <= ranges[index - 1].1
}

#[cfg(test)]
mod tests {
    #[test]
    fn decimal_digits_exclude_other_numeric_categories() {
        for ch in ['0', '9', '١', '𝟛', '\u{11de0}'] {
            assert!(super::is_digit(ch));
        }
        for ch in ['²', '½', 'Ⅳ', 'a'] {
            assert!(!super::is_digit(ch));
        }
    }
}
