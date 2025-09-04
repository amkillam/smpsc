#[cfg(feature = "alloc")]
mod if_async {
    pub use smpsc::oneshot;
    pub extern crate alloc;
    pub use alloc::{collections::vec_deque::VecDeque, rc::Rc, sync::Arc, vec::Vec};
    pub use core::{
        cell::{Cell, RefCell},
        convert::Infallible,
        future::Future,
        mem,
        pin::Pin,
        sync::atomic::{AtomicBool, Ordering},
        task::{Context, Poll, Waker},
    };
    pub use futures_util::{FutureExt, TryFutureExt};
    pub use tokio_stream::StreamExt;

    pub use async_sink::{Sink, SinkExt};

    // Sends a value on an i32 channel sink
    pub struct StartSendFut<S: async_sink::Sink<Item> + Unpin, Item: Unpin>(
        Option<S>,
        Option<Item>,
    );

    impl<S: async_sink::Sink<Item> + Unpin, Item: Unpin> StartSendFut<S, Item> {
        pub fn new(sink: S, item: Item) -> Self {
            Self(Some(sink), Some(item))
        }
    }
    impl<S: async_sink::Sink<Item> + Unpin, Item: Unpin> Future for StartSendFut<S, Item> {
        type Output = Result<S, S::Error>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let Self(inner, item) = self.get_mut();
            {
                let mut inner = inner.as_mut().unwrap();
                futures_util::ready!(Pin::new(&mut inner).poll_ready(cx))?;
                Pin::new(&mut inner).start_send(item.take().unwrap())?;
            }
            Poll::Ready(Ok(inner.take().unwrap()))
        }
    }

    // Immediately accepts all requests to start pushing, but completion is managed
    // by manually flushing
    pub struct ManualFlush<T: Unpin> {
        data: Vec<T>,
        waiting_tasks: Vec<Waker>,
    }

    impl<T: Unpin> async_sink::Sink<Option<T>> for ManualFlush<T> {
        type Error = ();

        fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(mut self: Pin<&mut Self>, item: Option<T>) -> Result<(), Self::Error> {
            if let Some(item) = item {
                self.data.push(item);
            } else {
                self.force_flush();
            }
            Ok(())
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            if self.data.is_empty() {
                Poll::Ready(Ok(()))
            } else {
                self.waiting_tasks.push(cx.waker().clone());
                Poll::Pending
            }
        }

        fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.poll_flush(cx)
        }
    }

    impl<T: Unpin> ManualFlush<T> {
        pub fn new() -> Self {
            Self {
                data: Vec::new(),
                waiting_tasks: Vec::new(),
            }
        }

        pub fn force_flush(&mut self) -> Vec<T> {
            for task in self.waiting_tasks.drain(..) {
                task.wake()
            }
            mem::take(&mut self.data)
        }
    }

    pub struct ManualAllow<T: Unpin> {
        pub data: Vec<T>,
        pub allow: Rc<Allow>,
    }

    pub struct Allow {
        pub flag: core::cell::Cell<bool>,
        pub tasks: core::cell::RefCell<Vec<Waker>>,
    }

    impl Allow {
        pub fn new() -> Self {
            Self {
                flag: Cell::new(false),
                tasks: RefCell::new(Vec::new()),
            }
        }

        pub fn check(&self, cx: &mut Context<'_>) -> bool {
            if self.flag.get() {
                true
            } else {
                self.tasks.borrow_mut().push(cx.waker().clone());
                false
            }
        }

        pub fn start(&self) {
            self.flag.set(true);
            let mut tasks = self.tasks.borrow_mut();
            for task in tasks.drain(..) {
                task.wake();
            }
        }
    }

    impl<T: Unpin> Sink<T> for ManualAllow<T> {
        type Error = ();

        fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            if self.allow.check(cx) {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        }

        fn start_send(mut self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
            self.data.push(item);
            Ok(())
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    pub fn manual_allow<T: Unpin>() -> (ManualAllow<T>, Rc<Allow>) {
        let allow = Rc::new(Allow::new());
        let manual_allow = ManualAllow {
            data: Vec::new(),
            allow: allow.clone(),
        };
        (manual_allow, allow)
    }

    // An Unpark struct that records unpark events for inspection
    pub struct Flag(AtomicBool);

    impl Flag {
        pub fn new() -> Arc<Self> {
            Arc::new(Self(AtomicBool::new(false)))
        }

        pub fn take(&self) -> bool {
            self.0.swap(false, Ordering::SeqCst)
        }

        pub fn set(&self, v: bool) {
            self.0.store(v, Ordering::SeqCst)
        }
    }

    impl futures_util::task::ArcWake for Flag {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.set(true)
        }
    }

    #[inline(always)]
    pub fn flag_cx<'a: 'b + 'c + 'd, 'b: 'c + 'd, 'c: 'd, 'd, F, R>(f: F) -> R
    where
        R: 'a,
        F: FnOnce(Arc<Flag>, &'c mut Context<'b>) -> R,
        F: 'd,
    {
        let flag = Flag::new();
        let waker = futures_util::task::waker(flag.clone());
        let mut cx =
            Context::from_waker(unsafe { core::mem::transmute::<&Waker, &'b Waker>(&waker) });
        f(flag.clone(), unsafe {
            core::mem::transmute::<&mut Context<'b>, &'c mut Context<'b>>(&mut cx)
        })
    }

    #[inline(always)]
    pub async fn async_flag_cx<'a: 'b + 'c + 'd, 'b: 'c + 'd, 'c: 'd, 'd, F, R, Fut>(f: F) -> R
    where
        R: 'd,
        Fut: Future<Output = R> + 'd,
        F: FnOnce(Arc<Flag>, &'c mut Context<'b>) -> Fut,
        F: 'd,
    {
        let flag = Flag::new();
        let waker = futures_util::task::waker(flag.clone());
        let cx = Context::from_waker(unsafe { core::mem::transmute::<&Waker, &'b Waker>(&waker) });
        let leaked_context = Box::leak(Box::new(cx));
        let ret = f(flag.clone(), unsafe {
            core::mem::transmute::<&mut Context<'b>, &'c mut Context<'b>>(leaked_context)
        })
        .await;
        //Unleak
        unsafe {
            let alloc = Box::from_raw(leaked_context as *mut Context<'b>);
            drop(alloc);
        }
        ret
    }

    pub fn sassert_next<S>(s: &mut S, item: S::Item)
    where
        S: tokio_stream::Stream + Unpin,
        S::Item: Eq + core::fmt::Debug,
    {
        match Pin::new(s).poll_next(&mut futures_test::task::panic_context()) {
            Poll::Ready(None) => panic!("stream is at its end"),
            Poll::Ready(Some(e)) => assert_eq!(e, item),
            Poll::Pending => panic!("stream wasn't ready"),
        }
    }

    pub fn unwrap<T, E: core::fmt::Debug>(x: Poll<Result<T, E>>) -> T {
        match x {
            Poll::Ready(Ok(x)) => x,
            Poll::Ready(Err(_)) => panic!("Poll::Ready(Err(_))"),
            Poll::Pending => panic!("Poll::Pending"),
        }
    }
}

#[cfg(feature = "alloc")]
use if_async::*;

#[cfg(feature = "alloc")]
#[tokio::test]
async fn either_sink() {
    let mut s = if true {
        Vec::<i32>::new().left_sink()
    } else {
        VecDeque::<i32>::new().right_sink()
    };

    Pin::new(&mut s).start_send(0).unwrap();
}

#[cfg(feature = "alloc")]
#[tokio::test]
async fn vec_sink() {
    let mut v = Vec::new();
    Pin::new(&mut v).start_send(0).unwrap();
    Pin::new(&mut v).start_send(1).unwrap();
    assert_eq!(v, vec![0, 1]);
    v.flush().await.unwrap();
    assert_eq!(v, vec![0, 1]);
}

#[cfg(feature = "alloc")]
#[tokio::test]
async fn vecdeque_sink() {
    let mut deque = VecDeque::new();
    Pin::new(&mut deque).start_send(2).unwrap();
    Pin::new(&mut deque).start_send(3).unwrap();

    assert_eq!(deque.pop_front(), Some(2));
    assert_eq!(deque.pop_front(), Some(3));
    assert_eq!(deque.pop_front(), None);
}

#[cfg(feature = "alloc")]
#[tokio::test]
async fn send() {
    let mut v = Vec::new();

    v.send(0).await.unwrap();
    assert_eq!(v, vec![0]);

    v.send(1).await.unwrap();
    assert_eq!(v, vec![0, 1]);

    v.send(2).await.unwrap();
    assert_eq!(v, vec![0, 1, 2]);
}

#[cfg(feature = "alloc")]
#[tokio::test]
async fn send_all() {
    let mut v = Vec::new();

    v.send_all(&mut tokio_stream::iter(vec![0, 1]).map(Ok))
        .await
        .unwrap();
    assert_eq!(v, vec![0, 1]);

    v.send_all(&mut tokio_stream::iter(vec![2, 3]).map(Ok))
        .await
        .unwrap();
    assert_eq!(v, vec![0, 1, 2, 3]);

    v.send_all(&mut tokio_stream::iter(vec![4, 5]).map(Ok))
        .await
        .unwrap();
    assert_eq!(v, vec![0, 1, 2, 3, 4, 5]);
}

// Test that `start_send` on an `mpsc` channel does indeed block when the
// channel is full
#[cfg(feature = "alloc")]
#[tokio::test]
async fn mpsc_blocking_start_send() {
    let (mut tx, mut rx) = smpsc::mpsc::channel::<usize>(1);

    futures_util::future::lazy(|_| {
        flag_cx(|flag, cx| {
            assert_eq!(Pin::new(&mut tx).poll_ready(cx), Poll::Ready(Ok(())));
            assert!(flag.take());
            Pin::new(&mut tx).start_send(0).unwrap();
            let mut task = StartSendFut::new(tx, 1);

            assert!(Pin::new(&mut task).poll(cx).is_pending());
            assert!(!flag.take());
            sassert_next(&mut rx, 0);
            assert!(flag.take());
            unwrap(Pin::new(&mut task).poll(cx));
            assert!(flag.take());
            sassert_next(&mut rx, 1);
        })
    })
    .await;
}

// test `flush` by using `with` to make the first insertion into a sink block
// until a oneshot is completed
#[cfg(feature = "alloc")]
#[tokio::test]
async fn with_flush() {
    let (tx, rx) = oneshot::channel();
    let mut block = rx.boxed();
    let mut sink = Vec::new().with(|elem| {
        mem::replace(&mut block, futures_util::future::ok(()).boxed())
            .map_ok(move |()| elem + 1)
            .map_err(|_| -> Infallible { panic!() })
    });

    assert_eq!(Pin::new(&mut sink).start_send(0).ok(), Some(()));

    async_flag_cx(|flag, cx| async move {
        let mut task = sink.flush();
        assert!(task.poll_unpin(cx).is_pending());
        tx.send(()).unwrap();
        assert!(flag.take());

        unwrap(task.poll_unpin(cx));

        sink.send(1).await.unwrap();
        assert_eq!(sink.get_ref(), &[1, 2]);
    })
    .await;
}

// test simple use of with to change data
#[cfg(feature = "alloc")]
#[tokio::test]
async fn with_as_map() {
    let mut sink = Vec::new().with(|item| futures_util::future::ok::<i32, Infallible>(item * 2));
    sink.send(0).await.unwrap();
    sink.send(1).await.unwrap();
    sink.send(2).await.unwrap();
    assert_eq!(sink.get_ref(), &[0, 2, 4]);
}

// test simple use of with_flat_map
#[cfg(feature = "alloc")]
#[tokio::test]
async fn with_flat_map() {
    let mut sink = Vec::new().with_flat_map(|item| tokio_stream::iter(vec![item; item]).map(Ok));
    sink.send(0).await.unwrap();
    sink.send(1).await.unwrap();
    sink.send(2).await.unwrap();
    sink.send(3).await.unwrap();
    assert_eq!(sink.get_ref(), &[1, 2, 2, 3, 3, 3]);
}

// Check that `with` propagates `poll_ready` to the inner sink.
#[cfg(feature = "alloc")]
#[tokio::test]
async fn with_propagates_poll_ready() {
    let (tx, mut rx) = smpsc::mpsc::channel::<i32>(1);
    let mut tx =
        tx.with(|item: i32| futures_util::future::ok::<i32, smpsc::mpsc::SendError<()>>(item + 10));

    futures_util::future::lazy(|_| {
        flag_cx(|flag, cx| {
            let mut tx = Pin::new(&mut tx);

            // Should be ready for the first item.
            assert_eq!(tx.as_mut().poll_ready(cx), Poll::Ready(Ok(())));
            assert_eq!(tx.as_mut().start_send(0), Ok(()));

            // Should be ready for the second item only after the first one is received.
            assert_eq!(tx.as_mut().poll_ready(cx), Poll::Pending);
            assert!(flag.take());
            sassert_next(&mut rx, 10);
            assert!(flag.take());
            assert_eq!(tx.as_mut().poll_ready(cx), Poll::Ready(Ok(())));
            assert_eq!(tx.as_mut().start_send(1), Ok(()));
        })
    })
    .await;
}

// test that the `with` sink doesn't require the underlying sink to flush,
// but doesn't claim to be flushed until the underlying sink is
#[cfg(feature = "alloc")]
#[tokio::test]
async fn with_flush_propagate() {
    let mut sink = ManualFlush::new().with(futures_util::future::ok::<Option<i32>, ()>);
    flag_cx(|flag, cx| {
        unwrap(Pin::new(&mut sink).poll_ready(cx));
        Pin::new(&mut sink).start_send(Some(0)).unwrap();
        unwrap(Pin::new(&mut sink).poll_ready(cx));
        Pin::new(&mut sink).start_send(Some(1)).unwrap();

        {
            let mut task = sink.flush();
            assert!(Pin::new(&mut task).poll(cx).is_pending());
            assert!(!flag.take());
        }
        assert_eq!(sink.get_mut().force_flush(), vec![0, 1]);
        assert!(flag.take());
        let mut flush_fut = sink.flush();
        unwrap(Pin::new(&mut flush_fut).poll(cx));
    })
}

// test that `Clone` is implemented on `with` sinks
#[cfg(feature = "alloc")]
#[tokio::test]
async fn with_implements_clone() {
    let rx = {
        let (tx, rx) = smpsc::mpsc::channel(5);
        {
            let mut is_positive = tx.clone().with(|item| {
                futures_util::future::ok::<bool, smpsc::mpsc::SendError<()>>(item > 0)
            });

            let mut is_long = tx.clone().with(|item: &str| {
                futures_util::future::ok::<bool, smpsc::mpsc::SendError<()>>(item.len() > 5)
            });

            is_positive.clone().send(-1).await.unwrap();
            is_long.clone().send("123456").await.unwrap();
            is_long.send("123").await.unwrap();
            is_positive.send(1).await.unwrap();
        }

        tx.send(false).await.unwrap();

        rx
    };

    assert_eq!(
        rx.collect::<Vec<_>>().await,
        vec![false, true, false, true, false]
    );
}

// test that a buffer is a no-nop around a sink that always accepts sends
#[cfg(feature = "alloc")]
#[tokio::test]
async fn buffer_noop() {
    let mut sink = Vec::new().buffer(0);
    sink.send(0).await.unwrap();
    sink.send(1).await.unwrap();
    assert_eq!(sink.get_ref(), &[0, 1]);

    let mut sink = Vec::new().buffer(1);
    sink.send(0).await.unwrap();
    sink.send(1).await.unwrap();
    assert_eq!(sink.get_ref(), &[0, 1]);
}

// test basic buffer functionality, including both filling up to capacity,
// and writing out when the underlying sink is ready
#[cfg(feature = "alloc")]
#[tokio::test]
async fn buffer() {
    let (sink, allow) = manual_allow::<i32>();
    let sink = sink.buffer(2);

    let sink = StartSendFut::new(sink, 0).await.unwrap();
    let mut sink = StartSendFut::new(sink, 1).await.unwrap();

    flag_cx(|flag, cx| {
        let mut task = sink.send(2);
        assert!(Pin::new(&mut task).poll(cx).is_pending());
        assert!(!flag.take());
        allow.start();
        assert!(flag.take());
        unwrap(Pin::new(&mut task).poll(cx));
        assert_eq!(sink.get_ref().data, vec![0, 1, 2]);
    })
}

#[cfg(feature = "alloc")]
#[tokio::test]
async fn fanout_smoke() {
    let sink1 = Vec::new();
    let sink2 = Vec::new();
    let mut sink = sink1.fanout(sink2);
    sink.send_all(&mut tokio_stream::iter(vec![1, 2, 3]).map(Ok))
        .await
        .unwrap();
    let (sink1, sink2) = sink.into_inner();
    assert_eq!(sink1, vec![1, 2, 3]);
    assert_eq!(sink2, vec![1, 2, 3]);
}

#[cfg(feature = "alloc")]
#[tokio::test]
async fn fanout_backpressure() {
    let (left_send, mut left_recv) = smpsc::mpsc::channel(1);
    let (right_send, mut right_recv) = smpsc::mpsc::channel(1);
    let sink = left_send.fanout(right_send);

    let mut sink = StartSendFut::new(sink, 0).await.unwrap();

    async_flag_cx(|flag, cx| {
        async move {
            let mut task = sink.send(2);
            assert!(!flag.take());
            assert!(Pin::new(&mut task).poll(cx).is_pending());
            assert_eq!(left_recv.next().await, Some(0));
            assert!(flag.take());
            assert!(Pin::new(&mut task).poll(cx).is_pending());
            assert_eq!(right_recv.next().await, Some(0));
            assert!(flag.take());

            assert!(Pin::new(&mut task).poll(cx).is_ready());
            assert_eq!(left_recv.next().await, Some(2));
            assert!(flag.take());
            assert!(Pin::new(&mut task).poll(cx).is_ready());
            assert_eq!(right_recv.next().await, Some(2));
            assert!(!flag.take());

            unwrap(Pin::new(&mut task).poll(cx));
            // make sure receivers live until end of test to prevent send errors
            drop(left_recv);
            drop(right_recv);
        }
    })
    .await;
}

#[cfg(feature = "alloc")]
#[tokio::test]
async fn sink_map_err() {
    {
        let cx = &mut futures_test::task::panic_context();
        let (tx, rx) = smpsc::mpsc::channel(1);
        let mut tx = tx.sink_map_err(|_| ());
        assert_eq!(Pin::new(&mut tx).start_send(()), Ok(()));
        drop(rx);
        assert_eq!(Pin::new(&mut tx).poll_flush(cx), Poll::Ready(Ok(())));
    }

    let tx = smpsc::mpsc::channel(1).0;
    assert_eq!(
        Pin::new(&mut tx.sink_map_err(|_| ())).start_send(()),
        Err(())
    );
}

#[cfg(feature = "alloc")]
#[tokio::test]
async fn sink_unfold() {
    core::future::poll_fn(|cx| {
        let (tx, mut rx) = smpsc::mpsc::channel(1);
        let unfold = async_sink::unfold((), |(), i: i32| {
            let tx = tx.clone();
            async move {
                tx.send(i).await.unwrap();
                Ok::<_, String>(())
            }
        });
        let mut unfold = core::pin::pin!(unfold);
        assert_eq!(unfold.as_mut().start_send(1), Ok(()));
        assert_eq!(unfold.as_mut().poll_flush(cx), Poll::Ready(Ok(())));
        assert_eq!(
            tokio_stream::Stream::poll_next(Pin::new(&mut rx), cx),
            Poll::Ready(Some(1))
        );

        assert_eq!(unfold.as_mut().poll_ready(cx), Poll::Ready(Ok(())));
        assert_eq!(unfold.as_mut().start_send(2), Ok(()));
        assert_eq!(unfold.as_mut().poll_ready(cx), Poll::Ready(Ok(())));
        assert_eq!(unfold.as_mut().start_send(3), Ok(()));
        assert_eq!(
            tokio_stream::Stream::poll_next(Pin::new(&mut rx), cx),
            Poll::Ready(Some(2))
        );
        assert_eq!(
            tokio_stream::Stream::poll_next(Pin::new(&mut rx), cx),
            Poll::Pending
        );
        assert_eq!(unfold.as_mut().poll_ready(cx), Poll::Ready(Ok(())));
        assert_eq!(unfold.as_mut().start_send(4), Ok(()));
        assert_eq!(unfold.as_mut().poll_flush(cx), Poll::Pending); // Channel full
        assert_eq!(
            tokio_stream::Stream::poll_next(Pin::new(&mut rx), cx),
            Poll::Ready(Some(3))
        );
        assert!(unfold.as_mut().poll_flush(cx).is_ready()); // last item has been sent to channel
        assert_eq!(
            tokio_stream::Stream::poll_next(Pin::new(&mut rx), cx),
            Poll::Ready(Some(4)),
        );

        Poll::Ready(())
    })
    .await
}

#[cfg(feature = "alloc")]
#[tokio::test]
async fn err_into() {
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct ErrIntoTest;

    impl core::fmt::Display for ErrIntoTest {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "ErrIntoTest")
        }
    }
    impl core::error::Error for ErrIntoTest {}

    impl From<smpsc::mpsc::SendError<()>> for ErrIntoTest {
        fn from(_e: smpsc::mpsc::SendError<()>) -> Self {
            ErrIntoTest
        }
    }

    {
        let cx = &mut futures_test::task::panic_context();
        let (tx, rx) = smpsc::mpsc::channel(1);
        let mut tx: async_sink::SinkErrInto<smpsc::mpsc::Sender<()>, _, ErrIntoTest> =
            tx.sink_err_into();
        assert_eq!(Pin::new(&mut tx).start_send(()), Ok(()));
        drop(rx);
        assert_eq!(Pin::new(&mut tx).poll_flush(cx), Poll::Ready(Ok(())));
    }

    let tx = smpsc::mpsc::channel(1).0;
    assert_eq!(
        Pin::new(&mut tx.sink_err_into()).start_send(()),
        Err(ErrIntoTest)
    );
}
