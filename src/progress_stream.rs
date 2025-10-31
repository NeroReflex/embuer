use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::RwLock,
};

use crate::status::UpdateStatus;

/// A wrapper around AsyncRead that tracks progress
pub struct ProgressReader<R> {
    inner: R,
    bytes_read: u64,
    total_size: Option<u64>,
    status_handle: Arc<RwLock<UpdateStatus>>,
    source: String,
    last_update: std::time::Instant,
}

impl<R: AsyncRead + Unpin> ProgressReader<R> {
    pub fn new(
        inner: R,
        total_size: Option<u64>,
        status_handle: Arc<RwLock<UpdateStatus>>,
        source: String,
    ) -> Self {
        Self {
            inner,
            bytes_read: 0,
            total_size,
            status_handle,
            source,
            last_update: std::time::Instant::now(),
        }
    }

    fn calculate_progress(&self) -> i32 {
        self.total_size
            .filter(|&size| size > 0)
            .map(|size| ((self.bytes_read as f64 / size as f64) * 100.0) as i32)
            .unwrap_or(-1)
    }

    fn should_update(&self) -> bool {
        // Update if 100ms has elapsed since last update
        self.last_update.elapsed().as_millis() > 100
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ProgressReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);

        if let Poll::Ready(Ok(())) = &result {
            let reader = self.get_mut();
            let bytes_increment = (buf.filled().len() - before) as u64;
            let was_zero = reader.bytes_read == 0;
            reader.bytes_read += bytes_increment;

            // Update immediately on first read to show progress has started
            // Then continue updating every 100ms
            if was_zero || reader.should_update() {
                reader.last_update = std::time::Instant::now();
                let progress = reader.calculate_progress();
                let status_handle = reader.status_handle.clone();
                let source = reader.source.clone();

                tokio::spawn(async move {
                    let mut status = status_handle.write().await;
                    *status = UpdateStatus::Installing { source, progress };
                });
            }
        }

        result
    }
}
