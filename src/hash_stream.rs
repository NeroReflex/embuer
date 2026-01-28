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

use sha2::{Digest, Sha512};
use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::RwLock,
};

/// A wrapper around AsyncRead that computes SHA512 hash incrementally
/// The hash result is stored in an Arc for retrieval after streaming completes
pub struct HashingReader<R> {
    inner: R,
    hasher: Sha512,
    hash_result: Arc<RwLock<Option<String>>>,
}

impl<R: AsyncRead + Unpin> HashingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha512::new(),
            hash_result: Arc::new(RwLock::new(None)),
        }
    }

    pub fn hash_result(&self) -> Arc<RwLock<Option<String>>> {
        self.hash_result.clone()
    }

    /// Finalize the hash and return it directly
    /// Call this after the stream has been fully consumed
    pub async fn get_hash(&mut self) -> Option<String> {
        let hash = std::mem::replace(&mut self.hasher, Sha512::new()).finalize();
        let hex_hash = hex::encode(hash);
        if let Ok(mut result) = self.hash_result.try_write() {
            *result = Some(hex_hash.clone());
        }
        Some(hex_hash)
    }

    fn finalize_hash(&mut self) {
        let hash = std::mem::replace(&mut self.hasher, Sha512::new()).finalize();
        let hex_hash = hex::encode(hash);
        if let Ok(mut result) = self.hash_result.try_write() {
            *result = Some(hex_hash);
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for HashingReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // Record how much was filled before the read
        let filled_before = buf.filled().len();

        // Perform the actual read
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);

        match &result {
            Poll::Ready(Ok(())) => {
                // Get the newly read data (everything that was filled after the read)
                let filled_after = buf.filled();
                if filled_after.len() > filled_before {
                    let newly_read = &filled_after[filled_before..];
                    if !newly_read.is_empty() {
                        self.hasher.update(newly_read);
                    }
                }

                // Check if we've reached EOF (no new data read - means EOF was reached)
                if filled_after.len() == filled_before {
                    // Finalize the hash when stream ends (EOF)
                    self.finalize_hash();
                }
            }
            Poll::Ready(Err(_)) => {
                // On error, finalize what we have
                self.finalize_hash();
            }
            Poll::Pending => {
                // Not ready yet, do nothing
            }
        }

        result
    }
}

impl<R> std::fmt::Debug for HashingReader<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HashingReader")
    }
}

impl<R> Unpin for HashingReader<R> {}
