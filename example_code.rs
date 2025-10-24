// DO NOT TOUCH THIS FILE!!!

async fn handle_archive<R>(mut archive: Archive<R>)
where
    R: AsyncRead + Unpin,
{
    let mut entries = archive.entries().unwrap();
    'update: while let Some(file) = entries.next().await {
        match file {
            Ok(f) => {
                println!("{}", f.path().unwrap().display());
                //f.take(limit)
            }
            Err(e) => {
                eprintln!("Error reading archive entry: {e}");
                break 'update;
            }
        }
    }
}

/*
// reqwest gives Stream<Item = Result<Bytes, reqwest::Error>>
let byte_stream = resp.bytes_stream()
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

// StreamReader expects Stream<Item = Result<impl Buf, E>>
let reader = StreamReader::new(byte_stream);

// reader implements AsyncRead + AsyncBufRead + Unpin -> usable by tokio_tar
let archive = Archive::new(reader);
// Example usage of btrfs inside update_check:
// (Currently just demonstrate access to version)
println!("btrfs version in update_check: {}", btrfs.version());
Self::handle_archive(archive).await;
*/