use crate::common::config::LoggerConfig;
use log::{debug, error, info, trace, warn, LevelFilter};
use log4rs::{
    append::{
        console::{ConsoleAppender, Target},
        rolling_file::{
            policy::compound::{
                roll::fixed_window::FixedWindowRoller, trigger::size::SizeTrigger, CompoundPolicy,
            },
            RollingFileAppender,
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

    let stderr = ConsoleAppender::builder().target(Target::Stderr).build();

    let trigger = SizeTrigger::new(trigger_file_size);
    let roller = FixedWindowRoller::builder()
        .base(0)
        .build(&config.archive_pattern, config.file_count)?;
    let policy = CompoundPolicy::new(Box::new(trigger), Box::new(roller));

    if let Some(parent) = Path::new(&config.path).parent() {
        fs::create_dir_all(parent)?;
    }

    let logfile = RollingFileAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "{d(%Y-%m-%d %H:%M:%S)} - {l} - {m}\n",
        )))
        .build(&config.path, Box::new(policy))?;

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
