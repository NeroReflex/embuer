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

use embuer::hash_stream::HashingReader;
use sha2::{Digest, Sha512};
use tokio::io::{AsyncReadExt, BufReader};

/// Compute SHA512 hash directly (for comparison)
fn compute_hash_direct(data: &[u8]) -> String {
    let mut hasher = Sha512::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[tokio::test]
async fn test_hashing_reader_small_data() {
    let data = b"Hello, World!";
    let expected_hash = compute_hash_direct(data);

    let reader = std::io::Cursor::new(data);
    let mut hashing_reader = HashingReader::new(reader);
    let hash_result = hashing_reader.hash_result();

    // Read all data
    let mut buffer = Vec::new();
    let mut async_reader = BufReader::new(&mut hashing_reader);
    async_reader.read_to_end(&mut buffer).await.unwrap();

    // Wait a bit for hash to finalize
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // Check hash
    let hash = hash_result.read().await.clone();
    assert!(hash.is_some(), "Hash should be computed");
    assert_eq!(hash.as_ref().unwrap(), &expected_hash, "Hash should match direct computation");
}

#[tokio::test]
async fn test_hashing_reader_large_data() {
    // Create 100KB of data
    let data: Vec<u8> = (0..100 * 1024).map(|i| (i % 256) as u8).collect();
    let expected_hash = compute_hash_direct(&data);

    let reader = std::io::Cursor::new(&data);
    let mut hashing_reader = HashingReader::new(reader);
    let hash_result = hashing_reader.hash_result();

    // Read all data
    let mut buffer = Vec::new();
    let mut async_reader = BufReader::new(&mut hashing_reader);
    async_reader.read_to_end(&mut buffer).await.unwrap();

    // Wait a bit for hash to finalize
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // Check hash
    let hash = hash_result.read().await.clone();
    assert!(hash.is_some(), "Hash should be computed");
    assert_eq!(hash.as_ref().unwrap(), &expected_hash, "Hash should match direct computation");
}

#[tokio::test]
async fn test_hashing_reader_empty_data() {
    let data = b"";
    let expected_hash = compute_hash_direct(data);

    let reader = std::io::Cursor::new(data);
    let mut hashing_reader = HashingReader::new(reader);
    let hash_result = hashing_reader.hash_result();

    // Read all data
    let mut buffer = Vec::new();
    let mut async_reader = BufReader::new(&mut hashing_reader);
    async_reader.read_to_end(&mut buffer).await.unwrap();

    // Wait a bit for hash to finalize
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // Check hash
    let hash = hash_result.read().await.clone();
    assert!(hash.is_some(), "Hash should be computed even for empty data");
    assert_eq!(hash.as_ref().unwrap(), &expected_hash, "Hash should match direct computation");
}

#[tokio::test]
async fn test_hashing_reader_chunked_read() {
    let data: Vec<u8> = (0..8192).map(|i| (i % 256) as u8).collect();
    let expected_hash = compute_hash_direct(&data);

    let reader = std::io::Cursor::new(&data);
    let mut hashing_reader = HashingReader::new(reader);
    let hash_result = hashing_reader.hash_result();

    // Read in small chunks to simulate streaming
    let mut buffer = vec![0u8; 512];
    let mut async_reader = BufReader::new(&mut hashing_reader);
    let mut total_read = 0;
    
    loop {
        let n = async_reader.read(&mut buffer).await.unwrap();
        if n == 0 {
            break;
        }
        total_read += n;
    }

    assert_eq!(total_read, data.len(), "Should read all data");

    // Wait a bit for hash to finalize
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // Check hash
    let hash = hash_result.read().await.clone();
    assert!(hash.is_some(), "Hash should be computed");
    assert_eq!(hash.as_ref().unwrap(), &expected_hash, "Hash should match direct computation");
}

#[tokio::test]
async fn test_hashing_reader_multiple_reads() {
    let data = b"Test data for multiple reads";
    let expected_hash = compute_hash_direct(data);

    let reader = std::io::Cursor::new(data);
    let mut hashing_reader = HashingReader::new(reader);
    let hash_result = hashing_reader.hash_result();

    // Read in multiple small reads
    let mut async_reader = BufReader::new(&mut hashing_reader);
    let mut read_buf = [0u8; 5];
    let mut total_read = 0;
    
    loop {
        let n = async_reader.read(&mut read_buf).await.unwrap();
        if n == 0 {
            break;
        }
        total_read += n;
    }

    assert_eq!(total_read, data.len(), "Should read all data");

    // Wait a bit for hash to finalize
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // Check hash
    let hash = hash_result.read().await.clone();
    assert!(hash.is_some(), "Hash should be computed");
    assert_eq!(hash.as_ref().unwrap(), &expected_hash, "Hash should match direct computation");
}

#[tokio::test]
async fn test_hashing_reader_consistency() {
    // Test that hashing the same data multiple times gives the same result
    let data = b"Consistent hashing test data";
    
    let reader1 = std::io::Cursor::new(data);
    let mut hashing_reader1 = HashingReader::new(reader1);
    let hash_result1 = hashing_reader1.hash_result();

    let reader2 = std::io::Cursor::new(data);
    let mut hashing_reader2 = HashingReader::new(reader2);
    let hash_result2 = hashing_reader2.hash_result();

    // Read from both
    let mut async_reader1 = BufReader::new(&mut hashing_reader1);
    let mut async_reader2 = BufReader::new(&mut hashing_reader2);
    
    async_reader1.read_to_end(&mut Vec::new()).await.unwrap();
    async_reader2.read_to_end(&mut Vec::new()).await.unwrap();

    // Wait for finalization
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let hash1 = hash_result1.read().await.clone();
    let hash2 = hash_result2.read().await.clone();

    assert!(hash1.is_some() && hash2.is_some(), "Both hashes should be computed");
    assert_eq!(hash1, hash2, "Same data should produce same hash");
}

#[tokio::test]
async fn test_hashing_reader_different_data() {
    // Test that different data produces different hashes
    let data1 = b"First set of data";
    let data2 = b"Second set of data";
    
    let reader1 = std::io::Cursor::new(data1);
    let mut hashing_reader1 = HashingReader::new(reader1);
    let hash_result1 = hashing_reader1.hash_result();

    let reader2 = std::io::Cursor::new(data2);
    let mut hashing_reader2 = HashingReader::new(reader2);
    let hash_result2 = hashing_reader2.hash_result();

    // Read from both
    let mut async_reader1 = BufReader::new(&mut hashing_reader1);
    let mut async_reader2 = BufReader::new(&mut hashing_reader2);
    
    async_reader1.read_to_end(&mut Vec::new()).await.unwrap();
    async_reader2.read_to_end(&mut Vec::new()).await.unwrap();

    // Wait for finalization
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let hash1 = hash_result1.read().await.clone();
    let hash2 = hash_result2.read().await.clone();

    assert!(hash1.is_some() && hash2.is_some(), "Both hashes should be computed");
    assert_ne!(hash1, hash2, "Different data should produce different hashes");
}

