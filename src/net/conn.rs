use socket2::{SockRef, TcpKeepalive};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// Probe interval for TCP keepalive (unix only; Windows uses its own default).
#[cfg(unix)]
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// Applies latency and liveness tuning to a TCP stream.
///
/// Keepalive makes abandoned peers (sleep, NAT timeout, cable pull) discoverable
/// instead of leaking a socket and a connection permit for hours.
/// Failing to enable it is not fatal: the connection stays usable.
pub fn tune_stream(stream: &TcpStream, keepalive: Option<Duration>) -> io::Result<()> {
    stream.set_nodelay(true)?;

    if let Some(idle) = keepalive {
        let keepalive = TcpKeepalive::new().with_time(idle);
        #[cfg(unix)]
        let keepalive = keepalive.with_interval(KEEPALIVE_INTERVAL).with_retries(3);

        if let Err(e) = SockRef::from(stream).set_tcp_keepalive(&keepalive) {
            log::debug!("Failed to enable TCP keepalive: {}", e);
        }
    }

    Ok(())
}

pub struct BufferedConnection {
    stream: TcpStream,
    read_buffer: Vec<u8>,
    temp_buffer: Vec<u8>,
    buffer_size: usize,
}

impl BufferedConnection {
    pub fn new(stream: TcpStream, buffer_size: usize) -> Self {
        BufferedConnection {
            stream,
            read_buffer: Vec::with_capacity(buffer_size),
            temp_buffer: vec![0u8; buffer_size],
            buffer_size,
        }
    }

    pub async fn read(&mut self) -> io::Result<usize> {
        let n = self.stream.read(&mut self.temp_buffer).await?;
        if n > 0 {
            self.read_buffer.extend_from_slice(&self.temp_buffer[..n]);
        }
        Ok(n)
    }

    pub fn read_from_buffer(&mut self, len: usize) -> Option<Vec<u8>> {
        if self.read_buffer.len() >= len {
            let data = self.read_buffer[..len].to_vec();
            self.read_buffer.drain(..len);
            Some(data)
        } else {
            None
        }
    }

    pub fn buffer_slice(&self, len: usize) -> Option<&[u8]> {
        if self.read_buffer.len() >= len {
            Some(&self.read_buffer[..len])
        } else {
            None
        }
    }

    pub async fn ensure_bytes(&mut self, n: usize) -> io::Result<()> {
        while self.read_buffer.len() < n {
            if self.read().await? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Connection closed",
                ));
            }
        }
        Ok(())
    }

    pub async fn read_exact_bytes(&mut self, n: usize) -> io::Result<Vec<u8>> {
        self.ensure_bytes(n).await?;
        self.read_from_buffer(n)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "Buffer underflow"))
    }

    pub async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.stream.write_all(data).await
    }

    pub fn unread(&mut self, data: &[u8]) {
        let mut new_buffer = Vec::with_capacity(data.len() + self.read_buffer.len());
        new_buffer.extend_from_slice(data);
        new_buffer.extend_from_slice(&self.read_buffer);
        self.read_buffer = new_buffer;
    }

    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    pub fn buffer_len(&self) -> usize {
        self.read_buffer.len()
    }

    /// Splits the connection into independently owned halves without any locking.
    ///
    /// Bytes already buffered (for example the first TLS record a client sends
    /// right after a CONNECT request) are kept by the read half, so no data is
    /// lost when switching to relay mode.
    pub fn into_split(self) -> (BufferedReadHalf, BufferedWriteHalf) {
        let (reader, writer) = self.stream.into_split();
        (
            BufferedReadHalf {
                reader,
                buffer: self.read_buffer,
                pos: 0,
            },
            BufferedWriteHalf { writer },
        )
    }
}

/// Read half of a [`BufferedConnection`]: drains buffered bytes first.
pub struct BufferedReadHalf {
    reader: OwnedReadHalf,
    buffer: Vec<u8>,
    pos: usize,
}

/// Write half of a [`BufferedConnection`].
pub struct BufferedWriteHalf {
    writer: OwnedWriteHalf,
}

