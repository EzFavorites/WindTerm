//! `DeskSurface` 的协议翻译辅助：错误分档与参数/结果形状。
//!
//! 从 `surface.rs` 拆出（4j：单文件 500 代码行上限）。纯函数，不动会话池；
//! 公开路径不变——`surface.rs` 再导出这些名字，`surface_tests.rs` 仍经
//! `super::*` 取到同一批判据。

use serde_json::{json, Value};
use term_core::session_runtime::{FacetKind, SessionState};
use term_core::surface_error::SurfaceError;
use term_script::PrimitiveError;
use term_session::OpenError;

use crate::session_open::OpenFailure;

/// `PrimitiveError` → 线上失败。
///
/// 分档的理由与 `term-script` 的 `Display` 文案一致：客户端要看得出「再等等」
/// （timeout）与「改参数」（invalid_params）与「状态不对」（invalid_request）的区别。
pub(super) fn primitive_failure(err: PrimitiveError) -> SurfaceError {
    match err {
        PrimitiveError::EmptyPattern => SurfaceError::invalid_params(err.to_string()),
        PrimitiveError::Timeout => SurfaceError::timeout(err.to_string()),
        PrimitiveError::Closed => SurfaceError::closed(err.to_string()),
        PrimitiveError::AlreadyRecording | PrimitiveError::NotRecording => {
            SurfaceError::invalid_request(err.to_string())
        }
        // 全局层那三类。远程侧目前**没有**走这条路的动词（`session_*` 由
        // `session_open` / `session_close` 自己管，参数校验也在那一层），所以这里
        // 不是「将来会用」，而是「脚本层与远程层共用同一个错误枚举」的必然：
        // 枚举加了变体，这里的 match 就得表态。按语义归类——点错名字是参数
        // 问题，没有绑定是状态问题。
        PrimitiveError::NoSuchSession(_) | PrimitiveError::BadArgument(_) => {
            SurfaceError::invalid_params(err.to_string())
        }
        PrimitiveError::CloseCurrentSession(_) | PrimitiveError::NoBinding => {
            SurfaceError::invalid_request(err.to_string())
        }
        // 键名不认识是参数问题（改键名就好）；`NoInput` 走不到这里——`ask`
        // 是脚本专有动词，远程侧没有那条调用路径。留一臂是为了让「枚举加了
        // 变体，映射得表态」这条规矩继续成立。
        PrimitiveError::UnknownKey(_) => SurfaceError::invalid_params(err.to_string()),
        // 名字不在某一层的表里（停靠面板名），同样是「改名字就好」= 参数问题。
        // 远程侧目前走不到（`app_*` 是 Script-only），留一臂的理由同上。
        PrimitiveError::UnknownName { .. } => SurfaceError::invalid_params(err.to_string()),
        PrimitiveError::NoInput => SurfaceError::invalid_request(err.to_string()),
        // ADR-0027 新增的四类。远程侧同样走不到它们（取消 / 授权 / 端口不支持
        // 都是**脚本运行时**的概念，RPC 那头没有脚本线程），但枚举加了变体，
        // 映射就得表态——按语义归类，别让将来真接上时临时拍脑袋：
        // 「有人叫停」是独立的终态，客户端不该自动重试；
        // 「没授权」与「宿主没这个能力」都属于「这条路走不通」，且**重试无用**
        // （要改的是授权声明或换宿主），所以归到 locked 而不是 internal。
        PrimitiveError::Cancelled => SurfaceError::cancelled(err.to_string()),
        PrimitiveError::Denied(_) | PrimitiveError::Unsupported(_) => {
            SurfaceError::new(term_core::surface_error::ErrorKind::Locked, err.to_string())
        }
        PrimitiveError::Failed(_) => SurfaceError::internal(err.to_string()),
    }
}

/// 按键名表把一串键名翻成字节。未知键名 → `invalid_params`（不是静默丢弃：
/// 丢掉一个键会让脚本在后面某一步等一个永远不会来的提示）。
///
/// `app_cursor` 只影响方向键（`ESC [ A` vs `ESC O A`），其余键名与终端状态无关。
pub(super) fn key_bytes(names: &[String], app_cursor: bool) -> Result<Vec<u8>, SurfaceError> {
    if names.is_empty() {
        return Err(SurfaceError::invalid_params(
            "参数 keys 不能是空数组：一次按键都没有",
        ));
    }
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let Some(bytes) = term_core::keys::key_bytes(name, app_cursor) else {
            return Err(SurfaceError::invalid_params(format!(
                "未知键名 {name:?}；可用 {}",
                term_core::keys::known_names()
            )));
        };
        out.extend_from_slice(bytes.as_bytes());
    }
    Ok(out)
}

