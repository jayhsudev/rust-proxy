use std::io;

use crate::net::conn::BufferedConnection;

/// 数据转发器
pub struct Forwarder;

impl Forwarder {
    /// 在两个连接之间转发数据
    pub async fn forward_between(
        conn1: &mut BufferedConnection,
        conn2: &mut BufferedConnection,
    ) -> Result<(), io::Error> {
        // 使用tokio的select!宏来同时监听两个连接
        loop {
            tokio::select! {
                // 从conn1读取并写入到conn2
                result = conn1.read() => {
                    match result {
                        Ok(0) => {
                            // 连接关闭
                            return Ok(());
                        }
                        Ok(n) => {
                            // 有数据可读，从缓冲区读取并写入到conn2
                            match conn1.read_from_buffer(n) {
                                Some(data) => {
                                    conn2.write_to_buffer(&data);
                                    conn2.flush().await?;
                                }
                                None => {
                                    // This should not happen if read() succeeded
                                    return Err(io::Error::new(
                                        io::ErrorKind::UnexpectedEof,
                                        "Buffer data mismatch"
                                    ));
                                }
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }
                // 从conn2读取并写入到conn1
                result = conn2.read() => {
                    match result {
                        Ok(0) => {
                            // 连接关闭
                            return Ok(());
                        }
                        Ok(n) => {
                            // 有数据可读，从缓冲区读取并写入到conn1
                            match conn2.read_from_buffer(n) {
                                Some(data) => {
                                    conn1.write_to_buffer(&data);
                                    conn1.flush().await?;
                                }
                                None => {
                                    // This should not happen if read() succeeded
                                    return Err(io::Error::new(
                                        io::ErrorKind::UnexpectedEof,
                                        "Buffer data mismatch"
                                    ));
                                }
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }
}
