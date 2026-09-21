//! Behaviours of the pinned grammar that a correct-looking port contradicts.
use tsr_glob::{match_elements, parse, read_range_rune, split, Element, Error, Glob};

#[test]
fn a_negated_range_stores_its_flag_and_ignores_it() {
    let glob = Glob::parse(b"[!a-z]").unwrap();
    assert_eq!(
        glob.elements,
        [Element::CharRange { negate: true, low: 97, high: 122 }]
    );
    assert_eq!(glob.to_bytes(), b"[a-z]");
    assert!(glob.matches(b"m"));
    assert!(!glob.matches(b"M"));
}

#[test]
fn a_star_cannot_meet_the_separator_that_follows_it() {
    assert!(!Glob::parse(b"*/a").unwrap().matches(b"x/a"));
    assert!(Glob::parse(b"a/*").unwrap().matches(b"a/x"));
    assert!(Glob::parse(b"**/x").unwrap().matches(b"a/b/x"));
}

#[test]
fn separator_runs_render_once_and_consume_a_whole_run() {
    let glob = Glob::parse(b"a//b").unwrap();
    assert_eq!(glob.elements.len(), 4);
    assert_eq!(glob.to_bytes(), b"a//b");
    assert!(Glob::parse(b"a/b").unwrap().matches(b"a///b"));
}

#[test]
#[should_panic(expected = "index out of bounds")]
fn a_separator_run_reaching_the_end_panics_like_the_pin() {
    Glob::parse(b"a/").unwrap().matches(b"a/");
}

#[test]
fn groups_try_each_alternative_with_the_tail_appended() {
    let glob = Glob::parse(b"{a,b,}x").unwrap();
    assert_eq!(glob.to_bytes(), b"{a,b,}x");
    assert!(glob.matches(b"ax") && glob.matches(b"bx") && glob.matches(b"x"));
    assert!(!glob.matches(b"cx"));
    assert_eq!(Glob::parse(b"{a"), Err(Error::UnmatchedBrace));
}

#[test]
fn parser_errors_carry_the_pinned_text() {
    assert_eq!(Glob::parse(b"a**").unwrap_err().to_string(), "** may only be adjacent to '/'");
    assert_eq!(Glob::parse(b"[a").unwrap_err().to_string(), "'[' patterns must be of the form [x-y]");
    assert_eq!(Glob::parse(b"[\xff-a]").unwrap_err(), Error::InvalidUtf8);
    assert!(Glob::parse("[\u{fffd}-a]".as_bytes()).is_ok());
    assert_eq!(read_range_rune(b""), Err(Error::BadRange));
}

#[test]
fn nested_parsing_hands_back_the_residual_and_bytes_match_bytewise() {
    let (glob, residual) = parse(b"a,b}", true).unwrap();
    assert_eq!((glob.to_bytes().as_slice(), residual), (b"a".as_slice(), b",b}".as_slice()));
    assert_eq!(split(b"a//b/c"), (b"a".as_slice(), b"b/c".as_slice()));
    assert_eq!(split(b"a//"), (b"a".as_slice(), b"".as_slice()));
    assert!(match_elements(&[Element::Literal(vec![0xff])], &[0xff]));
}
