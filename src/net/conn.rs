use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;

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

    #[allow(dead_code)]
    pub fn buffer_slice(&self, len: usize) -> Option<&[u8]> {
        if self.read_buffer.len() >= len {
            Some(&self.read_buffer[..len])
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub fn drain_buffer(&mut self, len: usize) -> bool {
        if self.read_buffer.len() >= len {
            self.read_buffer.drain(..len);
            true
        } else {
            false
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

    /// Returns the line content without the trailing `\r\n`.
    pub async fn read_line(&mut self) -> io::Result<String> {
        loop {
            if let Some(pos) = self.read_buffer.windows(2).position(|w| w == b"\r\n") {
                let line = String::from_utf8(self.read_buffer[..pos].to_vec())
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                self.read_buffer.drain(..pos + 2);
                return Ok(line);
            }
            if self.read().await? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Connection closed before line terminator",
                ));
            }
        }
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

    pub fn has_data(&self) -> bool {
        !self.read_buffer.is_empty()
    }

    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    #[allow(dead_code)]
    pub fn buffer_len(&self) -> usize {
        self.read_buffer.len()
    }

    #[allow(dead_code)]
    pub fn clear_buffer(&mut self) {
        self.read_buffer.clear();
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
    async fn test_read_line() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let mut client_conn = BufferedConnection::new(client_stream, 4096);

        let (server_stream, _) = listener.accept().await.unwrap();
        let mut server_conn = BufferedConnection::new(server_stream, 4096);

        client_conn
            .write(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();

        let line1 = server_conn.read_line().await.unwrap();
        assert_eq!(line1, "GET / HTTP/1.1");

        let line2 = server_conn.read_line().await.unwrap();
        assert_eq!(line2, "Host: example.com");

        let line3 = server_conn.read_line().await.unwrap();
        assert_eq!(line3, "");
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
        assert!(server_conn.has_data());

        let all = server_conn.read_exact_bytes(3).await.unwrap();
        assert_eq!(all, b"\x05\x01\x00");
    }
}
