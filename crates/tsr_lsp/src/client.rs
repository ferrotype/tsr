//! Reverse requests must be serviced by the reader independently of the
//! project worker. Implementations retire a pending call when its scope ends.
use tsr_ipc::Context;
use tsr_json::{Decode, Encode, RawValue};
use tsr_lsproto::ResponseError;

pub trait Client: Send + Sync {
    fn notify(&self, method: &str, params: RawValue) -> Result<(), ResponseError>;
    fn request(
        &self,
        context: &Context,
        method: &str,
        params: RawValue,
    ) -> Result<RawValue, ResponseError>;
    /// Progress creation is deliberately asynchronous in the pin.
    fn request_without_waiting(&self, method: &str, params: RawValue) -> Result<(), ResponseError>;
}
pub fn raw(value: &(impl Encode + ?Sized)) -> Result<RawValue, ResponseError> {
    tsr_json::marshal(value, tsr_json::Options::default())
        .map(RawValue)
        .map_err(|error| crate::invalid(&error.to_string()))
}
pub fn notify(
    client: &dyn Client,
    method: &str,
    value: &(impl Encode + ?Sized),
) -> Result<(), ResponseError> {
    client.notify(method, raw(value)?)
}
pub fn request<T: Decode + Default + 'static>(
    client: &dyn Client,
    context: &Context,
    method: &str,
    value: &(impl Encode + ?Sized),
) -> Result<T, ResponseError> {
    let response = client.request(context, method, raw(value)?)?;
    let mut result = T::default();
    tsr_json::unmarshal(&response.0, &mut result, tsr_json::Options::default())
        .map_err(|error| crate::invalid(&error.to_string()))?;
    Ok(result)
}
