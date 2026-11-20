//! 速率估计器的行为测试。
//!
//! 全部用纯数据：时间由参数传进来，不读时钟、不 sleep。速率这种东西最怕「测试自己
//! 也在跑，于是结果取决于机器快慢」——这里刻意把 `now_ms` 写死，让每条规则都可复现。

use term_xfer::{RateMeter, MIN_WINDOW_MS};

#[test]
fn the_first_sample_only_records_a_baseline() {
    // 没有窗口就没有速率：报 0 会让人以为卡住了，报一个猜的数则是撒谎。
    let mut meter = RateMeter::new();
    meter.sample(0, 1_000);
    assert!(meter.bytes_per_sec().is_none());
    assert_eq!(meter.bytes_per_sec_rounded(), None);
    assert_eq!(meter.eta_secs(1_000, 0), None, "没速率就没有剩余时间");
}

#[test]
fn a_second_sample_gives_the_instantaneous_rate() {
    // 500ms 走了 1000 字节 = 2000 B/s。
    let mut meter = RateMeter::new();
    meter.sample(0, 0);
    meter.sample(1_000, 500);
    let bps = meter.bytes_per_sec().expect("应有速率");
    assert!((bps - 2_000.0).abs() < 1.0, "{bps}");
    assert_eq!(meter.bytes_per_sec_rounded(), Some(2_000));
}

#[test]
fn samples_closer_than_the_minimum_window_are_folded_into_the_next_one() {
    // 30ms 走了 900 字节。直接算会是 30 KB/s（噪声），正确做法是把这些字节留给下一个
    // 窗口——所以第一次调用不改变估计，第二次按累计窗口算。
    let mut meter = RateMeter::new();
    meter.sample(0, 0);
    meter.sample(900, 30);
    assert!(
        meter.bytes_per_sec().is_none(),
        "窗口太短不该产生速率：{:?}",
        meter.bytes_per_sec()
    );
    // 到 1000ms 时累计 1000 字节 → 1000 B/s，而不是 30 KB/s。
    meter.sample(1_000, 1_000);
    let bps = meter.bytes_per_sec().expect("应有速率");
    assert!((bps - 1_000.0).abs() < 1.0, "{bps}");
}

#[test]
fn a_sample_at_the_same_millisecond_does_not_divide_by_zero() {
    let mut meter = RateMeter::new();
    meter.sample(0, 0);
    meter.sample(500, MIN_WINDOW_MS);
    let before = meter.bytes_per_sec();
    meter.sample(999, MIN_WINDOW_MS);
    assert_eq!(meter.bytes_per_sec(), before, "同一毫秒的重复采样应被忽略");
    assert!(meter.bytes_per_sec().is_some_and(|bps| bps.is_finite()));
}

#[test]
fn a_backwards_byte_count_restarts_the_estimate() {
    // 换文件、重试、续传从头开始都会让字节数变小。旧速率与新传输无关。
    let mut meter = RateMeter::new();
    meter.sample(0, 0);
    meter.sample(10_000, 1_000);
    assert!(meter.bytes_per_sec().is_some());

    meter.sample(0, 1_100);
    assert!(meter.bytes_per_sec().is_none(), "回退应清空估计");

    // 新传输要重新建基线再给速率。
    meter.sample(500, 1_600);
    let bps = meter.bytes_per_sec().expect("应有新速率");
    assert!((bps - 1_000.0).abs() < 1.0, "{bps}");
}

#[test]
fn the_estimate_is_smoothed_rather_than_jumping_at_every_sample() {
    // 稳定 1000 B/s 之后突然跳到 10 000 B/s：估计应当只往上挪三成（α = 0.3），
    // 否则界面上的速度会跟着每一帧乱跳。
    let mut meter = RateMeter::new();
    meter.sample(0, 0);
    meter.sample(1_000, 1_000); // 1000 B/s
    meter.sample(2_000, 2_000); // 又是 1000 B/s
    let steady = meter.bytes_per_sec().expect("应有速率");
    assert!((steady - 1_000.0).abs() < 1.0, "{steady}");

    meter.sample(12_000, 3_000); // 这一秒走了 10 000
    let jumped = meter.bytes_per_sec().expect("应有速率");
    let expected = 1_000.0 * 0.7 + 10_000.0 * 0.3;
    assert!(
        (jumped - expected).abs() < 1.0,
        "应只平滑移动 30%：{jumped} != {expected}"
    );
}

#[test]
fn eta_rounds_up_and_stops_at_zero() {
    let mut meter = RateMeter::new();
    meter.sample(0, 0);
    meter.sample(1_000, 1_000); // 1000 B/s

    // 还剩 2500 字节 → 2.5 秒 → 报 3 秒（向上取整）。
    assert_eq!(meter.eta_secs(3_500, 1_000), Some(3));
    // 刚好整除。
    assert_eq!(meter.eta_secs(5_000, 4_000), Some(1));
    // 传完了给 0，不是 None——「剩余 0 秒」才读得通。
    assert_eq!(meter.eta_secs(5_000, 5_000), Some(0));
    // 传超了（续传对不上）也只报 0。
    assert_eq!(meter.eta_secs(100, 5_000), Some(0));
}

#[test]
fn a_very_slow_link_reports_a_capped_eta_instead_of_astronomical_numbers() {
    // 1 字节/秒传 1 GB：真算是一万三千天。钳到一天，界面不显示荒唐数字。
    let mut meter = RateMeter::new();
    meter.sample(0, 0);
    meter.sample(1, 1_000); // 1 B/s
    assert_eq!(meter.eta_secs(1 << 30, 0), Some(86_400));
}

#[test]
fn reset_throws_away_everything() {
    let mut meter = RateMeter::new();
    meter.sample(0, 0);
    meter.sample(5_000, 1_000);
    assert!(meter.bytes_per_sec().is_some());
    meter.reset();
    assert!(meter.bytes_per_sec().is_none());
    assert_eq!(meter.eta_secs(1_000, 0), None);
    // 重置后第一次采样又只是基线。
    meter.sample(0, 9_000);
    assert!(meter.bytes_per_sec().is_none());
}

#[test]
fn a_sub_one_byte_per_second_rate_is_not_collapsed_to_zero() {
    // 串口链路可能慢到 0.5 B/s。`bytes_per_sec` 保留小数，只有显式取整才变 0——
    // 界面据此显示「<1 B/s」，而不是「0 B/s（卡住了）」。
    let mut meter = RateMeter::new();
    meter.sample(0, 0);
    meter.sample(1, 2_000); // 0.5 B/s
    let bps = meter.bytes_per_sec().expect("应有速率");
    assert!(bps > 0.0 && bps < 1.0, "{bps}");
    assert_eq!(meter.bytes_per_sec_rounded(), Some(1), "四舍五入到 1");
    assert_eq!(meter.eta_secs(100, 1), Some(198), "剩 99 字节 / 0.5 = 198 秒");
}
