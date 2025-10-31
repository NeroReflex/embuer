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
        let filled = buf.filled().to_vec();
        let before = filled.len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);

        if let Poll::Ready(Ok(())) = &result {
            // Update hasher with the newly read data
            let newly_read = &filled[before..];
            if !newly_read.is_empty() {
                self.hasher.update(newly_read);
            }

            // Check if we've reached EOF (no new data and buffer is at capacity)
            if newly_read.is_empty() && buf.remaining() == 0 {
                // Finalize the hash when stream ends
                self.finalize_hash();
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
