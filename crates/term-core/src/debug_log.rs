//! `Mode ▸ Debug Mode` 附带的排障日志选项。
//!
//! 与用户自己在 Logging 页配的日志**不是一回事**：那是清洗后的可读文本，
//! 这份要的是**原始字节（含所有转义序列）**——排障时恰恰需要看见
//! `ESC[38;5;…m` 这类控制序列。文件名固定加 `debug_` 前缀，避免和常规日志混。
//! 单独一个文件是为了不把 `log_opts.rs` 顶过 500 代码行门禁。

use crate::log_opts::{LogOptions, LogType, DEFAULT_FILE_NAME};

pub fn options() -> LogOptions {
    LogOptions {
        log_type: LogType::All,
        file_name: format!("debug_{DEFAULT_FILE_NAME}"),
        // 用户日志的出厂值是 0（∞），但排障日志会一直开着，长会话无界增长
        // 会把盘写满。给个有限上限，复用 log.rs 已有的轮转与 `# rotated from`。
        max_size_mb: 16,
        ..LogOptions::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_mode_log_captures_raw_bytes_under_a_distinct_name() {
        let opts = options();
        assert_eq!(opts.mode(), LogType::All, "排障要看见转义序列，不能清洗");
        assert_eq!(opts.file_name, format!("debug_{DEFAULT_FILE_NAME}"));
        assert!(opts.max_bytes().is_some(), "排障日志必须有轮转上限");
    }
}
