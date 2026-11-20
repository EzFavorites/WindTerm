//! 统一错误分类（ADR-0019 §B.11）：`{code, message, kind}` 三元组。
//!
//! 设计要点三句话：
//!
//! 1. **`kind` 是机器读的，`message` 是给人读的**。客户端按 `kind` 分支，
//!    不许去 match 中文文案；`message` 允许随时改写。
//! 2. **`code` 只用两种空间**：JSON-RPC 2.0 规定的标准码（-32700 / -32600 /
//!    -32601 / -32602 / -32603，改不了），以及规范留给实现自定义的服务端错误
//!    区间（-32000 … -32099）。后者用来承载「超时 / 会话已结束 / 不存在 /
//!    被锁屏拒绝」这类**协议层没有码、但客户端必须能分辨**的条件——
//!    把它们统统塞进 -32603 internal 会让「等超时了」和「桥接内部崩了」
//!    在客户端看来一模一样，那正是 ADR 反对的「静默吞错」。
//! 3. **绝不用成功形状冒充失败**。`session_close` 关不掉、`wait` 等不到，都是
//!    `Err`，不是 `{"closed": false}` / 空字符串。
//!
//! 传输层怎么放这个三元组：JSON-RPC 的 `error` 对象里 `code` / `message`
//! 是规范字段，`kind` 作为**扩展成员**平铺在同一层（规范未禁止扩展成员，
//! 且这样 Python 客户端一句 `err["kind"]` 就能拿到）。刻意不用 `data`
//! 包一层——`data` 是「补充信息」，而 `kind` 是主信息。

use std::fmt;

use term_core::session_runtime::WaitError;

/// 失败的种类。客户端按它分支，不看 `message`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// 请求不是合法 JSON。码 -32700。
    ParseError,
    /// JSON 合法但不是合法请求（空批量、`method` 不是字符串、`jsonrpc` 版本不对……）。
    /// 码 -32600。
    InvalidRequest,
    /// 方法不在能力面上。码 -32601（客户端该去调 `describe`）。
    MethodNotFound,
    /// 方法在，参数不对（缺失、类型错、空模式）。码 -32602。
    InvalidParams,
    /// 桥接内部故障，调用方无从修复。码 -32603。
    Internal,
    /// `wait` 等到了上限仍未命中。码 -32000。**不是**空结果。
    Timeout,
    /// 会话已结束（或被关闭），这个动作无从执行。码 -32001。
    Closed,
    /// 目标不存在（tab id 没登记过）。码 -32002。
    NotFound,
    /// 被策略拒绝：锁屏态 / 无授权时不许新建会话（ADR-0019 §B.9 的落点）。
    /// 码 -32003。本刀还没有 token 与锁屏门，但分类先占位——
    /// 加门的时候只改判定，不改线上形状。
    Locked,
    /// 被**用户的取消动作**打断（脚本里的「停止」按钮、上限到期前的主动
    /// 中止）。码 -32004。与 `Timeout` 分开：超时是「环境没给答案」，取消
    /// 是「有人叫停」，客户端要不要重试完全不同。
    Cancelled,
}

impl ErrorKind {
    /// 线上 `kind` 字段的取值（机器读，永不用中文）。
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::ParseError => "parse_error",
            ErrorKind::InvalidRequest => "invalid_request",
            ErrorKind::MethodNotFound => "method_not_found",
            ErrorKind::InvalidParams => "invalid_params",
            ErrorKind::Internal => "internal",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Closed => "closed",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Locked => "locked",
            ErrorKind::Cancelled => "cancelled",
        }
    }

    /// 该种类对应的 JSON-RPC 码。标准码给标准条件，其余落在
    /// -32000 … -32099（实现自定义服务端错误区间）。
    pub fn code(self) -> i32 {
        match self {
            ErrorKind::ParseError => -32700,
            ErrorKind::InvalidRequest => -32600,
            ErrorKind::MethodNotFound => -32601,
            ErrorKind::InvalidParams => -32602,
            ErrorKind::Internal => -32603,
            ErrorKind::Timeout => -32000,
            ErrorKind::Closed => -32001,
            ErrorKind::NotFound => -32002,
            ErrorKind::Locked => -32003,
            ErrorKind::Cancelled => -32004,
        }
    }

    /// 从线上 `kind` 字符串还原（客户端侧/测试用）。未知取值返回 `None`——
    /// 不猜、不降级成 internal。
    ///
    /// 刻意不叫 `from_str`：那会撞上 `std::str::FromStr` 的语义，而这里
    /// 是**不失败到默认值**的线格式还原（Clippy 的 `should_implement_trait`
    /// 也不允许同名固有方法）。
    pub fn from_wire(text: &str) -> Option<ErrorKind> {
        let all = [
            ErrorKind::ParseError,
            ErrorKind::InvalidRequest,
            ErrorKind::MethodNotFound,
            ErrorKind::InvalidParams,
            ErrorKind::Internal,
            ErrorKind::Timeout,
            ErrorKind::Closed,
            ErrorKind::NotFound,
            ErrorKind::Locked,
            ErrorKind::Cancelled,
        ];
        all.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// 一次失败：`{code, message, kind}`。
///
/// `code` 由 `kind` 推导（[`ErrorKind::code`]），存字段是为了序列化时不必
/// 再算一次，也为了让「码和种类对不上」这种 bug 在构造处就无处可藏——
/// 构造只经 [`SurfaceError::new`] 这一个入口。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceError {
    pub code: i32,
    pub message: String,
    pub kind: ErrorKind,
}

