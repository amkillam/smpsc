//! Wrappers for [tokio](https://docs.rs/tokio)'s channels that implement [`Stream`] for receivers
//! and [`Sink`] for senders.
//!
//! [`Stream`]: trait@tokio_stream::Stream
//! [`Sink`]: trait@async_sink::Sink

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod mpsc;
pub mod oneshot;

#[cfg(feature = "alloc")]
pub use mpsc::{channel, unbounded_channel};
