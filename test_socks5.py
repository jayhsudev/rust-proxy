#!/usr/bin/env python3
import socket
import sys

def test_socks5_handshake():
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.connect(('127.0.0.1', 1082))

    # Send handshake: version 5, 1 method, method 0 (no auth)
    sock.send(b'\x05\x01\x00')

    # Receive response: version 5, selected method
    response = sock.recv(2)
    print(f"Handshake response: {response}")
    if response != b'\x05\x00':
        print("Handshake failed")
        return False

    # Send request: version 5, command 1 (connect), reserved 0, address type 1 (IPv4), address 127.0.0.1, port 80
    request = b'\x05\x01\x00\x01\x7f\x00\x00\x01\x00\x50'
    sock.send(request)

    # Receive response
    response = sock.recv(10)
    print(f"Connect response: {response}")
    if len(response) < 10:
        print("Incomplete response")
        return False
    if response[1] != 0x00:
        print(f"Connect failed with code {response[1]}")
        return False

    print("SOCKS5 proxy test passed")
    sock.close()
    return True

if __name__ == '__main__':
    success = test_socks5_handshake()
    sys.exit(0 if success else 1)