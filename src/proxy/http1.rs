//! HTTP/1.x head parsing and response framing used by the forward proxy.
//!
//! Parsing is delegated to `httparse` (zero-copy, tolerant of partial input) and
//! bodies are copied with explicit framing so the proxy can tell exactly where a
//! response ends. That is what makes upstream keep-alive reuse safe.

use std::io;
use thiserror::Error;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
};

/// Maximum size of a request/response head (status line plus headers).
pub const MAX_HEAD_BYTES: usize = 64 * 1024;
/// Maximum number of headers accepted in a head.
pub const MAX_HEADERS: usize = 100;
/// Maximum length of a single chunk-size or trailer line.
pub const MAX_LINE_BYTES: usize = 8 * 1024;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("too many headers")]
    TooManyHeaders,
    #[error("malformed HTTP head: {0}")]
    Malformed(String),
}

#[derive(Debug, Clone)]
pub struct Header {
    pub name: String,
    pub lower: String,
    pub value: Vec<u8>,
}

impl Header {
    fn new(name: &str, value: &[u8]) -> Self {
        Self {
            name: name.to_string(),
            lower: name.to_ascii_lowercase(),
            value: value.to_vec(),
        }
    }

    /// Header value as text (lossy for non-UTF-8 values).
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.value).unwrap_or("")
    }
}

/// Case-insensitive lookup helpers over a header list.
pub fn get<'a>(headers: &'a [Header], lower_name: &str) -> Option<&'a Header> {
    headers.iter().find(|h| h.lower == lower_name)
}

pub fn get_str<'a>(headers: &'a [Header], lower_name: &str) -> Option<&'a str> {
    get(headers, lower_name).map(|h| h.as_str().trim())
}

/// Returns true when `lower_name` contains `token` in a comma separated list.
pub fn has_token(headers: &[Header], lower_name: &str, token: &str) -> bool {
    headers.iter().filter(|h| h.lower == lower_name).any(|h| {
        h.as_str()
            .split(',')
            .any(|v| v.trim().eq_ignore_ascii_case(token))
    })
}

pub struct RequestParts {
    pub method: String,
    pub target: String,
    pub minor_version: u8,
    pub headers: Vec<Header>,
    /// Number of bytes consumed from the input buffer.
    pub consumed: usize,
}

pub struct ResponseParts {
    pub status: u16,
    pub minor_version: u8,
    pub headers: Vec<Header>,
}

pub enum Parsed<T> {
    Complete(T),
    Partial,
}

fn convert_headers(raw: &[httparse::Header<'_>]) -> Vec<Header> {
    raw.iter().map(|h| Header::new(h.name, h.value)).collect()
}

/// Parses a request head, returning [`Parsed::Partial`] when more bytes are needed.
pub fn parse_request(buf: &[u8]) -> Result<Parsed<RequestParts>, ParseError> {
    let mut slots = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut slots);

    match request.parse(buf) {
        Ok(httparse::Status::Complete(consumed)) => Ok(Parsed::Complete(RequestParts {
            method: request.method.unwrap_or_default().to_string(),
            target: request.path.unwrap_or_default().to_string(),
            minor_version: request.version.unwrap_or(1),
            headers: convert_headers(request.headers),
            consumed,
        })),
        Ok(httparse::Status::Partial) => Ok(Parsed::Partial),
        Err(httparse::Error::TooManyHeaders) => Err(ParseError::TooManyHeaders),
        Err(e) => Err(ParseError::Malformed(e.to_string())),
    }
}

/// Parses a response head, returning [`Parsed::Partial`] when more bytes are needed.
pub fn parse_response(buf: &[u8]) -> Result<Parsed<ResponseParts>, ParseError> {
    let mut slots = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut response = httparse::Response::new(&mut slots);

    match response.parse(buf) {
        Ok(httparse::Status::Complete(_)) => Ok(Parsed::Complete(ResponseParts {
            status: response.code.unwrap_or(0),
            minor_version: response.version.unwrap_or(1),
            headers: convert_headers(response.headers),
        })),
        Ok(httparse::Status::Partial) => Ok(Parsed::Partial),
        Err(httparse::Error::TooManyHeaders) => Err(ParseError::TooManyHeaders),
        Err(e) => Err(ParseError::Malformed(e.to_string())),
    }
}

/// How the body of a response is delimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    None,
    Length(usize),
    Chunked,
    UntilEof,
}

