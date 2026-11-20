//! `NewSessionData` 串口五参数的往返单测。
//!
//! 挂在 `session_form.rs` 之下（`#[path]`），与主测试模块并行，避免让
//! `ui/` 源文件越过 500 行上限。

use super::*;

#[test]
fn serial_fields_round_trip_through_the_form() {
    // 串口五个参数必须真进真出。此前只有 baud_rate 有落点，
    // data_bits/stop_bits/parity/flow_control 在 SessionConfig 里是常量、
    // 表单写不进也读不出——「能填、看起来存了、实际没生效」的假控件。
    let mut c = SessionConfig::new(Protocol::Serial);
    c.baud_rate = 57600;
    c.data_bits = 7;
    c.stop_bits = 2;
    c.parity = "Even".to_string();
    c.flow_control = "RTS/CTS".to_string();
    let f = NewSessionData::from_config(&c);
    assert_eq!(f.baud_rate, 57600, "Form must echo the saved baud rate");
    assert_eq!(f.data_bits, 7, "Form must echo the saved data bits");
    assert_eq!(f.stop_bits, 2, "Form must echo the saved stop bits");
    assert_eq!(f.parity, "Even", "Form must echo the saved parity");
    assert_eq!(f.flow_control, "RTS/CTS", "Form must echo the saved flow control");

    // 表单改一遍再落盘，值必须原样回到配置（不退化成常量默认值）。
    let mut f2 = f;
    f2.baud_rate = 19200;
    f2.data_bits = 8;
    f2.stop_bits = 1;
    f2.parity = "Odd".to_string();
    f2.flow_control = "XON/XOFF".to_string();
    let saved = f2.to_config(SessionConfig::new(Protocol::Serial));
    assert_eq!(saved.baud_rate, 19200, "to_config must persist baud rate");
    assert_eq!(saved.data_bits, 8, "to_config must persist data bits");
    assert_eq!(saved.stop_bits, 1, "to_config must persist stop bits");
    assert_eq!(saved.parity, "Odd", "to_config must persist parity");
    assert_eq!(saved.flow_control, "XON/XOFF", "to_config must persist flow control");

    // 出厂会话（没动过串口字段）退回默认值，不把未设当真值。
    let blank = NewSessionData::from_config(&SessionConfig::new(Protocol::Serial));
    assert_eq!(blank.data_bits, term_core::config::SERIAL_DATA_BITS);
    assert_eq!(blank.stop_bits, term_core::config::SERIAL_STOP_BITS);
    assert_eq!(blank.parity, term_core::config::SERIAL_PARITY);
    assert_eq!(blank.flow_control, term_core::config::SERIAL_FLOW_CONTROL);
}
