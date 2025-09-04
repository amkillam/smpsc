//! A wrapper [`Sink`] implementation for [`tokio::sync::oneshot`].
//!
//! [`Sink`]: trait@async_sink::Sink
//! [`tokio::sync::oneshot`]: tokio::sync::oneshot

use async_sink::Sink;
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::oneshot;

/// A thin wrapper around [`tokio::sync::oneshot::Sender`] that implements [`Sync`].
///
/// [`tokio::sync::oneshot::Sender`]: struct@tokio::sync::oneshot::Sender
/// [`Sink`]: trait@async_sink::Sink
#[derive(Debug)]
#[repr(transparent)]
pub struct Sender<T>(pub Option<oneshot::Sender<T>>);

impl<T> Sender<T> {
    /// Create a new `Sender` wrapping the provided `Sender`.
    #[inline(always)]
    pub fn new(sender: oneshot::Sender<T>) -> Self {
        Self(Some(sender))
    }

    /// Get back the inner `Sender`.
    #[inline(always)]
    pub fn into_inner(self) -> Option<oneshot::Sender<T>> {
        self.0
    }

    #[inline(always)]
    pub fn send(self, t: T) -> Result<(), T> {
        match self.0 {
            Some(sender) => sender.send(t),
            None => Err(t),
        }
    }

    #[inline(always)]
    pub async fn closed(&mut self) {
        if let Some(sender) = self.0.as_mut() {
            sender.closed().await;
        }
    }

    #[inline(always)]
    pub fn is_closed(&self) -> bool {
        match self.0.as_ref() {
            None => true,
            Some(inner) => inner.is_closed(),
        }
    }

    #[inline(always)]
    pub fn poll_closed(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        match self.0.as_mut() {
            Some(sender) => sender.poll_closed(cx),
            None => Poll::Ready(()),
        }
    }
}

impl<T> From<oneshot::Sender<T>> for Sender<T> {
    #[inline(always)]
    fn from(sender: oneshot::Sender<T>) -> Self {
        Self::new(sender)
    }
}

impl<T> Sink<T> for Sender<T> {
    type Error = T;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // oneshot::Sender can accept exactly one message; we allow attempting to send
        // and let start_send error if it's already been used.
        assert!(
            self.0.is_some(),
            "poll_ready called after sending a message"
        );
        cx.waker().wake_by_ref();
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        match self.get_mut().0.take() {
            Some(sender) => sender.send(item),
            None => Err(item),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_close(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.as_mut().0.as_mut() {
            Some(s) => match s.poll_closed(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(()) => {
                    self.get_mut().0 = None;
                    Poll::Ready(Ok(()))
                }
            },
            None => Poll::Ready(Ok(())),
        }
    }
}
