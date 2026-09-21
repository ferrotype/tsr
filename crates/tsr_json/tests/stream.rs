use std::{
    borrow::Cow,
    io::{self, Write},
};
use tsr_json::{Decode, Decoder, Encode, Encoder, Error, Kind, Options, RawValue, Token};

#[test]
fn tokens_support_heterogeneous_members_and_exact_unsigned_values() {
    struct Record;
    impl Encode for Record {
        fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
            out.write_token(Token::BeginObject)?;
            out.string(b"name")?;
            out.value("worker")?;
            out.string(b"counter")?;
            out.value(&u64::MAX)?;
            out.string(b"active")?;
            out.value(&true)?;
            out.write_token(Token::EndObject)
        }
    }
    let bytes = tsr_json::marshal(&Record, Options::default()).unwrap();
    assert_eq!(
        bytes,
        br#"{"name":"worker","counter":18446744073709551615,"active":true}"#
    );
    let mut number = 0u64;
    tsr_json::unmarshal(b"18446744073709551615", &mut number, Options::default()).unwrap();
    assert_eq!(number, u64::MAX);
}

#[test]
fn failed_tokens_preserve_state_and_duplicate_names_survive_flush() {
    let mut out = Encoder::new(Options::default()).unwrap();
    out.write_token(Token::BeginObject).unwrap();
    out.string(b"key").unwrap();
    out.string(&vec![b'a'; 8192]).unwrap();
    let before = (out.output_offset(), out.stack_pointer(), out.into_bytes());
    let mut out = Encoder::new(Options::default()).unwrap();
    out.write_token(Token::BeginObject).unwrap();
    out.string(b"key").unwrap();
    out.string(&vec![b'a'; 8192]).unwrap();
    assert!(out
        .write_value(br#""\u006bey""#)
        .unwrap_err()
        .is_duplicate_name());
    assert_eq!(
        (out.output_offset(), out.stack_pointer()),
        (before.0, before.1)
    );
    assert_eq!(out.buffered_bytes(), b"");
    out.string(b"next").unwrap();
    assert!(out
        .write_token(Token::Number(Cow::Borrowed(b"1e")))
        .unwrap_err()
        .is_unexpected_eof());
    out.uint(2).unwrap();
    out.write_token(Token::EndObject).unwrap();
    assert!(out.into_bytes().ends_with(b",\"next\":2}\n"));
}

#[test]
fn raw_depth_is_checked_before_any_output_is_changed() {
    let mut out = Encoder::new(Options::default()).unwrap();
    for _ in 0..9999 {
        out.write_token(Token::BeginArray).unwrap();
    }
    let offset = out.output_offset();
    assert!(
        matches!(out.write_value(b"[[null]]"), Err(Error::Syntax(s)) if s.message == "exceeded max depth")
    );
    assert_eq!(out.output_offset(), offset);
    assert_eq!(out.stack_depth(), 9999);
    out.write_value(b"[null]").unwrap();
    for _ in 0..9999 {
        out.write_token(Token::EndArray).unwrap();
    }
}

#[test]
fn raw_decoder_error_retains_the_previous_position_and_reports_the_outer_context() {
    let mut input = Decoder::from_slice(br#"{"a":1,"b":[0,"#);
    input.read_token().unwrap();
    input.read_value().unwrap();
    input.read_value().unwrap();
    input.read_value().unwrap();
    let before = input.input_offset();
    let error = input.read_value().unwrap_err();
    assert_eq!(input.input_offset(), before);
    assert!(
        matches!(error, Error::Syntax(s) if s.pointer == "/b/1" && s.offset == 14 && s.unexpected_eof)
    );
}

#[test]
fn typed_decoding_preserves_successful_fields_and_allocated_pointer_on_failure() {
    #[derive(Default)]
    struct Record {
        first: String,
        count: Option<i64>,
    }
    impl Decode for Record {
        fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
            input.object(|key, input| match key {
                b"first" => input.value(&mut self.first),
                b"count" => input.value(&mut self.count),
                _ => input.skip_value(),
            })
        }
    }
    let mut record = Record::default();
    let e = tsr_json::unmarshal(
        br#"{"first":"retained","count":false}"#,
        &mut record,
        Options::default(),
    )
    .unwrap_err();
    assert_eq!(record.first, "retained");
    assert_eq!(record.count, Some(0));
    assert!(matches!(e, Error::Semantic(s) if s.pointer == "/count" && s.kind == Kind::False));
}

#[test]
fn custom_codecs_must_read_or_write_exactly_one_value() {
    struct Bad(bool);
    impl Encode for Bad {
        fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
            if self.0 {
                out.null()?;
                out.null()?;
            }
            Ok(())
        }
    }
    impl Decode for Bad {
        fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
            if self.0 {
                input.read_token()?;
                input.read_token()?;
            }
            Ok(())
        }
    }
    for count in [false, true] {
        assert!(tsr_json::marshal(&Bad(count), Options::default())
            .unwrap_err()
            .to_string()
            .contains("exactly one value"));
        assert!(
            tsr_json::unmarshal(b"null null", &mut Bad(count), Options::default())
                .unwrap_err()
                .to_string()
                .contains("exactly one value")
        );
    }
}

