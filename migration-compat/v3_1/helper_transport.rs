//! Retain complete helper responses while the released Hyper transport owns I/O.
use std::time::Duration;
use zcash_voting::{
    helper::transport::{HelperFuture, HelperTransport, HelperTransportError},
    HyperTransport,
};
pub(super) struct RecordingTransport(HyperTransport);
impl RecordingTransport {
    pub fn new() -> Self {
        Self(HyperTransport::new())
    }
}
impl HelperTransport for RecordingTransport {
    fn get<'a>(&'a self, url: &'a str, timeout: Duration) -> HelperFuture<'a> {
        Box::pin(async move {
            let response = self.0.get(url, timeout).await?;
            super::super::transport::record_helper_response(
                url,
                "GET",
                response.status(),
                response.body(),
            )
            .map_err(|_| HelperTransportError::Response("cannot retain capture evidence".into()))?;
            Ok(response)
        })
    }
    fn post_json<'a>(&'a self, url: &'a str, body: Vec<u8>, timeout: Duration) -> HelperFuture<'a> {
        Box::pin(async move {
            let response = self.0.post_json(url, body, timeout).await?;
            super::super::transport::record_helper_response(
                url,
                "POST",
                response.status(),
                response.body(),
            )
            .map_err(|_| HelperTransportError::Response("cannot retain capture evidence".into()))?;
            Ok(response)
        })
    }
}
