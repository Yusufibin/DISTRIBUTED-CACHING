use std::{
    sync::{
        atomic::{AtomicU64, AtomicU8, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

pub struct CircuitBreaker {
    state: AtomicU8,
    failure_count: AtomicU64,
    success_count: AtomicU64,
    last_failure_at: Mutex<Option<Instant>>,
    threshold: u64,
    timeout: Duration,
    half_open_successes: u64,
}

impl CircuitBreaker {
    pub fn new(threshold: u64, timeout: Duration) -> Self {
        CircuitBreaker {
            state: AtomicU8::new(STATE_CLOSED),
            failure_count: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            last_failure_at: Mutex::new(None),
            threshold,
            timeout,
            half_open_successes: 3,
        }
    }

    pub fn is_open(&self) -> bool {
        match self.state.load(Ordering::Acquire) {
            STATE_OPEN => {
                let last = self.last_failure_at.lock().unwrap();
                if let Some(ts) = *last {
                    if ts.elapsed() >= self.timeout {
                        drop(last);
                        self.state.store(STATE_HALF_OPEN, Ordering::Release);
                        return false;
                    }
                }
                true
            }
            STATE_HALF_OPEN => false,
            _ => false,
        }
    }

    pub fn record_success(&self) {
        match self.state.load(Ordering::Acquire) {
            STATE_HALF_OPEN => {
                let count = self.success_count.fetch_add(1, Ordering::Relaxed) + 1;
                if count >= self.half_open_successes {
                    self.state.store(STATE_CLOSED, Ordering::Release);
                    self.failure_count.store(0, Ordering::Relaxed);
                    self.success_count.store(0, Ordering::Relaxed);
                    tracing::info!("Circuit breaker closed after recovery");
                }
            }
            _ => {
                self.failure_count.store(0, Ordering::Relaxed);
            }
        }
    }

    pub fn record_failure(&self) {
        let count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
        *self.last_failure_at.lock().unwrap() = Some(Instant::now());

        if count >= self.threshold {
            let prev = self.state.swap(STATE_OPEN, Ordering::AcqRel);
            if prev != STATE_OPEN {
                tracing::warn!("Circuit breaker opened after {} failures", count);
            }
        }
    }

    #[allow(dead_code)]
    pub fn state_str(&self) -> &'static str {
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED => "closed",
            STATE_OPEN => "open",
            STATE_HALF_OPEN => "half-open",
            _ => "unknown",
        }
    }
}