/// 取必填的字符串数组参数（`wait_any` 的 `patterns`、`keys` 的 `keys`）。
///
/// 非字符串元素显式报错并点名是第几项——静默 `to_string()` 会把
/// `["OK", 7]` 变成「在等 '7'」，脚本作者永远查不出来。
pub(super) fn required_string_array(params: &Value, key: &str) -> Result<Vec<String>, SurfaceError> {
    let value = match params.get(key) {
        None | Some(Value::Null) => {
            return Err(SurfaceError::invalid_params(format!(
                "缺少必填参数 {key}（字符串数组）"
            )))
        }
        Some(value) => value,
    };
    let items = value
        .as_array()
        .ok_or_else(|| SurfaceError::invalid_params(format!("参数 {key} 必须是数组（实际：{value}）")))?;
    if items.is_empty() {
        return Err(SurfaceError::invalid_params(format!("参数 {key} 至少要有 1 项")));
    }
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            item.as_str().map(str::to_string).ok_or_else(|| {
                SurfaceError::invalid_params(format!(
                    "参数 {key} 第 {} 项必须是字符串（实际：{item}）",
                    i + 1
                ))
            })
        })
        .collect()
}

/// 会话描述符：**所有**返回会话的动词都用这一个形状（`name` / `alive` / `exit`）。
///
/// `session_list` 的一行、`session_open` 的结果、脚本侧 `session_open` /
/// `session_use` 的结果完全同形。为什么值得单独一个函数：两个前端各拼一次
/// 就是漂移的起点（ADR-0023 的「一个动词两个前端一种形状」）。
///
/// **不含 id**：线上形状里没有进程内自增的句柄（ADR-0024）。客户端要再指同一条
/// 会话，用的就是这里的 `name`。
pub(super) fn session_descriptor(state: SessionState, name: &str) -> Value {
    let (alive, exit) = split_state(state);
    json!({"name": name, "alive": alive, "exit": exit})
}

/// 会话状态 → `(alive, exit)`。
pub(super) fn split_state(state: SessionState) -> (bool, Option<String>) {
    match state {
        SessionState::Alive => (true, None),
        SessionState::Finished(exit) => (false, exit),
    }
}

/// 能力 facet 的线上名字。与 `term_core::surface` 的 `facets` 结果
/// schema 里的 `enum` 必须一一对应。
pub(super) fn facet_name(kind: FacetKind) -> &'static str {
    match kind {
        FacetKind::FileTransfer => "file_transfer",
        FacetKind::PortForward => "port_forward",
    }
}

/// `OpenFailure` → 线上失败。四档分开，因为客户端要做的动作不同：
///
/// - `NeedsVault` → `locked`（-32003）。这不是「内部故障」，也不是「参数错了」：
///   它是**策略**拒绝——密码在保险库里，API 客户端不会替用户解锁，得先在
///   前台窗口里解锁。与锁屏门用同一个码，客户端只需处理
///   「现在做不了，等条件变了再试」这一种情形。
/// - `Unsupported` / `Session::Spawn` / `Session::BuiltinChannelNeedsInjection`
///   → `invalid_request`：用户改会话配置或环境就能修。
/// - `Connect` → `internal`（-32603）：DNS / TCP / 握手 / 主机密钥都不是调用方
///   改参数能修的，重试同样一串参数结果一样。这不是「桥接自身的 bug」，但**码**
///   只能取这几个——`internal` 在这里的含义是「调用方无从修复」，比起把它
///   塞进 `invalid_params`（暗示「改参数就好了」）更不容易误导。
/// - `Session::Config` 是唯一走 `invalid_params` 的一档：那是**配置里缺东西**，
///   调用方补上就行（与这一层出现之前的映射逐字一致）。
pub(super) fn open_failure(err: OpenFailure) -> SurfaceError {
    match err {
        OpenFailure::NeedsVault { session } => SurfaceError::locked(format!(
            "会话 {session} 需要解锁保险库才能连接；请在前台窗口里解锁保险库后重试，\
             或改用 Agent / 私钥认证的会话"
        )),
        OpenFailure::Unsupported(why) => SurfaceError::invalid_request(why),
        OpenFailure::Connect(why) => SurfaceError::internal(format!("内置 SSH 连接失败：{why}")),
        OpenFailure::Session(OpenError::Config(detail)) => {
            SurfaceError::invalid_params(detail.to_string())
        }
        OpenFailure::Session(OpenError::Spawn(detail)) => {
            SurfaceError::invalid_request(format!("起会话失败：{detail}"))
        }
        OpenFailure::Session(OpenError::BuiltinChannelNeedsInjection) => {
            // 走不到：内置 SSH 已经被 `open_session` 接住了。留着这一臂是为了
            // 「新增 OpenError 变体时编译器会提醒你回来想清楚映射」。
            SurfaceError::invalid_request(OpenError::BuiltinChannelNeedsInjection.to_string())
        }
        OpenFailure::Session(OpenError::Channel(failure)) => {
            // 工厂的失败分类在 `session_open::open_failure` 已经翻成
            // NeedsVault / Unsupported / Connect 三档，正常走不到这里；
            // 留一臂是为了控制面新增分类时不会静默丢进「起会话失败」。
            match failure {
                term_session::ChannelFailure::NeedsVault { session } => SurfaceError::locked(
                    format!("会话 {session} 需要解锁保险库才能建立通道"),
                ),
                term_session::ChannelFailure::Unsupported(why) => {
                    SurfaceError::invalid_request(why)
                }
                term_session::ChannelFailure::Connect(why) => {
                    SurfaceError::internal(format!("内置 SSH 连接失败：{why}"))
                }
            }
        }
    }
}