impl AsyncRead for BufferedReadHalf {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        let remaining = this.buffer.len().saturating_sub(this.pos);
        if remaining > 0 {
            let to_copy = remaining.min(buf.remaining());
            buf.put_slice(&this.buffer[this.pos..this.pos + to_copy]);
            this.pos += to_copy;

            if this.pos >= this.buffer.len() {
                this.buffer.clear();
                this.pos = 0;
            }

            return Poll::Ready(Ok(()));
        }

        Pin::new(&mut this.reader).poll_read(cx, buf)
    }
}

impl AsyncWrite for BufferedWriteHalf {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().writer).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_shutdown(cx)
    }
}

/// Residual data in the read buffer is drained first before delegating to
/// the underlying stream, so that `tokio::io::copy_bidirectional` works
/// correctly after protocol negotiation.
impl AsyncRead for BufferedConnection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        if !this.read_buffer.is_empty() {
            let to_copy = std::cmp::min(this.read_buffer.len(), buf.remaining());
            buf.put_slice(&this.read_buffer[..to_copy]);
            this.read_buffer.drain(..to_copy);
            return Poll::Ready(Ok(()));
        }

        Pin::new(&mut this.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for BufferedConnection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_buffered_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let mut client_conn = BufferedConnection::new(client_stream, 4096);

        let (server_stream, _) = listener.accept().await.unwrap();
        let mut server_conn = BufferedConnection::new(server_stream, 4096);

        client_conn.write(b"Hello, server!").await.unwrap();

        let data = server_conn.read_exact_bytes(14).await.unwrap();
        assert_eq!(data, b"Hello, server!");

        server_conn.write(b"Hello, client!").await.unwrap();

        let data = client_conn.read_exact_bytes(14).await.unwrap();
        assert_eq!(data, b"Hello, client!");
    }

    #[tokio::test]
    async fn test_async_read_drains_buffer() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let mut client_conn = BufferedConnection::new(client_stream, 4096);

        let (server_stream, _) = listener.accept().await.unwrap();
        let mut server_conn = BufferedConnection::new(server_stream, 4096);

        client_conn.write(b"buffered-data").await.unwrap();
        drop(client_conn);

        // Force part of the payload into the internal buffer.
        let _ = server_conn.read_exact_bytes(4).await.unwrap();

        let mut out = Vec::new();
        tokio::io::copy(&mut server_conn, &mut out).await.unwrap();
        assert_eq!(out, b"ered-data");
    }

    #[tokio::test]
    async fn test_into_split_keeps_buffered_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let mut client = TcpStream::connect(addr).await.unwrap();
        let (server_stream, _) = listener.accept().await.unwrap();
        let mut conn = BufferedConnection::new(server_stream, 4096);

        client.write_all(b"prefix-after-handshake").await.unwrap();
        client.shutdown().await.unwrap();

        // Handshake style read leaves the rest inside the internal buffer.
        let first = conn.read_exact_bytes(6).await.unwrap();
        assert_eq!(first, b"prefix");

        let (mut reader, mut writer) = conn.into_split();
        let mut out = Vec::new();
        tokio::io::copy(&mut reader, &mut out).await.unwrap();
        assert_eq!(out, b"-after-handshake");

        // The write half still works after the split.
        assert!(writer.shutdown().await.is_ok());
    }

    #[tokio::test]
    async fn test_tune_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let client = TcpStream::connect(addr).await.unwrap();
        tune_stream(&client, Some(Duration::from_secs(60))).unwrap();
        tune_stream(&client, None).unwrap();
        assert!(client.peer_addr().is_ok());
    }

    #[tokio::test]
    async fn test_unread() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let mut client_conn = BufferedConnection::new(client_stream, 4096);

        let (server_stream, _) = listener.accept().await.unwrap();
        let mut server_conn = BufferedConnection::new(server_stream, 4096);

        client_conn.write(b"\x05\x01\x00").await.unwrap();

        let first = server_conn.read_exact_bytes(1).await.unwrap();
        assert_eq!(first[0], 0x05);

        server_conn.unread(&first);
        assert!(server_conn.buffer_len() > 0);

        let all = server_conn.read_exact_bytes(3).await.unwrap();
        assert_eq!(all, b"\x05\x01\x00");
    }
}
