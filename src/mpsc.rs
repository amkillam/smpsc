//! [`Sink`] implementations for [`tokio`](https://docs.rs/tokio)'s MPSC channels
//!
//! [`Sink`]: trait@async_sink::Sink

use alloc::boxed::Box;
use async_sink::Sink;
use core::fmt;
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc::{
    self, OwnedPermit, Permit, PermitIterator, WeakSender, WeakUnboundedSender,
};
use tokio_stream::wrappers::{ReceiverStream, UnboundedReceiverStream};

pub use tokio::sync::mpsc::error;
pub use tokio::sync::mpsc::error::*;

/// A thin wrapper around [`tokio::sync::mpsc::Sender`] that implements [`Sync`].
///
/// [`tokio::sync::mpsc::Sender`]: struct@tokio::sync::mpsc::Sender
/// [`Sink`]: trait@async_sink::Sink
pub struct Sender<'a, T: 'a> {
    pub(crate) inner: mpsc::Sender<T>,
    // Future created by `reserve()` to register wakers for readiness.
    // Stored across polls to maintain proper waker registration.
    #[allow(clippy::type_complexity)]
    reserve_fut: Option<
        Pin<
            Box<
                dyn core::future::Future<
                        Output = Result<mpsc::OwnedPermit<T>, mpsc::error::SendError<()>>,
                    > + 'a,
            >,
        >,
    >,
    reserved_permit: Option<mpsc::OwnedPermit<T>>,
    #[allow(clippy::type_complexity)]
    closed_fut: Option<Pin<Box<dyn core::future::Future<Output = ()> + 'a>>>,
    #[allow(clippy::type_complexity)]
    send_fut: Option<Pin<Box<dyn core::future::Future<Output = Result<(), SendError<()>>> + 'a>>>,
    _lifetime: core::marker::PhantomData<&'a ()>,
}

// SAFETY: reserve_fut is the only non-Send field, and it is pinned and owned by the struct.
unsafe impl<'a, T> Send for Sender<'a, T> where mpsc::Sender<T>: Send {}
// SAFETY: reserve_fut is the only non-Sync field, and it is pinned and owned by the struct.
unsafe impl<'a, T> Sync for Sender<'a, T> where mpsc::Sender<T>: Sync {}

impl<'a, T> Sender<'a, T> {
    /// Create a new `Sender` wrapping the provided `Sender`.
    #[inline(always)]
    pub fn new(sender: mpsc::Sender<T>) -> Self {
        Self {
            inner: sender,
            reserve_fut: None,
            reserved_permit: None,
            closed_fut: None,
            send_fut: None,
            _lifetime: core::marker::PhantomData,
        }
    }

    /// Get back the inner `Sender`.
    #[inline(always)]
    pub fn into_inner(self) -> mpsc::Sender<T> {
        self.inner
    }

    #[inline(always)]
    pub async fn closed(&self) {
        self.inner.closed().await
    }

    #[inline(always)]
    pub async fn reserve_many(&self, n: usize) -> Result<PermitIterator<'_, T>, SendError<()>> {
        self.inner.reserve_many(n).await
    }

    #[inline(always)]
    pub async fn reserve_owned(self) -> Result<OwnedPermit<T>, SendError<()>> {
        self.inner.reserve_owned().await
    }

    #[inline(always)]
    pub async fn reserve(&self) -> Result<Permit<'_, T>, SendError<()>> {
        self.inner.reserve().await
    }

    #[inline(always)]
    pub async fn send(&self, value: T) -> Result<(), SendError<T>> {
        self.inner.send(value).await
    }

    #[cfg(feature = "time")]
    #[inline(always)]
    pub async fn send_timeout(
        &self,
        value: T,
        timeout: core::time::Duration,
    ) -> Result<(), SendTimeoutError<T>> {
        self.inner.send_timeout(value, timeout).await
    }

    #[inline(always)]
    pub fn blocking_send(&self, value: T) -> Result<(), SendError<T>> {
        self.inner.blocking_send(value)
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    #[inline(always)]
    pub fn downgrade(&self) -> WeakSender<T> {
        self.inner.downgrade()
    }

    #[inline(always)]
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    #[inline(always)]
    pub fn max_capacity(&self) -> usize {
        self.inner.max_capacity()
    }

    #[inline(always)]
    pub fn same_channel(&self, other: &Self) -> bool {
        self.inner.same_channel(&other.inner)
    }

    #[inline(always)]
    pub fn strong_count(&self) -> usize {
        self.inner.strong_count()
    }

    #[inline(always)]
    pub fn try_reserve_many(&self, n: usize) -> Result<PermitIterator<'_, T>, TrySendError<()>> {
        self.inner.try_reserve_many(n)
    }

    #[inline(always)]
    pub fn try_reserve_owned(self) -> Result<OwnedPermit<T>, TrySendError<Self>> {
        self.inner.try_reserve_owned().map_err(|e| match e {
            mpsc::error::TrySendError::Closed(e) => mpsc::error::TrySendError::Closed(Self {
                inner: e,
                closed_fut: None,
                reserve_fut: None,
                reserved_permit: None,
                send_fut: None,
                _lifetime: core::marker::PhantomData,
            }),
            mpsc::error::TrySendError::Full(e) => mpsc::error::TrySendError::Full(Self {
                inner: e,
                closed_fut: None,
                reserve_fut: None,
                reserved_permit: None,
                send_fut: None,
                _lifetime: core::marker::PhantomData,
            }),
        })
    }

    #[inline(always)]
    pub fn try_reserve(&self) -> Result<Permit<'_, T>, TrySendError<()>> {
        self.inner.try_reserve()
    }

    #[inline(always)]
    pub fn try_send(&self, message: T) -> Result<(), TrySendError<T>> {
        self.inner.try_send(message)
    }

    #[inline(always)]
    pub fn weak_count(&self) -> usize {
        self.inner.weak_count()
    }
}

