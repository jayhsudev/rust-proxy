use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream as TokioTcpStream;

/// 带缓冲的连接
pub struct BufferedConnection {
    /// 底层TCP流
    stream: TokioTcpStream,
    /// 读缓冲区
    read_buffer: Vec<u8>,
    /// 写缓冲区
    write_buffer: Vec<u8>,
    /// 缓冲区大小
    buffer_size: usize,
}

impl BufferedConnection {
    /// 创建新的带缓冲连接
    pub fn new(stream: TokioTcpStream, buffer_size: usize) -> Self {
        BufferedConnection {
            stream,
            read_buffer: Vec::with_capacity(buffer_size),
            write_buffer: Vec::with_capacity(buffer_size),
            buffer_size,
        }
    }

    /// 从连接读取数据到缓冲区
    pub async fn read(&mut self) -> io::Result<usize> {
        let mut temp_buffer = vec![0; self.buffer_size];
        let n = self.stream.read(&mut temp_buffer).await?;
        if n > 0 {
            self.read_buffer.extend_from_slice(&temp_buffer[..n]);
        }
        Ok(n)
    }

    /// 从缓冲区读取指定数量的字节
    pub fn read_from_buffer(&mut self, len: usize) -> Option<Vec<u8>> {
        if self.read_buffer.len() >= len {
            let data = self.read_buffer[..len].to_vec();
            self.read_buffer.drain(..len);
            Some(data)
        } else {
            None
        }
    }

    /// 写入数据到缓冲区
    pub fn write_to_buffer(&mut self, data: &[u8]) {
        self.write_buffer.extend_from_slice(data);
    }

    /// 刷新缓冲区，将数据写入连接
    pub async fn flush(&mut self) -> io::Result<()> {
        if !self.write_buffer.is_empty() {
            self.stream.write_all(&self.write_buffer).await?;
            self.write_buffer.clear();
        }
        Ok(())
    }

    /// 直接写入数据到连接
    pub async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.stream.write_all(data).await
    }

    /// 关闭连接
    #[allow(dead_code)]
    pub async fn close(&mut self) -> io::Result<()> {
        self.stream.shutdown().await
    }

    /// 获取底层TCP流
    #[allow(dead_code)]
    pub fn into_inner(self) -> TokioTcpStream {
        self.stream
    }

    /// 检查读缓冲区中是否有数据
    pub fn has_data(&self) -> bool {
        !self.read_buffer.is_empty()
    }

    /// 获取缓冲区大小
    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    /// 获取读缓冲区中的数据长度
    pub fn available_bytes(&self) -> usize {
        self.read_buffer.len()
    }
}

/// 连接方向
#[allow(dead_code)]
pub enum ConnectionDirection {
    /// 客户端到代理
    ClientToProxy,
    /// 代理到服务器
    ProxyToServer,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio::runtime::Runtime;

    #[test]
    fn test_buffered_connection() {
        let rt = Runtime::new().unwrap();

        rt.block_on(async {
            // 启动测试服务器
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();

            // 客户端连接
            let client_stream = TokioTcpStream::connect(addr).await.unwrap();
            let mut client_conn = BufferedConnection::new(client_stream, 4096);

            // 服务器接受连接
            let (server_stream, _) = listener.accept().await.unwrap();
            let mut server_conn = BufferedConnection::new(server_stream, 4096);

            // 客户端发送数据
            client_conn.write_to_buffer(b"Hello, server!");
            client_conn.flush().await.unwrap();

            // 服务器读取数据
            server_conn.read().await.unwrap();
            let data = server_conn.read_from_buffer(14).unwrap();
            assert_eq!(data, b"Hello, server!");

            // 服务器回应
            server_conn.write_to_buffer(b"Hello, client!");
            server_conn.flush().await.unwrap();

            // 客户端读取回应
            client_conn.read().await.unwrap();
            let data = client_conn.read_from_buffer(14).unwrap();
            assert_eq!(data, b"Hello, client!");
        });
    }
}
