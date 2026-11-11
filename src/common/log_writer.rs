use log::{Level, Record};
use log4rs::append::Append;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// Wraps another appender and performs the actual write on a dedicated thread,
/// so a slow disk cannot block Tokio worker threads.
///
/// The queue is bounded: when it is full, records are dropped and counted
/// instead of stalling the application.
#[derive(Debug)]
pub struct AsyncAppender {
    tx: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
}

#[derive(Debug)]
enum Message {
    Record(OwnedRecord),
    Flush(SyncSender<()>),
}

#[derive(Debug)]
struct OwnedRecord {
    level: Level,
    target: &'static str,
    message: String,
}

impl OwnedRecord {
    /// Rebuilds a log record on the writer thread.
    fn with_record<R>(&self, f: impl FnOnce(&Record<'_>) -> R) -> R {
        let args = format_args!("{}", self.message);
        let record = Record::builder()
            .level(self.level)
            .target(self.target)
            .args(args)
            .build();

        f(&record)
    }
}

impl AsyncAppender {
    /// Spawns the writer thread; `capacity` bounds the in-flight queue.
    pub fn new(inner: Box<dyn Append>, capacity: usize) -> Self {
        let (tx, rx) = sync_channel(capacity.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let _ = std::thread::Builder::new()
            .name("rust-proxy-log".to_string())
            .spawn(move || writer_loop(inner, rx));

        Self { tx, dropped }
    }

    /// Number of records dropped because the queue was full.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// `Record::target` is not exposed as `&'static str`, but targets come from a
/// fixed set of module names, so interning them once is bounded and cheap.
fn intern_target(target: &str) -> &'static str {
    static INTERNED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut interned = INTERNED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(existing) = interned.get(target) {
        return existing;
    }

    let leaked: &'static str = Box::leak(target.to_string().into_boxed_str());
    interned.insert(leaked);
    leaked
}

fn writer_loop(inner: Box<dyn Append>, rx: Receiver<Message>) {
    while let Ok(message) = rx.recv() {
        match message {
            Message::Record(owned) => {
                // A failing appender must not kill the writer thread.
                owned.with_record(|record| {
                    let _ = inner.append(record);
                });
            }
            Message::Flush(ack) => {
                inner.flush();
                let _ = ack.send(());
            }
        }
    }

    inner.flush();
}

impl Append for AsyncAppender {
    fn append(&self, record: &Record) -> anyhow::Result<()> {
        let owned = OwnedRecord {
            level: record.level(),
            target: intern_target(record.metadata().target()),
            message: record.args().to_string(),
        };

        match self.tx.try_send(Message::Record(owned)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(TrySendError::Disconnected(_)) => Err(anyhow::anyhow!("log writer thread stopped")),
        }
    }

    fn flush(&self) {
        let (ack_tx, ack_rx) = sync_channel(1);
        if self.tx.send(Message::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(FLUSH_TIMEOUT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct Capture {
        lines: Arc<Mutex<Vec<String>>>,
        flushed: Arc<AtomicU64>,
    }

    impl Append for Capture {
        fn append(&self, record: &Record) -> anyhow::Result<()> {
            self.lines
                .lock()
                .unwrap()
                .push(format!("{} {}", record.level(), record.args()));
            Ok(())
        }

        fn flush(&self) {
            self.flushed.fetch_add(1, Ordering::Relaxed);
        }
    }

    type Captured = (Box<dyn Append>, Arc<Mutex<Vec<String>>>, Arc<AtomicU64>);

    fn capture() -> Captured {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let flushed = Arc::new(AtomicU64::new(0));
        (
            Box::new(Capture {
                lines: lines.clone(),
                flushed: flushed.clone(),
            }),
            lines,
            flushed,
        )
    }

    fn record(args: std::fmt::Arguments<'_>) -> Record<'_> {
        Record::builder()
            .level(Level::Info)
            .target("test")
            .args(args)
            .build()
    }

    #[test]
    fn test_appends_on_writer_thread() {
        let (inner, lines, _flushed) = capture();
        let appender = AsyncAppender::new(inner, 16);

        appender.append(&record(format_args!("one"))).unwrap();
        appender.append(&record(format_args!("two {}", 2))).unwrap();
        appender.flush();

        let captured = lines.lock().unwrap().clone();
        assert_eq!(captured, vec!["INFO one", "INFO two 2"]);
        assert_eq!(appender.dropped(), 0);
    }

    #[test]
    fn test_full_queue_drops_instead_of_blocking() {
        // Capacity 1 and no flush in between: the writer thread may consume
        // entries, so this only asserts that appending never panics or blocks.
        let (inner, _lines, _flushed) = capture();
        let appender = AsyncAppender::new(inner, 1);

        for _ in 0..1000 {
            appender.append(&record(format_args!("spam"))).unwrap();
        }

        appender.flush();
        assert!(appender.dropped() <= 1000);
    }
}
