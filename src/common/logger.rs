use crate::common::config::LoggerConfig;
use log::{debug, error, info, trace, warn, LevelFilter, SetLoggerError};
use log4rs::{
    append::{
        console::{ConsoleAppender, Target},
        rolling_file::policy::compound::{
            roll::fixed_window::FixedWindowRoller, trigger::size::SizeTrigger, CompoundPolicy,
        },
    },
    config::{Appender, Config, Root},
    encode::pattern::PatternEncoder,
    filter::threshold::ThresholdFilter,
};
use std::fs;
use std::path::Path;
use std::str::FromStr;

pub fn setup_logger(config: LoggerConfig) -> Result<log4rs::Handle, SetLoggerError> {
    let level = LevelFilter::from_str(&config.level).unwrap_or_else(|e| {
        log::warn!(
            "Invalid log level '{}': {}, defaulting to Info",
            config.level,
            e
        );
        LevelFilter::Info
    });
    let trigger_file_size = config.file_size * 1024 * 1024; // MB

    // Build a stderr logger.
    let stderr = ConsoleAppender::builder().target(Target::Stderr).build();

    // Create a policy to use with the file logging
    let trigger = SizeTrigger::new(trigger_file_size);
    let roller = FixedWindowRoller::builder()
        .base(0) // Default Value (line not needed unless you want to change from 0 (only here for demo purposes)
        .build(&config.archive_pattern, config.file_count) // Roll based on pattern and max 3 archive files
        .unwrap();
    let policy = CompoundPolicy::new(Box::new(trigger), Box::new(roller));

    // Ensure log directory exists
    if let Some(parent) = Path::new(&config.path).parent() {
        fs::create_dir_all(parent).unwrap();
    }

    // Logging to log file. (with rolling)
    let logfile = log4rs::append::rolling_file::RollingFileAppender::builder()
        // Pattern: https://docs.rs/log4rs/*/log4rs/encode/pattern/index.html
        .encoder(Box::new(PatternEncoder::new(
            "{d(%Y-%m-%d %H:%M:%S)} - {l} - {m}\n",
        )))
        .build(&config.path, Box::new(policy))
        .unwrap();

    // Log Trace level output to file where trace is the default level
    // and the programmatically specified level to stderr.
    let runtime_config = Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(logfile)))
        .appender(
            Appender::builder()
                .filter(Box::new(ThresholdFilter::new(level)))
                .build("stderr", Box::new(stderr)),
        )
        .build(
            Root::builder()
                .appender("logfile")
                .appender("stderr")
                .build(level),
        )
        .unwrap();

    // Use this to change log levels at runtime.
    // This means you can change the default log level to trace
    // if you are trying to debug an issue and need more logs on then turn it off
    // once you are done.
    let handle: log4rs::Handle = log4rs::init_config(runtime_config)?;

    match level {
        LevelFilter::Trace => trace!("Logger initialized with trace level"),
        LevelFilter::Debug => debug!("Logger initialized with debug level"),
        LevelFilter::Info => info!("Logger initialized with info level"),
        LevelFilter::Warn => warn!("Logger initialized with warn level"),
        LevelFilter::Error => error!("Logger initialized with error level"),
        LevelFilter::Off => (),
    }

    info!(
        "See '{}' for log and '{}' for archived logs",
        config.path, config.archive_pattern
    );

    Ok(handle)
}
