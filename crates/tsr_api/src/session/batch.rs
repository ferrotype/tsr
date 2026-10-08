//! `batchRequests`: each item is dispatched as its own request with panic
//! recovery, a nested batch is refused, and the encoded responses are paged
//! by bytes with the pin's accounting: the page's JSON length including the
//! continuation token must fit `maxResponseBytesPerPage`, except that a
//! single oversized response is always sent.
//! port: tsc/internal/api/session.go
use super::{ApiSession, SessionError, SessionResult};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use tsr_ipc::{Context, Response};
use tsr_json::{Decode, Decoder, Encode, Encoder, RawValue};

/// The pin's `DefaultMaxResponseBytesPerPage` (session.go).
pub const DEFAULT_MAX_RESPONSE_BYTES_PER_PAGE: usize = 300_000_000;

/// The pin's `BatchRequest` with the method kept as text: an unknown method
/// is answered inside its item, not refused for the whole batch.
#[derive(Default)]
struct BatchRequest {
    method: String,
    params: RawValue,
}
impl Decode for BatchRequest {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), tsr_json::Error> {
        input.object(|name, input| match name {
            b"method" => input.value(&mut self.method),
            b"params" => input.value(&mut self.params),
            _ => input.skip_value(),
        })
    }
}

/// The pin's `BatchRequestsParams` (proto.go).
#[derive(Default)]
struct BatchRequestsParams {
    requests: Vec<BatchRequest>,
    continuation_token: String,
    max_response_bytes_per_page: i64,
}
impl Decode for BatchRequestsParams {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), tsr_json::Error> {
        input.object(|name, input| match name {
            b"requests" => input.value(&mut self.requests),
            b"continuationToken" => input.value(&mut self.continuation_token),
            b"maxResponseBytesPerPage" => input.value(&mut self.max_response_bytes_per_page),
            _ => input.skip_value(),
        })
    }
}

/// One item's answer: the result, or the error text.
/// The pin's `BatchResponse` (proto.go).
struct BatchItem {
    method: String,
    result: Option<RawValue>,
    error: String,
}
impl Encode for BatchItem {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), tsr_json::Error> {
        tsr_jsonrpc::object(
            out,
            &[
                tsr_jsonrpc::field(b"method", &self.method),
                tsr_jsonrpc::field(b"result", &self.result),
                tsr_jsonrpc::omitted(
                    b"error",
                    (!self.error.is_empty()).then_some(&self.error as &dyn Encode),
                ),
            ],
        )
    }
}

/// A page of pre-encoded responses, written verbatim.
/// port: tsc/internal/api/proto.go:BatchRequestsResponse.MarshalJSONTo
struct BatchPage {
    responses: Vec<RawValue>,
    continuation_token: String,
}
impl Encode for BatchPage {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), tsr_json::Error> {
        tsr_jsonrpc::object(
            out,
            &[
                tsr_jsonrpc::field(b"responses", &self.responses),
                tsr_jsonrpc::omitted(
                    b"continuationToken",
                    (!self.continuation_token.is_empty())
                        .then_some(&self.continuation_token as &dyn Encode),
                ),
            ],
        )
    }
}

impl ApiSession {
    /// port: tsc/internal/api/session.go:Session.handleBatchRequests
    pub(super) fn handle_batch_requests(
        &self,
        ctx: &Context,
        payload: &[u8],
    ) -> SessionResult<Response> {
        let mut params = BatchRequestsParams::default();
        tsr_json::unmarshal(payload, &mut params, tsr_json::Options::default())
            .map_err(|error| SessionError::InvalidRequest(format!("{error}")))?;
        if !params.continuation_token.is_empty() {
            let remaining = self
                .batch_pages
                .lock()
                .expect("batch pages")
                .remove(&params.continuation_token)
                .ok_or_else(|| SessionError::Client("invalid batch continuation token".into()))?;
            return Ok(self.paginate(remaining, params.max_response_bytes_per_page));
        }
        let mut encoded = Vec::with_capacity(params.requests.len());
        for request in &params.requests {
            let item = self.handle_batch_request(ctx, request);
            let bytes = tsr_json::marshal(&item, tsr_json::Options::default())
                .map_err(|error| SessionError::Other(format!("{error}")))?;
            encoded.push(RawValue(bytes));
        }
        Ok(self.paginate(encoded, params.max_response_bytes_per_page))
    }