impl<'a, T> AsRef<mpsc::Sender<T>> for Sender<'a, T> {
    #[inline(always)]
    fn as_ref(&self) -> &mpsc::Sender<T> {
        &self.inner
    }
}

impl<'a, T> AsMut<mpsc::Sender<T>> for Sender<'a, T> {
    #[inline(always)]
    fn as_mut(&mut self) -> &mut mpsc::Sender<T> {
        &mut self.inner
    }
}

impl<'a, T> Clone for Sender<'a, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            closed_fut: None,
            reserve_fut: None,
            reserved_permit: None,
            send_fut: None,
            _lifetime: core::marker::PhantomData,
        }
    }
}

impl<'a, T> fmt::Debug for Sender<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sender")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<'a, T> From<mpsc::Sender<T>> for Sender<'a, T> {
    #[inline(always)]
    fn from(sender: mpsc::Sender<T>) -> Self {
        Self::new(sender)
    }
}

impl<'a, T: 'a> Sink<T> for Sender<'a, T> {
    type Error = mpsc::error::SendError<()>;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.as_mut().get_mut();

        if this.inner.is_closed() {
            Poll::Ready(Err(mpsc::error::SendError(())))
        } else {
            match this.reserved_permit {
                Some(_) => Poll::Ready(Ok(())),
                None => {
                    let waker_clone = cx.waker().clone();
                    if this.reserve_fut.is_none() {
                        let sender = this.inner.clone();
                        this.reserve_fut = Some(Box::pin(async move {
                            let ret = match sender.reserve_owned().await {
                                Ok(permit) => Ok(permit),
                                Err(_e) => Err(mpsc::error::SendError(())),
                            };
                            waker_clone.wake_by_ref();
                            ret
                        }));
                    }

                    match this
                        .reserve_fut
                        .as_mut()
                        .expect("reserve_fut must be set")
                        .as_mut()
                        .poll(cx)
                    {
                        Poll::Ready(Ok(permit)) => {
                            this.reserve_fut = None;
                            this.reserved_permit = Some(permit);
                            Poll::Ready(Ok(()))
                        }
                        Poll::Ready(Err(_)) => {
                            this.reserve_fut = None;
                            Poll::Ready(Err(mpsc::error::SendError(())))
                        }
                        Poll::Pending => Poll::Pending,
                    }
                }
            }
        }
    }

    fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        let this = self.get_mut();
        if this.inner.is_closed() {
            Err(mpsc::error::SendError(()))
        } else {
            match this.reserved_permit.take() {
                None => match this.send_fut.take() {
                    Some(send_fut) => {
                        let cloned_inner = this.inner.clone();
                        let new_send_fut = Box::pin(async move {
                            cloned_inner
                                .send(item)
                                .await
                                .map_err(|_e| mpsc::error::SendError(()))
                        });
                        this.send_fut = Some(Box::pin(async move {
                            send_fut.await.map_err(|_e| mpsc::error::SendError(()))?;
                            new_send_fut.await.map_err(|_e| mpsc::error::SendError(()))
                        }));

                        Ok(())
                    }
                    None => {
                        let cloned_inner = this.inner.clone();
                        this.send_fut = Some(Box::pin(async move {
                            cloned_inner
                                .send(item)
                                .await
                                .map_err(|_| mpsc::error::SendError(()))
                        }));
                        Ok(())
                    }
                },
                Some(permit) => {
                    permit.send(item);
                    Ok(())
                }
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.send_fut.as_mut() {
            None => Poll::Ready(Ok(())),
            Some(fut) => match fut.as_mut().poll(cx) {
                Poll::Ready(_ret) => {
                    self.send_fut = None;
                    Poll::Ready(Ok(()))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let mut this = self.as_mut();
        if let Some(closed_fut) = this.closed_fut.as_mut() {
            match closed_fut.as_mut().poll(cx) {
                Poll::Ready(ret) => Poll::Ready(Ok(ret)),
                Poll::Pending => Poll::Pending,
            }
        } else {
            let inner_clone = this.inner.clone();
            let waker_clone = cx.waker().clone();
            this.closed_fut = Some(Box::pin(async move {
                inner_clone.closed().await;
                waker_clone.wake_by_ref();
            }));
            match this.closed_fut.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Ready(_) => Poll::Ready(Ok(())),
                Poll::Pending => Poll::Pending,
            }
        }
    }
}

/// A thin wrapper around [`tokio::sync::mpsc::UnboundedSender`] that implements [`Sync`].
///
/// [`tokio::sync::mpsc::UnboundedSender`]: struct@tokio::sync::mpsc::UnboundedSender
/// [`Sink`]: trait@async_sink::Sink
pub struct UnboundedSender<'a, T: 'a> {
    pub(crate) inner: mpsc::UnboundedSender<T>,
    _lifetime: core::marker::PhantomData<&'a ()>,
    closed_fut: Option<Pin<Box<dyn core::future::Future<Output = ()> + 'a>>>,
}

unsafe impl<'a, T> Send for UnboundedSender<'a, T> where mpsc::UnboundedSender<T>: Send {}
unsafe impl<'a, T> Sync for UnboundedSender<'a, T> where mpsc::UnboundedSender<T>: Sync {}

impl<'a, T> UnboundedSender<'a, T> {
    /// Create a new `UnboundedSender` wrapping the provided `UnboundedSender`.
    #[inline(always)]
    pub fn new(sender: mpsc::UnboundedSender<T>) -> Self {
        Self {
            inner: sender,
            closed_fut: None,
            _lifetime: core::marker::PhantomData,
        }
    }

    /// Get back the inner `UnboundedSender`.
    #[inline(always)]
    pub fn into_inner(self) -> mpsc::UnboundedSender<T> {
        self.inner
    }

    #[inline(always)]
    pub async fn closed(&self) {
        self.inner.closed().await
    }

    #[inline(always)]
    pub fn downgrade(&self) -> WeakUnboundedSender<T> {
        self.inner.downgrade()
    }

    #[inline(always)]
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    #[inline(always)]
    pub fn same_channel(&self, other: &Self) -> bool {
        self.inner.same_channel(&other.inner)
    }

    #[inline(always)]
    pub fn send(&self, message: T) -> Result<(), SendError<T>> {
        self.inner.send(message)
    }

    #[inline(always)]
    pub fn strong_count(&self) -> usize {
        self.inner.strong_count()
    }

    #[inline(always)]
    pub fn weak_count(&self) -> usize {
        self.inner.weak_count()
    }
}

impl<'a, T> AsRef<mpsc::UnboundedSender<T>> for UnboundedSender<'a, T> {
    #[inline(always)]
    fn as_ref(&self) -> &mpsc::UnboundedSender<T> {
        &self.inner
    }
}

impl<'a, T> AsMut<mpsc::UnboundedSender<T>> for UnboundedSender<'a, T> {
    #[inline(always)]
    fn as_mut(&mut self) -> &mut mpsc::UnboundedSender<T> {
        &mut self.inner
    }
}

impl<'a, T> From<mpsc::UnboundedSender<T>> for UnboundedSender<'a, T> {
    #[inline(always)]
    fn from(sender: mpsc::UnboundedSender<T>) -> Self {
        Self::new(sender)
    }
}

impl<'a, T> Sink<T> for UnboundedSender<'a, T> {
    type Error = mpsc::error::SendError<T>;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // The unbounded sender is always ready to send
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        self.get_mut().inner.send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        <Self as Sink<T>>::poll_close(self, cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let mut this = self.as_mut();
        if let Some(closed_fut) = this.closed_fut.as_mut() {
            closed_fut.as_mut().poll(cx).map(|_| Ok(()))
        } else {
            let inner_clone = this.inner.clone();
            this.closed_fut = Some(Box::pin(async move { inner_clone.closed().await }));
            this.closed_fut
                .as_mut()
                .unwrap()
                .as_mut()
                .poll(cx)
                .map(|_| Ok(()))
        }
    }
}

/// Creates a bounded MPSC channel, returning the sender wrapped in a [`Sender`] and the receiver wrapped in a [`ReceiverStream`].
///
/// [`Sender`]: struct@Sender
/// [`ReceiverStream`]: struct@tokio_stream::wrappers::ReceiverStream
#[inline(always)]
pub fn channel<'a, T>(buffer: usize) -> (Sender<'a, T>, ReceiverStream<T>) {
    let (tx, rx) = mpsc::channel(buffer);
    (Sender::new(tx), ReceiverStream::new(rx))
}

/// Creates an unbounded MPSC channel, returning the sender wrapped in a [`UnboundedSender`] and the receiver wrapped in a [`UnboundedReceiverStream`].
///
/// [`UnboundedSender`]: struct@UnboundedSender
/// [`UnboundedReceiverStream`]: struct@tokio_stream::wrappers::UnboundedReceiverStream
#[inline(always)]
pub fn unbounded_channel<'a, T>() -> (UnboundedSender<'a, T>, UnboundedReceiverStream<T>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (UnboundedSender::new(tx), UnboundedReceiverStream::new(rx))
}
