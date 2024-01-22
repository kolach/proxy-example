use http::{
    header::{AsHeaderName, IntoHeaderName},
    Request,
};
use tower::{Layer, Service};

/// Enforces a rate limit on the number of requests the underlying
/// service can handle over a period of time.
#[derive(Debug, Clone)]
pub struct RenameHeaderLayer<F, T> {
    from: F,
    to: T,
}

impl<F, T> RenameHeaderLayer<F, T> {
    /// Create new rate limit layer.
    pub fn new(from: F, to: T) -> Self {
        RenameHeaderLayer { from, to }
    }
}

impl<F, T, S> Layer<S> for RenameHeaderLayer<F, T>
where
    F: Clone,
    T: Clone,
{
    type Service = RenameHeader<S, F, T>;

    fn layer(&self, service: S) -> Self::Service {
        RenameHeader::new(service, self.from.clone(), self.to.clone())
    }
}

#[derive(Clone)]
pub struct RenameHeader<S, F, T> {
    inner: S,
    from: F,
    to: T,
}

impl<S, F, T> RenameHeader<S, F, T> {
    fn new(inner: S, from: F, to: T) -> Self {
        Self { inner, from, to }
    }
}

impl<S, B, F, T> Service<Request<B>> for RenameHeader<S, F, T>
where
    S: Service<Request<B>>,
    F: AsHeaderName + Clone,
    T: IntoHeaderName + Clone,
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
        let value = req.headers_mut().remove(self.from.clone());
        if let Some(value) = value {
            req.headers_mut().insert(self.to.clone(), value);
        }
        self.inner.call(req)
    }
}
