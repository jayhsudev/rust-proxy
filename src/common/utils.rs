use base64::{engine::general_purpose, Engine as _};
use std::io::{self, Read};
use std::net::TcpStream;
use std::time::Duration;

/// Set timeout for TCP connection
#[allow(dead_code)]
pub fn set_tcp_timeout(stream: &TcpStream, timeout: Duration) -> io::Result<()> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(())
}

/// Read specified number of bytes from stream
#[allow(dead_code)]
pub fn read_exact(stream: &mut TcpStream, buf: &mut [u8]) -> io::Result<()> {
    let mut bytes_read = 0;
    while bytes_read < buf.len() {
        match stream.read(&mut buf[bytes_read..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Connection closed prematurely",
                ))
            }
            Ok(n) => bytes_read += n,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Decode Base64 string
#[allow(dead_code)]
pub fn base64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    general_purpose::STANDARD.decode(s)
}

/// Encode to Base64 string
#[allow(dead_code)]
pub fn base64_encode(bytes: &[u8]) -> String {
    general_purpose::STANDARD.encode(bytes)
}
