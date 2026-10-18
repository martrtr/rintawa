//! Logging API exposed to extension components.

/// Logging levels supported by the Rintawa logging API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LogLevel {
    /// A trace-level log message.
    Trace,
    /// A debug-level log message.
    Debug,
    /// An informational log message.
    Info,
    /// A warning-level log message.
    Warn,
    /// An error-level log message.
    Error,
}

/// The logging API available to extensions.
///
/// This trait provides a simple logging interface that extensions can use
/// to emit log messages at different severity levels.
pub trait LoggerApi: Send + Sync {
    /// Logs a message at the specified level.
    fn log(&self, level: LogLevel, message: &str);

    /// Logs an informational message.
    fn info(&self, message: &str) {
        self.log(LogLevel::Info, message);
    }

    /// Logs a warning message.
    fn warn(&self, message: &str) {
        self.log(LogLevel::Warn, message);
    }

    /// Logs an error message.
    fn error(&self, message: &str) {
        self.log(LogLevel::Error, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_json_round_trip() -> serde_json::Result<()> {
        let encoded = serde_json::to_string(&LogLevel::Info)?;
        let decoded: LogLevel = serde_json::from_str(&encoded)?;

        assert_eq!(encoded, "\"Info\"");
        assert_eq!(decoded, LogLevel::Info);

        Ok(())
    }
}