#[test]
fn writer_retry_does_not_repeat_the_accepted_prefix() {
    struct Flaky {
        bytes: Vec<u8>,
        calls: usize,
    }
    impl Write for Flaky {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls == 2 {
                return Err(io::Error::other("injected"));
            }
            let size = bytes.len().min(2);
            self.bytes.extend_from_slice(&bytes[..size]);
            Ok(size)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut sink = Flaky {
        bytes: vec![],
        calls: 0,
    };
    let mut out = Encoder::with_writer(&mut sink, Options::default()).unwrap();
    assert!(out.string(b"abc").is_err());
    assert_eq!(out.output_offset(), 6);
    out.flush().unwrap();
    out.uint(42).unwrap();
    drop(out);
    assert_eq!(sink.bytes, b"\"abc\"\n42\n");
}

#[test]
fn call_options_are_temporary_and_namespace_changes_are_rejected_before_writing() {
    let mut out = Encoder::new(Options::default()).unwrap();
    tsr_json::marshal_encode(
        &mut out,
        &RawValue(b"{\"x\":1,\"x\":2}".to_vec()),
        Options {
            allow_duplicate_names: Some(true),
            ..Options::default()
        },
    )
    .unwrap();
    out.write_token(Token::BeginObject).unwrap();
    assert!(
        tsr_json::marshal_encode(&mut out, "key", Options::default())
            .unwrap_err()
            .to_string()
            .contains("cannot change UTF-8 checks")
    );
    assert_eq!(out.stack_depth(), 1);
    out.string(b"key").unwrap();
    // A value within an object may use the wrapper's repairing policy.
    tsr_json::marshal_encode(
        &mut out,
        &tsr_jsstring::JsString::from_bytes([0xff]),
        Options::default(),
    )
    .unwrap();
    assert!(out.string(b"key").unwrap_err().is_duplicate_name());
}

#[test]
fn failed_marshal_retains_partial_output_and_nested_error_location() {
    let (bytes, result) = tsr_json::marshal_partial(&vec![1.0, f64::INFINITY], Options::default());
    assert_eq!(bytes, b"[1");
    let Error::Semantic(error) = result.unwrap_err() else {
        panic!("semantic failure")
    };
    assert_eq!(error.pointer, "/1");
    // Native Marshal reports the next value position, including its comma.
    assert_eq!(error.offset, 3);
}

#[test]
fn decoder_call_options_restore_after_success_and_failure() {
    let mut decoder = Decoder::from_slice(b"{\"x\":1,\"x\":2} {\"x\":3,\"x\":4}");
    let mut raw = RawValue::default();
    tsr_json::unmarshal_decode(
        &mut decoder,
        &mut raw,
        Options {
            allow_duplicate_names: Some(true),
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(raw.0, br#"{"x":1,"x":2}"#);
    assert!(
        tsr_json::unmarshal_decode(&mut decoder, &mut raw, Options::default())
            .unwrap_err()
            .is_duplicate_name()
    );
    let mut decoder = Decoder::from_slice(b"{\"x\":1}");
    decoder.read_token().unwrap();
    let before = decoder.input_offset();
    assert!(tsr_json::unmarshal_decode(
        &mut decoder,
        &mut raw,
        Options {
            allow_duplicate_names: Some(true),
            ..Options::default()
        }
    )
    .is_err());
    assert_eq!(decoder.input_offset(), before);
    assert_eq!(
        decoder.read_token().unwrap(),
        Token::String("x".as_bytes().into())
    );
}
