use http::{Request, Uri};
use http_body::Body;
use std::str::FromStr;
use tower::{Layer, Service};

/// Enforces a rate limit on the number of requests the underlying
/// service can handle over a period of time.
#[derive(Debug, Clone)]
pub struct ForwardRequestLayer {
    uri: Uri,
}

impl ForwardRequestLayer {
    /// Create new rate limit layer.
    pub fn new(uri: Uri) -> Self {
        ForwardRequestLayer { uri }
    }
}

impl<S> Layer<S> for ForwardRequestLayer {
    type Service = ForwardRequest<S>;

    fn layer(&self, service: S) -> Self::Service {
        ForwardRequest::new(service, self.uri.clone())
    }
}

pub struct ForwardRequest<S> {
    uri: Uri,
    inner: S,
}

impl<S> ForwardRequest<S> {
    fn new(inner: S, uri: Uri) -> Self {
        Self { inner, uri }
    }
}

impl<S, B> Service<Request<B>> for ForwardRequest<S>
where
    S: Service<Request<B>>,
    B: Body,
{
    type Response = S::Response;

    type Error = S::Error;

    type Future = S::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        let forward_uri = match req.uri().query() {
            Some(query) => format!("{}{}?{}", self.uri, req.uri().path(), query),
            None => format!("{}{}", self.uri, req.uri().path()),
        };
        let uri = Uri::from_str(forward_uri.as_str()).expect("valid url");
        *req.uri_mut() = uri;
        self.inner.call(req)
    }
}

impl<S: Clone> Clone for ForwardRequest<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            uri: self.uri.clone(),
        }
    }
}
