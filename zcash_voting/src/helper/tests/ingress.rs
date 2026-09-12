//! A host route exercising real HyperTransport header forwarding and classification.
use crate::transport::{RouteError, RouteFuture, RouteHttp, RouteRequest, RouteResponse};
use std::{collections::VecDeque, sync::Mutex};

pub(crate) enum Reply {
    Receipt,
    Accepted,
    Lost,
    Mismatched,
    UnknownVersion,
    ExtraField,
    MissingToken,
    WrongContentType,
    Oversized,
    Truncated,
    GenericTimeout,
}

pub(crate) struct IngressRoute {
    replies: Mutex<VecDeque<Reply>>,
    pub tokens: Mutex<Vec<String>>,
}

impl IngressRoute {
    pub fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            tokens: Mutex::new(vec![]),
        }
    }
}

impl RouteHttp for IngressRoute {
    fn execute<'a>(
        &'a self,
        request: RouteRequest<'a>,
        on_dispatch: &'a (dyn Fn() + Send + Sync),
    ) -> RouteFuture<'a> {
        Box::pin(async move {
            on_dispatch();
            if request.method == http::Method::GET {
                return Ok(RouteResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: br#"{"status":"pending"}"#.to_vec(),
                });
            }
            let token = request
                .headers
                .iter()
                .find(|(name, _)| name == crate::ingress_timeout::REQUEST_HEADER)
                .expect("attempt header forwarded")
                .1
                .clone();
            self.tokens.lock().unwrap().push(token.clone());
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected POST");
            let mut receipt = serde_json::json!({"error":{"version":1,"code":"request_body_timeout","dispatch":"not_started","attempt":token}});
            let mut content_type = "application/json";
            match reply {
                Reply::Lost => return Err(RouteError::after_dispatch("response lost")),
                Reply::Accepted => {
                    return Ok(RouteResponse {
                        status: 200,
                        headers: vec![("content-type".into(), content_type.into())],
                        body: br#"{"status":"queued"}"#.to_vec(),
                    })
                }
                Reply::Mismatched => receipt["error"]["attempt"] = "wrong-attempt".into(),
                Reply::UnknownVersion => receipt["error"]["version"] = 2.into(),
                Reply::ExtraField => receipt["error"]["extra"] = true.into(),
                Reply::MissingToken => {
                    receipt["error"].as_object_mut().unwrap().remove("attempt");
                }
                Reply::WrongContentType => content_type = "text/plain",
                _ => {}
            }
            let body = match reply {
                Reply::Oversized => {
                    vec![b' '; crate::helper::transport::MAX_HELPER_RESPONSE_BYTES + 1]
                }
                Reply::Truncated => br#"{"error":"#.to_vec(),
                Reply::GenericTimeout => br#"{"error":"timeout"}"#.to_vec(),
                _ => serde_json::to_vec(&receipt).unwrap(),
            };
            Ok(RouteResponse {
                status: 408,
                headers: vec![("content-type".into(), content_type.into())],
                body,
            })
        })
    }
}
