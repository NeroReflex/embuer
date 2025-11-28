/*
    embuer: an embedded software updater DBUS daemon and CLI interface
    Copyright (C) 2025  Denis Benato
    
    This program is free software; you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation; either version 2 of the License, or
    (at your option) any later version.
    
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.
    
    You should have received a copy of the GNU General Public License along
    with this program; if not, write to the Free Software Foundation, Inc.,
    51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA.
*/

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
        log::debug!("[PROGRESS] ProgressReader::new: total_size={:?}, source={}", total_size, source);
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
        log::debug!("[PROGRESS] ProgressReader::poll_read called: before={}, capacity={}", before, buf.capacity());
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);

        if let Poll::Ready(Ok(())) = &result {
            let reader = self.get_mut();
            let bytes_increment = (buf.filled().len() - before) as u64;
            let was_zero = reader.bytes_read == 0;
            reader.bytes_read += bytes_increment;

            log::debug!(
                "[PROGRESS] ProgressReader::poll_read: bytes_read={}, increment={}, was_zero={}, total_size={:?}",
                reader.bytes_read,
                bytes_increment,
                was_zero,
                reader.total_size
            );

            // Update immediately on first read to show progress has started
            // Then continue updating every 100ms
            if was_zero || reader.should_update() {
                reader.last_update = std::time::Instant::now();
                let progress = reader.calculate_progress();
                let status_handle = reader.status_handle.clone();
                let source = reader.source.clone();

                log::debug!(
                    "[PROGRESS] ProgressReader: Spawning status update task: progress={}%, bytes_read={}/{:?}",
                    progress,
                    reader.bytes_read,
                    reader.total_size
                );

                tokio::spawn(async move {
                    let mut status = status_handle.write().await;
                    log::debug!("[PROGRESS] Status update task: Setting status to Installing with progress={}%", progress);
                    *status = UpdateStatus::Installing { source, progress };
                });
            } else {
                log::debug!(
                    "[PROGRESS] ProgressReader: Skipping update (throttled): elapsed={}ms, should_update={}",
                    reader.last_update.elapsed().as_millis(),
                    reader.should_update()
                );
            }
        } else if let Poll::Ready(Err(ref e)) = &result {
            log::warn!("[PROGRESS] ProgressReader::poll_read: Error from inner reader: {}", e);
        } else {
            log::debug!("[PROGRESS] ProgressReader::poll_read: Poll::Pending - waiting for more data");
        }

        result
    }
}
