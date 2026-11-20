//! Rhai 沙箱配置与预算档位：操作数上限、表达式深度、执行墙钟。
//!
//! Rhai 默认无文件 / 网络访问，这里不放开（只做减法）。三个旋钮
//! 各管一类失控：死循环 → 操作数；递归表达式爆栈 → 表达式深度；
//! 慢脚本 → 墙钟超时（顺带钳制单次阻塞等待的上限）。

use std::time::Duration;

/// 一次执行的操作数上限默认值：够跑完正常自动化脚本，
/// 又能阻断 `while true {}` 这类死循环。
pub const DEFAULT_MAX_OPERATIONS: u64 = 1_000_000;

/// 表达式最大嵌套深度默认值：阻断递归表达式把栈打爆。
pub const DEFAULT_MAX_EXPR_DEPTH: usize = 32;

/// 全局语法结构最大嵌套深度默认值（≥ 表达式深度）。
pub const DEFAULT_MAX_GLOBAL_DEPTH: usize = 64;

/// 执行墙钟上限默认值：同时是单次 `wait` / `read_until` 超时的钳制上限
/// ——沙箱的墙钟预算不该被单个阻塞原语击穿。
pub const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Rhai 沙箱配置。
#[derive(Clone, Debug)]
pub struct SandboxConfig {
    /// 一次执行允许的最大操作数（防死循环）。
    pub max_operations: u64,
    /// 表达式最大嵌套深度（防递归爆栈）。
    pub max_expr_depth: usize,
    /// 全局语法结构最大嵌套深度。
    pub max_global_depth: usize,
    /// 执行墙钟上限；同时钳制单次等待超时。
    pub execution_timeout: Duration,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            max_operations: DEFAULT_MAX_OPERATIONS,
            max_expr_depth: DEFAULT_MAX_EXPR_DEPTH,
            max_global_depth: DEFAULT_MAX_GLOBAL_DEPTH,
            execution_timeout: DEFAULT_EXECUTION_TIMEOUT,
        }
    }
}

/// 预算档位（ADR-0027）：界面点「运行」与 CI 跑同一条脚本，该给的额度不同。
///
/// 为什么是两档而不是「一个可调的秒数」：界面上的用户在心里等结果（30 秒
/// 已经很长），CI 里的用户要的是「别在中途放弃」（十分钟）。中间那 200 档
/// 数字的差别只是让人多做一个决定，两个名字才说得清各自的意图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetKind {
    /// 界面：30 秒 / 100 万操作。
    Interactive,
    /// 批处理：10 分钟 / 5000 万操作。
    Batch,
}

/// 一次执行的预算：墙钟 + 操作数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptBudget {
    pub wall: Duration,
    pub max_operations: u64,
}

impl ScriptBudget {
    pub fn of(kind: BudgetKind) -> Self {
        match kind {
            BudgetKind::Interactive => Self {
                wall: DEFAULT_EXECUTION_TIMEOUT,
                max_operations: DEFAULT_MAX_OPERATIONS,
            },
            BudgetKind::Batch => Self {
                wall: Duration::from_secs(600),
                max_operations: 50_000_000,
            },
        }
    }

    /// 套到沙箱配置上（其余旋钮保持缺省）。
    pub fn sandbox(self) -> SandboxConfig {
        SandboxConfig {
            max_operations: self.max_operations,
            execution_timeout: self.wall,
            ..SandboxConfig::default()
        }
    }
}

impl Default for ScriptBudget {
    fn default() -> Self {
        Self::of(BudgetKind::Interactive)
    }
}