/// Determines body framing from the response head (RFC 9112 §6).
pub fn response_framing(method: &str, status: u16, headers: &[Header]) -> Framing {
    if method.eq_ignore_ascii_case("HEAD")
        || status == 204
        || status == 304
        || (100..200).contains(&status)
    {
        return Framing::None;
    }

    if has_token(headers, "transfer-encoding", "chunked") {
        return Framing::Chunked;
    }

    // A transfer coding other than chunked (or a chunked token we could not
    // find) leaves the message delimited by connection close (RFC 9112 §6.3).
    if get_str(headers, "transfer-encoding").is_some() {
        return Framing::UntilEof;
    }

    match get_str(headers, "content-length").and_then(|v| v.parse::<usize>().ok()) {
        Some(len) => Framing::Length(len),
        None => Framing::UntilEof,
    }
}

pub struct RelayOutcome {
    pub status: u16,
    /// Whether the upstream connection is still usable for another request.
    pub reusable: bool,
    /// Body bytes relayed (excluding the head).
    pub bytes: u64,
}

/// Relays one response (including any interim 1xx responses) to the client.
///
/// The client connection is always told `Connection: close` because this proxy
/// does not implement client-side keep-alive; hop-by-hop headers are stripped.
pub async fn relay_response<R, W>(
    reader: &mut R,
    writer: &mut W,
    request_method: &str,
) -> io::Result<RelayOutcome>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        let (raw, parts) = read_head_raw(reader).await?;
        let is_interim = (100..200).contains(&parts.status);
        write_head(writer, &raw, &parts.headers, !is_interim).await?;

        if is_interim {
            // Interim response: relay and keep reading for the final one.
            continue;
        }

        let framing = response_framing(request_method, parts.status, &parts.headers);
        let bytes = relay_body(reader, writer, framing).await?;

        let reusable =
            framing != Framing::UntilEof && keep_alive_allowed(parts.minor_version, &parts.headers);

        return Ok(RelayOutcome {
            status: parts.status,
            reusable,
            bytes,
        });
    }
}

fn keep_alive_allowed(minor_version: u8, headers: &[Header]) -> bool {
    if has_token(headers, "connection", "close") {
        return false;
    }

    if minor_version == 0 {
        // HTTP/1.0 defaults to close and needs an explicit opt-in.
        return has_token(headers, "connection", "keep-alive");
    }

    true
}

async fn read_head_raw<R>(reader: &mut R) -> io::Result<(Vec<u8>, ResponseParts)>
where
    R: AsyncBufRead + Unpin,
{
    let mut raw = Vec::with_capacity(512);

    loop {
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line).await?;

        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upstream closed before sending a response head",
            ));
        }

        raw.extend_from_slice(&line);

        if raw.len() > MAX_HEAD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "upstream response head too large",
            ));
        }

        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }

    match parse_response(&raw) {
        Ok(Parsed::Complete(parts)) => Ok((raw, parts)),
        Ok(Parsed::Partial) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete upstream response head",
        )),
        Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
    }
}

/// Rewrites the response head for the client: status line verbatim, hop-by-hop
/// headers dropped and, for the final response, `Connection: close` injected.
async fn write_head<W>(
    writer: &mut W,
    raw: &[u8],
    headers: &[Header],
    inject_close: bool,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    // The status line may end with CRLF or (leniently) a bare LF; matching on
    // LF only avoids re-emitting the headers when the terminator is not CRLF.
    let status_line_end = raw
        .iter()
        .position(|&byte| byte == b'\n')
        .map(|pos| pos + 1)
        .unwrap_or(raw.len());

    let mut out = Vec::with_capacity(raw.len() + 32);
    out.extend_from_slice(&raw[..status_line_end]);

    for header in headers {
        if is_hop_by_hop(&header.lower) {
            continue;
        }
        out.extend_from_slice(header.name.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(&header.value);
        out.extend_from_slice(b"\r\n");
    }

    if inject_close {
        out.extend_from_slice(b"Connection: close\r\n\r\n");
    } else {
        out.extend_from_slice(b"\r\n");
    }

    writer.write_all(&out).await
}

fn is_hop_by_hop(lower_name: &str) -> bool {
    matches!(
        lower_name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "upgrade"
    )
}

async fn relay_body<R, W>(reader: &mut R, writer: &mut W, framing: Framing) -> io::Result<u64>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let bytes = match framing {
        Framing::None => 0,
        Framing::Length(len) => copy_exactly(reader, writer, len).await?,
        Framing::Chunked => copy_chunked(reader, writer).await?,
        Framing::UntilEof => {
            let copied = tokio::io::copy(reader, writer).await?;
            writer.flush().await?;
            copied
        }
    };

    writer.flush().await?;
    Ok(bytes)
}

