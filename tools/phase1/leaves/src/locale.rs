//! Request wiring to the production locale and localization contracts.
use crate::api::{self, Outcome};
use serde_json::{json, Value};
use tsr_locale::{Locale, LocaleContext, DEFAULT};

pub fn observe(request: &Value) -> Option<Outcome> {
    if !api::subject(request).starts_with("locale.") {
        return None;
    }
    let mut context = LocaleContext::default();
    let mut rows = Vec::new();
    for action in api::actions(request) {
        let op = api::action_op(action);
        let tag = api::action_str(action, "tag");
        let row = match op {
            "parse" => {
                let (locale, error) = Locale::parse_detailed(tag);
                json!({"op":op,"input":tag,"ok":error.is_none(),"locale_string":locale.to_string(),"tag_string":locale.tag_string(),"is_default":locale.is_default(),"parse_error":error.as_ref().map(ToString::to_string).unwrap_or_default(),"error_subtag":error.as_ref().map_or("",tsr_locale::ParseError::subtag)})
            }
            "ctx_root" => {
                context = LocaleContext::default();
                json!({"op":op})
            }
            "ctx_with" => {
                let (locale, ok) = Locale::parse(tag);
                let row = json!({"op":op,"input":tag,"parse_ok":ok,"stored_locale_string":locale.to_string(),"stored_tag_string":locale.tag_string()});
                context = context.with_locale(locale);
                row
            }
            "ctx_with_default" => {
                context = context.with_locale(DEFAULT.clone());
                json!({"op":op})
            }
            "ctx_read" => {
                let locale = context.locale();
                json!({"op":op,"has_locale":context.has_locale(),"locale_string":locale.to_string(),"tag_string":locale.tag_string(),"is_default":locale.is_default()})
            }
            "default_read" => {
                json!({"op":op,"locale_string":DEFAULT.to_string(),"tag_string":DEFAULT.tag_string(),"is_zero_tag":DEFAULT==Locale::default()})
            }
            "localize" | "localize_message" => {
                let (locale, ok) = Locale::parse(tag);
                let key = api::action_str(action, "key");
                let args: Vec<_> = action["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|arg| arg.as_str().unwrap().as_bytes())
                    .collect();
                let result = std::panic::catch_unwind(|| {
                    if op == "localize" {
                        tsr_diagnostics::localize(&locale, None, key.as_bytes(), &args)
                    } else {
                        tsr_diagnostics::by_key(key)
                            .expect("unknown message fixture")
                            .localize(
                                &locale,
                                &args
                                    .iter()
                                    .map(|bytes| tsr_diagnostics::Argument::Bytes(bytes.to_vec()))
                                    .collect::<Vec<_>>(),
                            )
                    }
                });
                let (text, panic) = match result {
                    Ok(bytes) => (String::from_utf8(bytes).unwrap(), String::new()),
                    Err(error) => (
                        String::new(),
                        format!("string:{}", super::diagnostics::panic_text(error.as_ref())),
                    ),
                };
                let mut row = json!({"op":op,"input":tag,"parse_ok":ok,"locale_string":locale.to_string(),"key":key,"text":text,"panic":panic});
                if op == "localize_message" {
                    row["code"] = json!(tsr_diagnostics::by_key(key).unwrap().code);
                }
                row
            }
            _ => return Some(Outcome::Failed(format!("unknown locale action {op}"))),
        };
        rows.push(row);
    }
    Some(Outcome::Observed(json!({"ordered":rows})))
}
