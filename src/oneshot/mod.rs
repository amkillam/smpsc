pub mod sink;
pub mod stream;

pub use sink::Sender;
pub use stream::Receiver;

/// Create a new oneshot channel, returning a [`Sender`] and a [`ReceiverStream`].
///
/// [`Sender`]: struct@Sender
/// [`ReceiverStream`]: struct@Receiver
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    (Sender::new(tx), Receiver::new(rx))
}