    /// port: tsc/internal/api/session.go:Session.paginateBatchResponses
    fn paginate(&self, encoded: Vec<RawValue>, max_response_bytes_per_page: i64) -> Response {
        let max = usize::try_from(max_response_bytes_per_page)
            .ok()
            .filter(|max| *max > 0)
            .unwrap_or(DEFAULT_MAX_RESPONSE_BYTES_PER_PAGE);
        let mut encoded_length = r#"{"responses":[]}"#.len();
        let mut page_length = 0usize;
        for response in &encoded {
            let mut additional = response.0.len();
            if page_length > 0 {
                additional += 1;
            }
            if page_length > 0 && encoded_length + additional > max {
                break;
            }
            encoded_length += additional;
            page_length += 1;
        }
        if page_length == encoded.len() {
            return Response::json(BatchPage {
                responses: encoded,
                continuation_token: String::new(),
            });
        }
        let continuation_token = format!(
            "{}-{}",
            self.id,
            self.next_batch_page.fetch_add(1, Ordering::Relaxed) + 1
        );
        let continuation_length = r#","continuationToken":"""#.len() + continuation_token.len();
        while page_length > 1 && encoded_length + continuation_length > max {
            encoded_length -= encoded[page_length - 1].0.len() + 1;
            page_length -= 1;
        }
        let mut current = encoded;
        let remaining = current.split_off(page_length);
        self.batch_pages
            .lock()
            .expect("batch pages")
            .insert(continuation_token.clone(), remaining);
        Response::json(BatchPage {
            responses: current,
            continuation_token,
        })
    }

    /// port: tsc/internal/api/session.go:Session.handleBatchRequest
    fn handle_batch_request(&self, ctx: &Context, request: &BatchRequest) -> BatchItem {
        let mut item = BatchItem {
            method: request.method.clone(),
            result: None,
            error: String::new(),
        };
        if request.method == "batchRequests" {
            item.error = "api: invalid request: batchRequests cannot be nested".into();
            return item;
        }
        match catch_unwind(AssertUnwindSafe(|| {
            self.dispatch(ctx, &request.method, &request.params.0)
        })) {
            Ok(Ok(Some(Response::Json(value)))) => {
                match tsr_json::marshal(value.as_ref(), tsr_json::Options::default()) {
                    Ok(bytes) => item.result = Some(RawValue(bytes)),
                    Err(error) => item.error = format!("{error}"),
                }
            }
            Ok(Ok(Some(Response::Binary(bytes)))) => {
                // A binary result inside a batch travels as base64 text, as the
                // pin's []byte marshals.
                let encoded = super::responses::base64_standard(&bytes);
                item.result = tsr_json::marshal(&encoded, tsr_json::Options::default())
                    .ok()
                    .map(RawValue);
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => item.error = error.to_string(),
            Err(panic) => {
                item.error = format!("panic: {}", tsr_ipc::panic_message(panic.as_ref()));
            }
        }
        item
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ports `TestBatchResponseEncodesEmptyResult`.
    #[test]
    fn empty_results_encode_as_empty_arrays() {
        let item = BatchItem {
            method: "getSignaturesOfType".into(),
            result: Some(RawValue(b"[]".to_vec())),
            error: String::new(),
        };
        let encoded = tsr_json::marshal(&item, tsr_json::Options::default()).unwrap();
        assert_eq!(encoded, br#"{"method":"getSignaturesOfType","result":[]}"#);
    }
}
