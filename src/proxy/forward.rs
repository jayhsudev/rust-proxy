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
        // 简化版本：直接进行双向转发
        // 由于克隆问题，使用更简单的循环方式
        loop {
            // 从 conn1 读取并写入到 conn2
            if conn1.has_data() || conn1.read().await? > 0 {
                if conn1.has_data() {
                    if let Some(data) = conn1.read_from_buffer(conn1.available_bytes()) {
                        conn2.write_to_buffer(&data);
                        conn2.flush().await?;
                    }
                }
            }

            // 从 conn2 读取并写入到 conn1
            if conn2.has_data() || conn2.read().await? > 0 {
                if conn2.has_data() {
                    if let Some(data) = conn2.read_from_buffer(conn2.available_bytes()) {
                        conn1.write_to_buffer(&data);
                        conn1.flush().await?;
                    }
                }
            }

            // 短暂睡眠以避免CPU占用过高
            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        }
    }
}
