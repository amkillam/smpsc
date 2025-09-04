#[cfg(feature = "alloc")]
#[tokio::test]
async fn it_works() {
    use async_sink::SinkExt;
    use smpsc::mpsc;
    use tokio_stream::{self as stream, StreamExt};

    let (tx1, rx1) = mpsc::channel(1);
    let (tx2, rx2) = mpsc::channel(2);
    let mut tx = tx1.fanout(tx2).sink_map_err(|_| ());

    let mut src = stream::iter((0..10).map(Ok));
    let local_set = tokio::task::LocalSet::new();
    local_set.spawn_local(async move {
        tx.send_all(&mut src).await.unwrap();
    });

    let collect_fut1 = local_set.spawn_local(rx1.collect::<Vec<_>>());
    let collect_fut2 = local_set.spawn_local(Box::pin(rx2.collect::<Vec<_>>()));
    local_set.await;
    let vec1 = collect_fut1.await.unwrap();
    let vec2 = collect_fut2.await.unwrap();

    let expected = (0..10).collect::<Vec<_>>();

    assert_eq!(vec1, expected.as_slice());
    assert_eq!(vec2, expected.as_slice());
}
