use std::{
    sync::{Arc, RwLock},
    task::{Context, Poll},
};

use futures_core::{ready, Future};
use http::{header::AUTHORIZATION, HeaderValue, Request, Response, StatusCode};
use pin_project_lite::pin_project;
use tower::{Layer, Service};

#[derive(Clone)]
pub struct KeyPool {
    data: Arc<RwLock<(Vec<String>, usize)>>,
}

impl From<Vec<&str>> for KeyPool {
    fn from(value: Vec<&str>) -> Self {
        let keys = value.iter().map(|s| s.to_string()).collect();
        KeyPool::new(keys)
    }
}

impl KeyPool {
    pub fn new(keys: Vec<String>) -> KeyPool {
        KeyPool {
            data: Arc::new(RwLock::new((keys, 0))),
        }
    }

    pub fn active_key(&self) -> Option<String> {
        let data = self.data.read().unwrap();
        let cursor = data.1;
        data.0.get(cursor).cloned()
    }

    pub fn remove_active_key(&self) -> Option<String> {
        let mut data = self.data.write().unwrap();
        if data.0.is_empty() {
            None
        } else {
            let cursor = data.1;
            let key = data.0.remove(cursor);
            tracing::log::warn!("active key removed: {}", key);
            if data.1 >= data.0.len() {
                data.1 = 0;
            }
            Some(key)
        }
    }

    pub fn shift_active_key(&self) {
        let mut data = self.data.write().unwrap();
        let len = data.0.len();
        if len > 1 {
            let current_cursor = data.1;
            data.1 = (current_cursor + 1) % len;
            tracing::log::warn!("active key shifted");
        }
    }

    pub fn shift_active_key_if_equal(&self, key: Option<String>) {
        if self.active_key() == key {
            self.shift_active_key();
        }
    }

    pub fn remove_active_key_if_equal(&self, key: Option<String>) {
        if self.active_key() == key {
            self.remove_active_key();
        }
    }
}

pin_project! {
    pub struct ResponseFuture<F> {
        keys: KeyPool,
        cur_key: Option<String>,
        #[pin]
        fut: F,
    }
}

impl<F> ResponseFuture<F> {
    fn new(fut: F, keys: KeyPool, cur_key: Option<String>) -> Self {
        Self { fut, keys, cur_key }
    }
}

impl<F, ResBody, Error> Future for ResponseFuture<F>
where
    F: Future<Output = Result<Response<ResBody>, Error>>,
{
    type Output = Result<Response<ResBody>, Error>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let result = ready!(this.fut.poll(cx));

        if let Ok(response) = &result {
            let cur_key = this.cur_key.clone();
            match response.status() {
                StatusCode::UNAUTHORIZED => {
                    this.keys.remove_active_key_if_equal(cur_key);
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    this.keys.shift_active_key_if_equal(cur_key);
                }
                _ => (),
            }
        }

        Poll::Ready(result)
    }
}

#[derive(Clone)]
pub struct Authorize<S> {
    keys: KeyPool,
    inner: S,
}

impl<S> Authorize<S> {
    fn new(inner: S, keys: KeyPool) -> Self {
        Self { inner, keys }
    }

    fn extract_api_key<B>(&self, request: &Request<B>) -> Option<String> {
        let auth_header = request.headers().get(AUTHORIZATION.to_string())?;
        let parts: Vec<&str> = auth_header.to_str().ok()?.splitn(2, ' ').collect();
        if parts.len() != 2 {
            return None;
        }
        let api_key = parts[1].to_string();
        Some(api_key)
    }
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for Authorize<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = S::Response;

    type Error = S::Error;

    type Future = ResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        // add authorization Bearer if missing
        let mut api_key = self.extract_api_key(&req);
        if api_key.is_none() {
            api_key = self.keys.active_key();
            if let Some(api_key) = api_key.clone() {
                let header_value = HeaderValue::from_str(&format!("Bearer {}", api_key)).unwrap();
                req.headers_mut().insert(AUTHORIZATION, header_value);
            }
        }

        let fut = self.inner.call(req);
        ResponseFuture::new(fut, self.keys.clone(), api_key)
    }
}

#[derive(Clone)]
pub struct AuthLayer {
    keys: KeyPool,
}

impl AuthLayer {
    /// Create new rate limit layer.
    pub fn new(keys: KeyPool) -> Self {
        AuthLayer { keys }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = Authorize<S>;

    fn layer(&self, service: S) -> Self::Service {
        Authorize::new(service, self.keys.clone())
    }
}
