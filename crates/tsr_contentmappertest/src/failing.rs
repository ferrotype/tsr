//! A mapper whose every transform fails.
use crate::{initialize_result, unexpected_method, MapperHandler};
use tsr_contentmapper::{METHOD_INITIALIZE, METHOD_TRANSFORM};
use tsr_ipc::{Context, HandlerResult};

pub struct FailingHandler;

impl MapperHandler for FailingHandler {
    /// port: tsc/internal/testutil/contentmappertest/failing.go:failingHandler.HandleRequest
    fn handle_request(&self, _: &Context, method: &str, _: &[u8]) -> HandlerResult {
        match method {
            METHOD_INITIALIZE => Ok(Some(tsr_ipc::Response::json(initialize_result("mapper")))),
            METHOD_TRANSFORM => Err("content mapper failed to transform the file".into()),
            _ => Err(unexpected_method(method)),
        }
    }
}
