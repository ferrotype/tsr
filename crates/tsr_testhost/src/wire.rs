//! Serialize opaque JSON without converting its numeric tokens through f64.
use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::value::{to_raw_value, RawValue};

pub(crate) type Json = Box<RawValue>;

pub(crate) fn raw<T: Serialize + ?Sized>(value: &T) -> Json {
    // Callers serialize protocol primitives, already-valid RawValue payloads,
    // and collections of those. None has a fallible custom serializer.
    to_raw_value(value).expect("serializable test-host wire value")
}

pub(crate) fn object(value: &RawValue) -> bool {
    value.get().starts_with('{')
}

pub(crate) fn fields(value: &RawValue) -> serde_json::Result<BTreeMap<String, &RawValue>> {
    serde_json::from_str(value.get())
}

// Serialize each field directly. json! would first convert RawValue to Value,
// rounding opaque numbers before they reach the writer.
macro_rules! wire {
    ({}) => { $crate::wire::raw(&serde_json::json!({})) };
    ({$($key:literal: $value:expr),+ $(,)?}) => {
        $crate::wire::raw(&std::collections::BTreeMap::from([
            $(($key, $crate::wire::raw(&$value))),+
        ]))
    };
}
pub(crate) use wire;
