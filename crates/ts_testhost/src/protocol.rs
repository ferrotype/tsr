use std::{collections::BTreeMap, io};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::wire::{self, wire, Json};

/// LSP request identities. String and integer namespaces remain distinct.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum Id {
    Integer(i32),
    String(String),
}

pub(crate) type RequestResult = Result<Vec<Json>, (i64, String)>;
pub(crate) fn decode<T: DeserializeOwned>(value: &RawValue) -> Result<T, (i64, String)> {
    serde_json::from_str(value.get()).map_err(|error| (-32602, error.to_string()))
}
pub(crate) fn empty(value: &RawValue) -> Result<(), (i64, String)> {
    if wire::fields(value).is_ok_and(|fields| fields.is_empty()) {
        Ok(())
    } else {
        Err((-32602, "expected empty params object".into()))
    }
}
pub(crate) fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
pub(crate) fn fields(object: &BTreeMap<String, &RawValue>, allowed: &[&str]) -> io::Result<()> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid("unknown envelope field"));
    }
    Ok(())
}
pub(crate) fn notify(method: &str, params: &RawValue) -> Json {
    wire!({"jsonrpc":"2.0","method":method,"params":params})
}
pub(crate) fn progress(id: &Id, callback: &str, phase: &str) -> Json {
    notify(
        "testhost/progress",
        &wire!({"id":id,"callback":callback,"phase":phase}),
    )
}
pub(crate) fn success(id: &Id, result: &RawValue) -> Json {
    wire!({"jsonrpc":"2.0","id":id,"result":result})
}
pub(crate) fn failure(id: &Id, code: i64, message: &str, data: Option<Json>) -> Json {
    let message = if message.len() > 4096 {
        "invalid oversized protocol payload"
    } else {
        message
    };
    let error = if let Some(data) = data {
        wire!({"code":code,"message":message,"data":data})
    } else {
        wire!({"code":code,"message":message})
    };
    let response = wire!({"jsonrpc":"2.0","id":id,"error":error});
    if fits(&response) {
        response
    } else {
        // Admission reserves this minimal error even for near-limit string IDs.
        wire!({"jsonrpc":"2.0","id":id,"error":wire!({"code":code,"message":""})})
    }
}
pub(crate) fn fits(value: &RawValue) -> bool {
    value.get().len() <= crate::framing::MAX_BODY
}
pub(crate) fn response_fits(id: &Id, value: &RawValue) -> Result<(), String> {
    if fits(&success(id, value)) {
        Ok(())
    } else {
        Err("response exceeds frame limit".into())
    }
}
