#![allow(unused)]
use futures::sync::oneshot;
use futures::prelude::{async, await};
use futures::{Stream, Sink, Poll};

use crypto::identity::PublicKey;

/// A struct that reports when it is dropped.
struct Tracked<T> {
    inner: T,
    opt_drop_sender: Option<oneshot::Sender<()>>,
}

impl<T> Tracked<T> {
    pub fn new(inner: T, drop_sender: oneshot::Sender<()>) -> Tracked<T>  {
        Tracked {
            inner,
            opt_drop_sender: Some(drop_sender),
        }
    }
}

impl<T> Stream for Tracked<T> where T: Stream {
    type Item = T::Item;
    type Error = T::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.inner.poll()
    }
}

impl<T> Drop for Tracked<T> {
    fn drop(&mut self) {
        match self.opt_drop_sender.take() {
            Some(drop_sender) => {
                let _ = drop_sender.send(());
            },
            None => {},
        };
    }
}


#[async]
fn conn_limiter<M,K,ME,KE,T,TE>(
                incoming_conns: T,
                max_conns: usize) -> Result<(),()>
where
    T: Stream<Item=(M, K, PublicKey), Error=TE>,
    M: Stream<Item=Vec<u8>, Error=ME>,
    K: Sink<SinkItem=Vec<u8>, SinkError=KE>,
{
    let mut cur_conns: usize = 0;
    unimplemented!();
}
