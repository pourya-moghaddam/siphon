use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

#[derive(Clone)]
pub struct TokenBucket {
    inner: Arc<Mutex<BucketInner>>,
}

struct BucketInner {
    bytes_per_sec: u64,
    tokens: f64,
    capacity: f64,
    last_update: Instant,
}

impl TokenBucket {
    pub fn new(bytes_per_sec: u64) -> Self {
        let capacity = (bytes_per_sec as f64).max(64.0 * 1024.0); // At least 64KB burst capacity
        Self {
            inner: Arc::new(Mutex::new(BucketInner {
                bytes_per_sec,
                tokens: capacity,
                capacity,
                last_update: Instant::now(),
            })),
        }
    }

    pub fn unlimited() -> Self {
        Self::new(0)
    }

    pub async fn set_rate(&self, bytes_per_sec: u64) {
        let mut inner = self.inner.lock().await;
        inner.bytes_per_sec = bytes_per_sec;
        let capacity = (bytes_per_sec as f64).max(64.0 * 1024.0);
        inner.capacity = capacity;
        if inner.tokens > capacity {
            inner.tokens = capacity;
        }
    }

    pub async fn acquire(&self, needed_bytes: usize) {
        if needed_bytes == 0 {
            return;
        }

        loop {
            let sleep_duration = {
                let mut inner = self.inner.lock().await;
                if inner.bytes_per_sec == 0 {
                    // Unlimited
                    return;
                }

                let now = Instant::now();
                let elapsed = now.duration_since(inner.last_update).as_secs_f64();
                inner.last_update = now;

                // Refill tokens
                inner.tokens = (inner.tokens + elapsed * inner.bytes_per_sec as f64)
                    .min(inner.capacity);

                let needed = needed_bytes as f64;
                if inner.tokens >= needed {
                    inner.tokens -= needed;
                    return;
                }

                // Not enough tokens, calculate time to sleep
                let deficit = needed - inner.tokens;
                let wait_secs = deficit / inner.bytes_per_sec as f64;
                Duration::from_secs_f64(wait_secs)
            };

            sleep(sleep_duration).await;
        }
    }
}
