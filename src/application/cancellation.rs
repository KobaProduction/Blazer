use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use vpn_domain::client::ClientError;

#[derive(Default)]
pub struct CancellationSignal {
    generation: AtomicU64,
    waiters: Mutex<Vec<(u64, Waker)>>,
}

impl CancellationSignal {
    pub fn cancel(&self, request_id: u64) {
        self.generation.fetch_max(request_id, Ordering::AcqRel);
        let waiters = self
            .waiters
            .lock()
            .map(|mut values| std::mem::take(&mut *values))
            .unwrap_or_default();
        for (_, waker) in waiters {
            waker.wake();
        }
    }
}

#[derive(Clone)]
pub struct Cancellation {
    signal: Arc<CancellationSignal>,
    request_id: u64,
}

impl Cancellation {
    pub fn new(signal: Arc<CancellationSignal>, request_id: u64) -> Self {
        Self { signal, request_id }
    }

    pub fn check(&self) -> Result<(), ClientError> {
        if self.signal.generation.load(Ordering::Acquire) >= self.request_id {
            Err(ClientError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn cancelled(&self) -> Cancelled<'_> {
        Cancelled { cancellation: self }
    }
}

pub struct Cancelled<'a> {
    cancellation: &'a Cancellation,
}

impl Future for Cancelled<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.cancellation.check().is_err() {
            return Poll::Ready(());
        }
        if let Ok(mut waiters) = self.cancellation.signal.waiters.lock() {
            if let Some((_, waker)) = waiters
                .iter_mut()
                .find(|(id, _)| *id == self.cancellation.request_id)
            {
                *waker = context.waker().clone();
            } else {
                if waiters.len() == 8 {
                    waiters.remove(0);
                }
                waiters.push((self.cancellation.request_id, context.waker().clone()));
            }
        }
        if self.cancellation.check().is_err() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Drop for Cancelled<'_> {
    fn drop(&mut self) {
        if let Ok(mut waiters) = self.cancellation.signal.waiters.lock() {
            waiters.retain(|(id, _)| *id != self.cancellation.request_id);
        }
    }
}
