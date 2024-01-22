use core::time;
use std::pin::Pin;
use std::time::Duration;

use futures_core::Future;
use http::{Request, Response};
use tower::retry::Policy;

use crate::rng::{HasherRng, Rng};

pub trait Backoff {
    type Future: Future<Output = Self> + Send;

    fn next(&self) -> Self::Future;
}

#[derive(Clone)]
pub struct LinearBackoff {
    timeout: Duration,
}

impl LinearBackoff {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl Backoff for LinearBackoff {
    type Future = Pin<Box<dyn Future<Output = Self> + Send>>;

    fn next(&self) -> Self::Future {
        let this = self.clone();
        let fut = async {
            tokio::time::sleep(this.timeout).await;
            this
        };
        Box::pin(fut)
    }
}

#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    /// The minimum amount of time to wait before resuming an operation.
    min: time::Duration,
    /// The maximum amount of time to wait before resuming an operation.
    max: time::Duration,
    jitter: f64,
    /// The ratio of the base timeout that may be randomly added to a backoff.
    ///
    /// Must be greater than or equal to 0.0.
    rng: HasherRng,
    iterations: u32,
}

impl ExponentialBackoff {
    pub fn new(min: time::Duration, max: time::Duration, jitter: f64) -> Self {
        Self {
            min,
            max,
            jitter,
            ..Default::default()
        }
    }

    fn base(&self) -> time::Duration {
        debug_assert!(
            self.min <= self.max,
            "maximum backoff must not be less than minimum backoff"
        );
        debug_assert!(
            self.max > time::Duration::from_millis(0),
            "Maximum backoff must be non-zero"
        );
        self.min
            .checked_mul(2_u32.saturating_pow(self.iterations))
            .unwrap_or(self.max)
            .min(self.max)
    }

    /// Returns a random, uniform duration on `[0, base*self.jitter]` no greater
    /// than `self.max`.
    fn jitter(&mut self, base: time::Duration) -> time::Duration {
        if self.jitter == 0.0 {
            time::Duration::default()
        } else {
            let jitter_factor = self.rng.next_f64();
            debug_assert!(
                jitter_factor > 0.0,
                "rng returns values between 0.0 and 1.0"
            );
            let rand_jitter = jitter_factor * self.jitter;
            let secs = (base.as_secs() as f64) * rand_jitter;
            let nanos = (base.subsec_nanos() as f64) * rand_jitter;
            let remaining = self.max - base;
            time::Duration::new(secs as u64, nanos as u32).min(remaining)
        }
    }
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self {
            min: Duration::from_secs(1),
            max: Duration::from_secs(60),
            jitter: 2.0,
            rng: HasherRng::new(),
            iterations: 0,
        }
    }
}

impl Backoff for ExponentialBackoff {
    type Future = Pin<Box<dyn Future<Output = Self> + Send>>;

    fn next(&self) -> Self::Future {
        let mut this = self.clone();
        let fut = async {
            let base = this.base();
            let timeout = base + this.jitter(base);
            this.iterations += 1;
            tokio::time::sleep(timeout).await;
            this
        };
        Box::pin(fut)
    }
}

#[derive(Clone)]
pub struct WithBackoff<B> {
    attempts: u32,
    backoff: B,
}

impl<B> WithBackoff<B> {
    pub fn new(attempts: u32, backoff: B) -> Self {
        Self { attempts, backoff }
    }
}

impl<B, ReqBody, ResBody, E> Policy<Request<ReqBody>, Response<ResBody>, E> for WithBackoff<B>
where
    ReqBody: http_body::Body + Clone,
    B: Backoff + Clone + Send + Sync + 'static,
{
    type Future = Pin<Box<dyn Future<Output = Self> + Send>>;

    fn retry(
        &self,
        _req: &Request<ReqBody>,
        result: Result<&Response<ResBody>, &E>,
    ) -> Option<Self::Future> {
        if let Ok(res) = result {
            if res.status().is_success() {
                return None;
            }
        }

        if self.attempts == 0 {
            return None;
        }

        let mut this = self.clone();
        let fut = async move {
            this.backoff = this.backoff.next().await;
            this.attempts -= 1;
            this
        };

        Some(Box::pin(fut))
    }

    fn clone_request(&self, req: &Request<ReqBody>) -> Option<Request<ReqBody>> {
        let mut b = Request::builder();
        b = b.uri(req.uri().clone());
        b = b.method(req.method().clone());
        for (k, v) in req.headers() {
            if k != http::header::AUTHORIZATION {
                b = b.header(k, v);
            }
        }
        let req = b.body(req.body().clone());
        let req = req.expect("request cloned");
        Some(req)
    }
}
