use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures_core::Future;
use http::Request;
use tower::{Layer, Service};

#[derive(Clone)]
pub struct ByteBody {
    data: Arc<Vec<u8>>,
}

impl std::fmt::Debug for ByteBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut data = Vec::new();
        data.extend_from_slice(&self.data);
        let s = String::from_utf8(data).expect("found invalid UTF-8");
        f.debug_struct("ByteBody").field("data", &s).finish()
    }
}

impl ByteBody {
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data: Arc::new(data),
        }
    }
}

impl From<Bytes> for ByteBody {
    fn from(value: Bytes) -> Self {
        Self::new(Vec::from(value))
    }
}

impl http_body::Body for ByteBody {
    type Data = Bytes;

    type Error = hyper::Error;

    fn poll_data(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        let bytes = Bytes::copy_from_slice(&self.data);
        Poll::Ready(Some(Ok(bytes)))
    }

    fn poll_trailers(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<Option<http::HeaderMap>, Self::Error>> {
        Poll::Ready(Ok(None))
    }

    fn size_hint(&self) -> http_body::SizeHint {
        let length = self.data.len() as u64;
        http_body::SizeHint::with_exact(length)
    }
}

#[derive(Clone)]
pub struct ReadRequestBody<S> {
    inner: S,
}

impl<S> ReadRequestBody<S> {
    pub fn new(service: S) -> Self {
        Self { inner: service }
    }
}

impl<S> Service<Request<hyper::Body>> for ReadRequestBody<S>
where
    S: Service<Request<ByteBody>> + Clone + Send + 'static,
    S::Future: Send,
{
    type Response = S::Response;

    type Error = S::Error;

    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<hyper::Body>) -> Self::Future {
        let clone = self.inner.clone();
        // take the service that was ready
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            let (parts, b) = req.into_parts();
            let bytes = hyper::body::to_bytes(b).await;
            let bytes = bytes.unwrap();
            let req = Request::from_parts(parts, ByteBody::from(bytes));

            inner.call(req).await
        })
    }
}

/// Enforces a rate limit on the number of requests the underlying
/// service can handle over a period of time.
#[derive(Debug, Clone)]
pub struct ReadRequestLayer;

impl ReadRequestLayer {
    pub fn new() -> Self {
        Self {}
    }
}

impl<S> Layer<S> for ReadRequestLayer {
    type Service = ReadRequestBody<S>;

    fn layer(&self, service: S) -> Self::Service {
        ReadRequestBody::new(service)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;
    use http::Request;
    use httpmock::prelude::*;
    use hyper::Client;
    use hyper_tls::HttpsConnector;
    use serde_json::json;
    use tower::{ServiceBuilder, ServiceExt};

    impl TryFrom<serde_json::Value> for ByteBody {
        type Error = serde_json::Error;

        fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
            let body_bytes = serde_json::to_vec(&value)?;
            Ok(Self::new(body_bytes))
        }
    }

    #[tokio::test]
    async fn test_read_request_body() -> Result<(), Box<dyn Error>> {
        // Arrange
        // let _ = env_logger::try_init();
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.path("/user")
                .header("content-type", "application/json")
                .json_body(json!({
                    "username": "nick",
                }));
            then.status(201);
        });

        // Create a new HTTP client
        let https = HttpsConnector::new();
        let client = Client::builder().build::<_, ByteBody>(https);

        let body = ByteBody::try_from(json!({
            "username": "nick",
        }))?;

        let request = Request::builder()
            .method("POST")
            .uri(&format!("http://{}/user", server.address()))
            .header("content-type", "application/json")
            .body(body)?;

        let response = client.request(request).await?;

        // Assert
        m.assert();
        assert_eq!(response.status(), 201);

        Ok(())
    }

    #[tokio::test]
    async fn test_read_request() -> Result<(), Box<dyn Error>> {
        // Arrange
        // let _ = env_logger::try_init();
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.path("/user")
                .header("content-type", "application/json")
                .json_body(json!({
                    "username": "nick",
                }));
            then.status(201);
        });

        let data = serde_json::to_vec(&json!({
            "username": "nick",
        }))?;
        let body = hyper::Body::from(data);

        let request = Request::builder()
            .method("POST")
            .uri(&format!("http://{}/user", server.address()))
            .header("content-type", "application/json")
            .body(body)?;

        // Create a new HTTP client
        let https_client = Client::builder().build::<_, ByteBody>(HttpsConnector::new());
        let mut client = ServiceBuilder::new()
            .layer(ReadRequestLayer::new())
            .service(https_client);

        let response = client.ready().await?.call(request).await?;

        // Assert
        m.assert();
        assert_eq!(response.status(), 201);

        Ok(())
    }
}
