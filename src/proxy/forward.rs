use std::time::Duration;
use tokio::io;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::net::conn::BufferedConnection;

/// Error types for connection operations
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    #[error("Failed to resolve address: {0}")]
    AddressResolutionFailed(String),
    #[error("Connection timed out")]
    ConnectionTimeout,
    #[error("Connection refused: {0}")]
    ConnectionRefused(String),
    #[error("Target address not found")]
    AddressNotFound,
}

/// Resolve a host:port address to a SocketAddr
pub async fn resolve_address(addr: &str) -> Result<std::net::SocketAddr, ConnectError> {
    tokio::net::lookup_host(addr)
        .await
        .map_err(|e| ConnectError::AddressResolutionFailed(e.to_string()))?
        .next()
        .ok_or(ConnectError::AddressNotFound)
}

/// Connect to a target address with timeout
pub async fn connect_with_timeout(
    addr: &str,
    connect_timeout: Duration,
) -> Result<TcpStream, ConnectError> {
    let target_addr = resolve_address(addr).await?;
    timeout(connect_timeout, TcpStream::connect(target_addr))
        .await
        .map_err(|_| ConnectError::ConnectionTimeout)?
        .map_err(|e| ConnectError::ConnectionRefused(e.to_string()))
}

/// Bidirectional data forwarding between two connections using zero-copy I/O.
pub async fn forward_bidirectional(
    conn1: &mut BufferedConnection,
    conn2: &mut BufferedConnection,
) -> io::Result<()> {
    let (c2s, s2c) = tokio::io::copy_bidirectional(conn1, conn2).await?;
    log::debug!(
        "Forwarded {} bytes client->target, {} bytes target->client",
        c2s,
        s2c,
    );
    Ok(())
}
