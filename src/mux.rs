//! Stream multiplexing
//!
//! Multiplexes multiple sinks into a single one, allowing no more than one frame to be buffered for
//! each. Up to 256 channels are supported, being encoded with a leading byte on the underlying
//! downstream.
//!
//! ## Fairness
//!
//! Multiplexing is fair per handle, that is every handle is eventually guaranteed to receive a slot
//! for sending on the underlying sink. Under maximal contention, every `MultiplexerHandle` will
//! receive `1/n` of the slots, with `n` being the total number of multiplexers, with no handle
//! being able to send more than twice without all other waiting handles receiving a slot.

use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use bytes::Buf;
use futures::{FutureExt, Sink, SinkExt};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio_util::sync::ReusableBoxFuture;

use crate::ImmediateFrame;

pub type ChannelPrefixedFrame<F> = bytes::buf::Chain<ImmediateFrame<[u8; 1]>, F>;

/// A frame multiplexer.
///
/// A multiplexer is not used directly, but used to spawn multiplexing handles.
pub struct Multiplexer<S> {
    /// The shared sink for output.
    sink: Arc<Mutex<Option<S>>>,
}

impl<S> Multiplexer<S> {
    /// Creates a new multiplexer with the given sink.
    pub fn new(sink: S) -> Self {
        Self {
            sink: Arc::new(Mutex::new(Some(sink))),
        }
    }

    /// Create a handle for a specific multiplexer channel on this multiplexer.
    ///
    /// Any item sent via this handle's `Sink` implementation will be sent on the given channel.
    ///
    /// It is valid to have multiple handles for the same channel.
    pub fn create_channel_handle(&self, channel: u8) -> MultiplexerHandle<S>
    where
        S: Send + 'static,
    {
        MultiplexerHandle {
            sink: self.sink.clone(),
            channel,
            lock_future: ReusableBoxFuture::new(mk_lock_future(self.sink.clone())),
            guard: None,
        }
    }

    /// Deconstructs the multiplexer into its sink.
    ///
    /// This function will block until outstanding writes to the underlying sink have completed. Any
    /// handle to this multiplexer will be closed afterwards.
    pub fn into_inner(self) -> S {
        self.sink
            .blocking_lock()
            .take()
            // This function is the only one ever taking out of the `Option<S>` and it consumes the
            // only `Multiplexer`, thus we can always expect a `Some` value here.
            .expect("did not expect sink to be missing")
    }
}

/// A guard of a protected sink.
type SinkGuard<S> = OwnedMutexGuard<Option<S>>;

/// Helper function to create a locking future.
///
/// It is important to always return a same-sized future when replacing futures using
/// `ReusableBoxFuture`. For this reason, lock futures are only ever created through this helper
/// function.
fn mk_lock_future<S>(
    sink: Arc<Mutex<Option<S>>>,
) -> impl futures::Future<Output = tokio::sync::OwnedMutexGuard<Option<S>>> {
    sink.lock_owned()
}

/// A handle to a multiplexer.
///
/// A handle is bound to a specific channel, see [`Multiplexer::create_channel_handle`] for details.
pub struct MultiplexerHandle<S> {
    /// The sink shared across the multiplexer and all its handles.
    sink: Arc<Mutex<Option<S>>>,
    /// Channel ID assigned to this handle.
    channel: u8,
    /// The future locking the shared sink.
    // Note: To avoid frequent heap allocations, a single box is reused for every lock this handle
    //       needs to acquire, whcich is on every sending of an item via `Sink`.
    //
    //       This relies on the fact that merely instantiating the locking future (via
    //       `mk_lock_future`) will not do anything before the first poll (TODO: write test).
    lock_future: ReusableBoxFuture<'static, SinkGuard<S>>,
    /// A potential acquired guard for the underlying sink.
    guard: Option<SinkGuard<S>>,
}

impl<S> MultiplexerHandle<S>
where
    S: Send + 'static,
{
    /// Retrieve the shared sink.
    ///
    /// # Panics
    ///
    /// If no guard is held in `self.guard`, panics.
    fn assume_get_sink(&mut self) -> &mut S {
        match self.guard {
            Some(ref mut guard) => {
                let mref = guard
                    .as_mut()
                    .expect("TODO: sink disappeard -- could be closed");
                mref
            }
            None => {
                todo!("assumption failed")
            }
        }
    }
}

impl<F, S> Sink<F> for MultiplexerHandle<S>
where
    S: Sink<ChannelPrefixedFrame<F>> + Unpin + Send + 'static,
    F: Buf,
{
    type Error = <S as Sink<ChannelPrefixedFrame<F>>>::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let guard = match self.guard {
            None => {
                // We do not hold the guard at the moment, so attempt to acquire it.
                match self.lock_future.poll_unpin(cx) {
                    Poll::Ready(guard) => {
                        // It is our turn: Save the guard and prepare another locking future for later,
                        // which will not attempt to lock until first polled.
                        let guard = self.guard.insert(guard);
                        let sink = self.sink.clone();
                        self.lock_future.set(mk_lock_future(sink));
                        guard
                    }
                    Poll::Pending => {
                        // The lock could not be acquired yet.
                        return Poll::Pending;
                    }
                }
            }
            Some(ref mut guard) => guard,
        };

        // At this point we have acquired the lock, now our only job is to stuff data into the sink.
        guard
            .as_mut()
            .expect("TODO: closed sink")
            .poll_ready_unpin(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: F) -> Result<(), Self::Error> {
        let prefixed = ImmediateFrame::from(self.channel).chain(item);
        self.assume_get_sink().start_send_unpin(prefixed)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Obtain the flush result, then release the sink lock.
        match self.assume_get_sink().poll_flush_unpin(cx) {
            Poll::Ready(Ok(())) => {
                // Acquire wait list lock to update it.
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(_)) => {
                todo!("handle error")
            }

            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Simply close? Note invariants, possibly checking them in debug mode.
        todo!()
    }
}
