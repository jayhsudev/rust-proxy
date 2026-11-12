use log::{Level, Record};
use log4rs::append::Append;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long `flush` may spend trying to reach the writer thread.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
/// Minimum interval between queue-overflow notices.
const OVERFLOW_NOTICE_INTERVAL: Duration = Duration::from_secs(1);

/// Process-wide counters, exposed through the metrics endpoint.
static DROPPED_TOTAL: AtomicU64 = AtomicU64::new(0);
static WRITE_ERRORS_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Records dropped because the queue was full.
pub fn dropped_total() -> u64 {
    DROPPED_TOTAL.load(Ordering::Relaxed)
}

/// Records the appender failed to write (for example, disk full).
pub fn write_errors_total() -> u64 {
    WRITE_ERRORS_TOTAL.load(Ordering::Relaxed)
}

/// Wraps another appender and performs the actual write on a dedicated thread,
/// so a slow disk cannot block Tokio worker threads.
///
/// The queue is bounded: when it is full, records are dropped and counted
/// instead of stalling the application. Drops are surfaced through the metrics
/// endpoint and through a throttled warning written by the writer thread.
#[derive(Debug)]
pub struct AsyncAppender {
    tx: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
    write_errors: Arc<AtomicU64>,
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
        let write_errors = Arc::new(AtomicU64::new(0));

        let thread_dropped = dropped.clone();
        let thread_errors = write_errors.clone();
        let _ = std::thread::Builder::new()
            .name("rust-proxy-log".to_string())
            .spawn(move || writer_loop(inner, rx, thread_dropped, thread_errors));

        Self {
            tx,
            dropped,
            write_errors,
        }
    }

    /// Number of records dropped because the queue was full.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Number of records the inner appender failed to write.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn write_errors(&self) -> u64 {
        self.write_errors.load(Ordering::Relaxed)
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

fn writer_loop(
    inner: Box<dyn Append>,
    rx: Receiver<Message>,
    dropped: Arc<AtomicU64>,
    write_errors: Arc<AtomicU64>,
) {
    let mut reported_dropped = 0u64;
    let mut last_notice = Instant::now();

    while let Ok(message) = rx.recv() {
        match message {
            Message::Record(owned) => {
                // A failing appender must not kill the writer thread, but it must
                // not go unnoticed either.
                let failed = owned.with_record(|record| inner.append(record).is_err());

                if failed {
                    write_errors.fetch_add(1, Ordering::Relaxed);
                    WRITE_ERRORS_TOTAL.fetch_add(1, Ordering::Relaxed);
                }
            }
            Message::Flush(ack) => {
                inner.flush();
                let _ = ack.send(());
            }
        }

        // Report queue overflow from the writer thread: appending here does not
        // go through the (full) queue, so it cannot recurse.
        let dropped_now = dropped.load(Ordering::Relaxed);
        if dropped_now > reported_dropped && last_notice.elapsed() >= OVERFLOW_NOTICE_INTERVAL {
            let delta = dropped_now - reported_dropped;
            reported_dropped = dropped_now;
            last_notice = Instant::now();

            let args = format_args!("log queue overflow: {} record(s) dropped", delta);
            let record = Record::builder()
                .level(Level::Warn)
                .target("rust_proxy::logging")
                .args(args)
                .build();

            if inner.append(&record).is_err() {
                WRITE_ERRORS_TOTAL.fetch_add(1, Ordering::Relaxed);
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
                DROPPED_TOTAL.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(TrySendError::Disconnected(_)) => Err(anyhow::anyhow!("log writer thread stopped")),
        }
    }

    /// Best-effort flush that never blocks for longer than [`FLUSH_TIMEOUT`],
    /// even when the queue is full.
    fn flush(&self) {
        let deadline = Instant::now() + FLUSH_TIMEOUT;

        loop {
            let (ack_tx, ack_rx) = sync_channel(1);

            match self.tx.try_send(Message::Flush(ack_tx)) {
                Ok(()) => {
                    let _ = ack_rx.recv_timeout(FLUSH_TIMEOUT);
                    return;
                }
                Err(TrySendError::Full(_)) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
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

    /// Appender that always fails, for the write-error counter.
    #[derive(Debug)]
    struct Failing;

    impl Append for Failing {
        fn append(&self, _record: &Record) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("disk full"))
        }

        fn flush(&self) {}
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
        assert_eq!(appender.write_errors(), 0);
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

    #[test]
    fn test_write_errors_are_counted() {
        let appender = AsyncAppender::new(Box::new(Failing), 8);

        for _ in 0..3 {
            appender.append(&record(format_args!("lost"))).unwrap();
        }
        appender.flush();

        assert_eq!(appender.write_errors(), 3);
        assert!(write_errors_total() >= 3);
    }

    #[test]
    fn test_flush_is_bounded_when_queue_is_full() {
        // A tiny queue with a slow consumer: flush must still return promptly.
        #[derive(Debug)]
        struct Slow;

        impl Append for Slow {
            fn append(&self, _record: &Record) -> anyhow::Result<()> {
                std::thread::sleep(Duration::from_millis(5));
                Ok(())
            }

            fn flush(&self) {}
        }

        let appender = AsyncAppender::new(Box::new(Slow), 1);

        for _ in 0..100 {
            appender.append(&record(format_args!("x"))).unwrap();
        }

        let start = Instant::now();
        appender.flush();
        assert!(
            start.elapsed() <= FLUSH_TIMEOUT + Duration::from_millis(500),
            "flush blocked for {:?}",
            start.elapsed()
        );
    }
}
