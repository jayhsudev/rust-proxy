// Test SOCKS5 proxy handshake
// Usage: cargo run --bin test_socks5

use std::io::{Read, Write};
use std::net::TcpStream;

fn main() {
    match test_socks5_handshake() {
        Ok(()) => {
            println!("SOCKS5 proxy handshake test passed");
            std::process::exit(0);
        }
        Err(e) => {
            println!("SOCKS5 proxy test failed: {}", e);
            std::process::exit(1);
        }
    }
}

fn test_socks5_handshake() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing SOCKS5 proxy handshake...");

    // Try to connect to the proxy server (port 1080 by default)
    println!("Connecting to SOCKS5 proxy at 127.0.0.1:1080...");

    let mut sock = TcpStream::connect("127.0.0.1:1080")?;
    println!("Connected successfully");

    // Send SOCKS5 handshake: version 5, 1 method, method 0 (no authentication)
    sock.write_all(b"\x05\x01\x00")?;
    println!("Handshake request sent: 0x05 0x01 0x00");

    // Receive handshake response
    let mut response = [0u8; 2];
    sock.read_exact(&mut response)?;
    println!(
        "Handshake response received: {:02x} {:02x}",
        response[0], response[1]
    );

    if response != [0x05, 0x00] {
        return Err(format!("Invalid handshake response: {:?}", response).into());
    }

    println!("SOCKS5 proxy handshake successful!");

    Ok(())
}
