//! The write side of a connection: a task that writes queued messages in
//! order, so the session task never blocks on a slow client. Flow control
//! (≤ 4 video frames in flight) bounds what can pile up.

use std::time::Duration;

use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

enum Message {
    Data(Vec<u8>),
    /// Flush what is queued, send TLS close_notify, close.
    Close,
}

/// Queues encoded frames for the writer task. Cheap to clone.
#[derive(Clone)]
pub(crate) struct Outbox {
    tx: UnboundedSender<Message>,
}

impl Outbox {
    pub fn spawn<W: AsyncWrite + Unpin + Send + 'static>(writer: W) -> (Outbox, JoinHandle<()>) {
        let (tx, rx) = unbounded_channel();
        (Outbox { tx }, tokio::spawn(write_loop(writer, rx)))
    }

    pub fn send(&self, data: Vec<u8>) {
        let _ = self.tx.send(Message::Data(data));
    }

    /// Everything queued before this is still written; later sends are dropped.
    pub fn close(&self) {
        let _ = self.tx.send(Message::Close);
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(mut writer: W, mut rx: UnboundedReceiver<Message>) {
    'outer: while let Some(first) = rx.recv().await {
        let mut next = Some(first);
        // Write everything already queued, then flush once.
        while let Some(message) = next {
            match message {
                Message::Data(data) => {
                    if writer.write_all(&data).await.is_err() {
                        break 'outer;
                    }
                }
                Message::Close => {
                    // Do not wait forever on a peer that stopped reading.
                    let _ = tokio::time::timeout(Duration::from_secs(2), async {
                        let _ = writer.flush().await;
                        let _ = writer.shutdown().await;
                    })
                    .await;
                    break 'outer;
                }
            }
            next = rx.try_recv().ok();
        }
        if writer.flush().await.is_err() {
            break;
        }
    }
}