impl SurfaceError {
    /// 唯一构造入口：`code` 一律由 `kind` 推导。
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            code: kind.code(),
            message: message.into(),
            kind,
        }
    }

    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ParseError, message)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidRequest, message)
    }

    pub fn method_not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::MethodNotFound, message)
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidParams, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Timeout, message)
    }

    pub fn closed(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Closed, message)
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Cancelled, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    pub fn locked(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Locked, message)
    }
}

impl fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}:{}] {}", self.kind.as_str(), self.code, self.message)
    }
}

impl std::error::Error for SurfaceError {}

impl From<WaitError> for SurfaceError {
    /// 等待类失败到线上错误的一一映射。四臂都要有名字——`EmptyPattern`
    /// 是调用方写错了脚本（可修复），`Timeout` 是环境没给答案（可能重试），
    /// `Closed` 是会话没了（重试无用），`Cancelled` 是有人叫停（重试等于
    /// 无视用户的意志）。把它们合并就丢了这些区别。
    fn from(err: WaitError) -> Self {
        match err {
            WaitError::EmptyPattern => SurfaceError::invalid_params(err.to_string()),
            WaitError::Timeout => SurfaceError::timeout(err.to_string()),
            WaitError::Closed => SurfaceError::closed(err.to_string()),
            WaitError::Cancelled => SurfaceError::cancelled(err.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_kind_has_a_distinct_code_and_a_machine_readable_name() {
        let all = [
            ErrorKind::ParseError,
            ErrorKind::InvalidRequest,
            ErrorKind::MethodNotFound,
            ErrorKind::InvalidParams,
            ErrorKind::Internal,
            ErrorKind::Timeout,
            ErrorKind::Closed,
            ErrorKind::NotFound,
            ErrorKind::Locked,
            ErrorKind::Cancelled,
        ];
        let mut codes = HashSet::new();
        for kind in all {
            assert!(codes.insert(kind.code()), "{kind:?} 的码与别的种类撞了");
            assert!(
                kind.as_str().chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{kind:?} 的 kind 必须是 snake_case ASCII：{}",
                kind.as_str()
            );
            assert_eq!(ErrorKind::from_wire(kind.as_str()), Some(kind), "kind 往返不一致");
        }
        assert_eq!(ErrorKind::from_wire("no_such_kind"), None, "未知 kind 不得猜");
    }

    #[test]
    fn the_five_standard_jsonrpc_codes_are_used_for_their_own_conditions() {
        // 标准码不能被挪作他用：-32603 只给「真的内部故障」。
        assert_eq!(ErrorKind::ParseError.code(), -32700);
        assert_eq!(ErrorKind::InvalidRequest.code(), -32600);
        assert_eq!(ErrorKind::MethodNotFound.code(), -32601);
        assert_eq!(ErrorKind::InvalidParams.code(), -32602);
        assert_eq!(ErrorKind::Internal.code(), -32603);
        // 应用条件落在实现自定义区间（-32000 … -32099）。
        for kind in [
            ErrorKind::Timeout,
            ErrorKind::Closed,
            ErrorKind::NotFound,
            ErrorKind::Locked,
            ErrorKind::Cancelled,
        ] {
            assert!(
                (-32099..=-32000).contains(&kind.code()),
                "{kind:?} 的码 {} 不在实现自定义区间",
                kind.code()
            );
        }
    }

    #[test]
    fn the_triple_always_agrees_with_its_kind() {
        let err = SurfaceError::timeout("等待 3000ms 未命中 PASSWORD");
        assert_eq!(err.code, -32000);
        assert_eq!(err.kind, ErrorKind::Timeout);
        assert_eq!(err.message, "等待 3000ms 未命中 PASSWORD");
        assert_eq!(err.to_string(), "[timeout:-32000] 等待 3000ms 未命中 PASSWORD");
    }

    #[test]
    fn wait_errors_map_to_distinguishable_kinds() {
        assert_eq!(
            SurfaceError::from(WaitError::EmptyPattern).kind,
            ErrorKind::InvalidParams,
            "空模式是调用方的错，不是超时"
        );
        assert_eq!(SurfaceError::from(WaitError::Timeout).kind, ErrorKind::Timeout);
        assert_eq!(SurfaceError::from(WaitError::Closed).kind, ErrorKind::Closed);
        // 文案也不能是空的：Display 直接面向脚本作者。
        for err in [WaitError::EmptyPattern, WaitError::Timeout, WaitError::Closed] {
            assert!(!SurfaceError::from(err).message.is_empty());
        }
    }
}