/// Copies exactly `len` bytes; errors if the peer closes early.
pub async fn copy_exactly<R, W>(reader: &mut R, writer: &mut W, len: usize) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut remaining = len as u64;
    let copied = remaining;
    let mut buffer = vec![0u8; 8192.min(len.max(1))];

    while remaining > 0 {
        let want = remaining.min(buffer.len() as u64) as usize;
        let read = reader.read(&mut buffer[..want]).await?;

        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before the declared body was complete",
            ));
        }

        writer.write_all(&buffer[..read]).await?;
        remaining -= read as u64;
    }

    Ok(copied)
}

/// Copies a chunked body verbatim (framing, data and trailers).
pub async fn copy_chunked<R, W>(reader: &mut R, writer: &mut W) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut total = 0u64;

    loop {
        let size_line = read_crlf_line(reader).await?;
        let chunk_size = parse_chunk_size(&size_line)?;
        writer.write_all(&size_line).await?;
        total += size_line.len() as u64;

        if chunk_size == 0 {
            loop {
                let trailer = read_crlf_line(reader).await?;
                let empty = trailer == b"\r\n" || trailer == b"\n";
                writer.write_all(&trailer).await?;
                total += trailer.len() as u64;

                if empty {
                    return Ok(total);
                }
            }
        }

        total += copy_exactly(reader, writer, chunk_size).await?;

        let terminator = read_crlf_line(reader).await?;
        if terminator != b"\r\n" && terminator != b"\n" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed chunk terminator",
            ));
        }
        writer.write_all(&terminator).await?;
        total += terminator.len() as u64;
    }
}

pub fn parse_chunk_size(line: &[u8]) -> io::Result<usize> {
    let text =
        std::str::from_utf8(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let size = text
        .trim_end_matches(['\r', '\n'])
        .split(';')
        .next()
        .unwrap_or("")
        .trim();

    if size.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty chunk size",
        ));
    }

    usize::from_str_radix(size, 16)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size"))
}

