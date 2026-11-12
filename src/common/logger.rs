use crate::common::config::LoggerConfig;
use crate::common::log_writer::AsyncAppender;
use log::{LevelFilter, debug, error, info, trace, warn};
use log4rs::{
    append::{
        Append,
        console::{ConsoleAppender, Target},
        rolling_file::{
            RollingFileAppender,
            policy::compound::{
                CompoundPolicy, roll::fixed_window::FixedWindowRoller, trigger::size::SizeTrigger,
            },
        },
    },
    config::{Appender, Config, Root},
    encode::pattern::PatternEncoder,
    filter::threshold::ThresholdFilter,
};
use std::fs;
use std::path::Path;
use std::str::FromStr;

pub fn setup_logger(config: LoggerConfig) -> Result<log4rs::Handle, Box<dyn std::error::Error>> {
    let level = LevelFilter::from_str(&config.level).unwrap_or(LevelFilter::Info);
    let trigger_file_size = config.file_size * 1024 * 1024;

    let stderr: Box<dyn Append> =
        Box::new(ConsoleAppender::builder().target(Target::Stderr).build());

    let trigger = SizeTrigger::new(trigger_file_size);
    let roller = FixedWindowRoller::builder()
        .base(0)
        .build(&config.archive_pattern, config.file_count)?;
    let policy = CompoundPolicy::new(Box::new(trigger), Box::new(roller));

    if let Some(parent) = Path::new(&config.path).parent() {
        fs::create_dir_all(parent)?;
    }

    // Logs may contain usernames and target hosts: keep them owner-only on unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        if !Path::new(&config.path).exists() {
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&config.path)?;
        }
    }

    // PatternEncoder never appends a terminator, so make sure one record cannot
    // run into the next even when the configured pattern omits the newline.
    let pattern = normalized_pattern(&config.format);

    let logfile = RollingFileAppender::builder()
        .encoder(Box::new(PatternEncoder::new(&pattern)))
        .build(&config.path, Box::new(policy))?;

    // Writing happens on dedicated threads so file / stderr I/O never blocks a
    // Tokio worker thread.
    let (logfile, stderr): (Box<dyn Append>, Box<dyn Append>) = if config.non_blocking {
        (
            Box::new(AsyncAppender::new(Box::new(logfile), config.queue_capacity)),
            Box::new(AsyncAppender::new(stderr, config.queue_capacity)),
        )
    } else {
        (Box::new(logfile), stderr)
    };

    let runtime_config = Config::builder()
        .appender(Appender::builder().build("logfile", logfile))
        .appender(
            Appender::builder()
                .filter(Box::new(ThresholdFilter::new(level)))
                .build("stderr", stderr),
        )
        .build(
            Root::builder()
                .appender("logfile")
                .appender("stderr")
                .build(level),
        )?;

    let handle = log4rs::init_config(runtime_config)?;

    match level {
        LevelFilter::Trace => trace!("Logger initialized (trace)"),
        LevelFilter::Debug => debug!("Logger initialized (debug)"),
        LevelFilter::Info => info!("Logger initialized (info)"),
        LevelFilter::Warn => warn!("Logger initialized (warn)"),
        LevelFilter::Error => error!("Logger initialized (error)"),
        LevelFilter::Off => (),
    }

    info!(
        "Log file: '{}', archive: '{}'",
        config.path, config.archive_pattern
    );

    Ok(handle)
}

/// Ensures every encoded record ends with a newline.
fn normalized_pattern(format: &str) -> String {
    let mut pattern = format.to_string();
    if !pattern.ends_with('\n') {
        pattern.push('\n');
    }
    pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_gets_a_terminator() {
        assert_eq!(normalized_pattern("{m}"), "{m}\n");
        assert_eq!(normalized_pattern("{m}\n"), "{m}\n");
        assert_eq!(normalized_pattern("{m}\n\n"), "{m}\n\n");
    }

    #[test]
    fn test_default_pattern_is_usable() {
        let format = LoggerConfig::default().format;
        assert!(format.contains("{m}"));
        assert!(format.contains("%.3f"), "millisecond timestamps");
        assert!(format.contains("{t}"), "target for diagnosis");
        assert!(normalized_pattern(&format).ends_with('\n'));
    }
}