async fn read_crlf_line<R>(reader: &mut R) -> io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut line = Vec::with_capacity(64);
    let mut byte = [0u8; 1];

    loop {
        let read = reader.read(&mut byte).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed inside a chunked body",
            ));
        }

        line.push(byte[0]);

        if byte[0] == b'\n' {
            return Ok(line);
        }

        if line.len() > MAX_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "chunked body line too long",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[test]
    fn test_parse_request() {
        let raw =
            b"GET http://example.com/a?b=1 HTTP/1.1\r\nHost: example.com\r\nX-Test: v\r\n\r\nrest";
        let parsed = match parse_request(raw).unwrap() {
            Parsed::Complete(parts) => parts,
            Parsed::Partial => panic!("expected complete head"),
        };

        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.target, "http://example.com/a?b=1");
        assert_eq!(parsed.consumed, raw.len() - 4);
        assert_eq!(get_str(&parsed.headers, "host"), Some("example.com"));
        assert_eq!(get(&parsed.headers, "x-test").unwrap().name, "X-Test");
    }

    #[test]
    fn test_parse_request_partial() {
        assert!(matches!(
            parse_request(b"GET / HTTP/1.1\r\nHost: a").unwrap(),
            Parsed::Partial
        ));
    }

    #[test]
    fn test_parse_request_too_many_headers() {
        let mut raw = String::from("GET / HTTP/1.1\r\n");
        for i in 0..(MAX_HEADERS + 5) {
            raw.push_str(&format!("X-{i}: v\r\n"));
        }
        raw.push_str("\r\n");

        assert!(matches!(
            parse_request(raw.as_bytes()),
            Err(ParseError::TooManyHeaders)
        ));
    }

    #[test]
    fn test_response_framing() {
        let headers = vec![Header::new("Content-Length", b"12")];

        assert_eq!(response_framing("GET", 200, &headers), Framing::Length(12));
        assert_eq!(response_framing("HEAD", 200, &headers), Framing::None);
        assert_eq!(response_framing("GET", 204, &headers), Framing::None);
        assert_eq!(response_framing("GET", 304, &headers), Framing::None);
        assert_eq!(
            response_framing("GET", 200, &[Header::new("Transfer-Encoding", b"chunked")]),
            Framing::Chunked
        );
        assert_eq!(response_framing("GET", 200, &[]), Framing::UntilEof);
    }

    #[test]
    fn test_keep_alive_allowed() {
        assert!(keep_alive_allowed(1, &[]));
        assert!(!keep_alive_allowed(
            1,
            &[Header::new("Connection", b"close")]
        ));
        assert!(!keep_alive_allowed(0, &[]));
        assert!(keep_alive_allowed(
            0,
            &[Header::new("Connection", b"keep-alive")]
        ));
    }

    #[tokio::test]
    async fn test_relay_content_length() {
        let input = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: keep-alive\r\n\r\nhello";
        let mut reader = BufReader::new(&input[..]);
        let mut out = Vec::new();

        let outcome = relay_response(&mut reader, &mut out, "GET").await.unwrap();
        assert_eq!(outcome.status, 200);
        assert!(outcome.reusable);
        assert_eq!(outcome.bytes, 5);

        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Length: 5\r\n"));
        assert!(text.ends_with("Connection: close\r\n\r\nhello"));
        assert!(!text.to_ascii_lowercase().contains("connection: keep-alive"));
    }

    #[tokio::test]
    async fn test_relay_chunked_and_interim() {
        let input = b"HTTP/1.1 100 Continue\r\n\r\n\
                      HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                      3\r\nabc\r\n0\r\n\r\n";
        let mut reader = BufReader::new(&input[..]);
        let mut out = Vec::new();

        let outcome = relay_response(&mut reader, &mut out, "POST").await.unwrap();
        assert_eq!(outcome.status, 200);
        assert!(outcome.reusable);

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("100 Continue"));
        assert!(text.contains("3\r\nabc\r\n0\r\n\r\n"));
    }

    #[tokio::test]
    async fn test_relay_until_eof_is_not_reusable() {
        let input = b"HTTP/1.1 200 OK\r\n\r\nbody-without-length";
        let mut reader = BufReader::new(&input[..]);
        let mut out = Vec::new();

        let outcome = relay_response(&mut reader, &mut out, "GET").await.unwrap();
        assert_eq!(outcome.bytes, 19);
        assert!(!outcome.reusable);
    }

    #[tokio::test]
    async fn test_relay_head_request_has_no_body() {
        let input = b"HTTP/1.1 200 OK\r\nContent-Length: 42\r\n\r\n";
        let mut reader = BufReader::new(&input[..]);
        let mut out = Vec::new();

        let outcome = relay_response(&mut reader, &mut out, "HEAD").await.unwrap();
        assert_eq!(outcome.bytes, 0);
        assert!(outcome.reusable);
    }

    /// A lenient LF-only status line must not make the headers appear twice.
    #[tokio::test]
    async fn test_relay_lf_only_head_is_not_duplicated() {
        let input = b"HTTP/1.1 200 OK\nContent-Length: 2\n\nok";
        let mut reader = BufReader::new(&input[..]);
        let mut out = Vec::new();

        let outcome = relay_response(&mut reader, &mut out, "GET").await.unwrap();
        assert_eq!(outcome.bytes, 2);

        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.matches("Content-Length").count(), 1, "text: {text:?}");
        assert!(text.starts_with("HTTP/1.1 200 OK\n"));
        assert!(text.ends_with("Connection: close\r\n\r\nok"));
    }

    /// Interim responses must not carry the final `Connection: close`.
    #[tokio::test]
    async fn test_relay_interim_has_no_close_header() {
        let input = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        let mut reader = BufReader::new(&input[..]);
        let mut out = Vec::new();

        relay_response(&mut reader, &mut out, "POST").await.unwrap();

        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text.matches("Connection: close").count(),
            1,
            "text: {text:?}"
        );
        assert!(text.starts_with("HTTP/1.1 100 Continue\r\n\r\n"));
    }

    #[test]
    fn test_response_framing_transfer_encoding_not_chunked() {
        let headers = vec![Header::new("Transfer-Encoding", b"gzip")];
        assert_eq!(response_framing("GET", 200, &headers), Framing::UntilEof);
    }
}
