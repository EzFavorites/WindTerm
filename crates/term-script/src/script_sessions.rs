//! 按会话共享的**脚本游标登记处**（ADR-0030 二轮审查缺口 2）。
//!
//! 历史上两份游标各活在自己的宿主里：`ScriptWorkspace` 的
//! `Tabs.map` 与 `DeskSurface` 的 `cursors`。同一条会话被脚本和 API
//! 先后 `read_until` 时，两边各从自己的「上次取走点」往后读：第二边读不到
//! 第一边刚消费过的文本，甚至会把同一段输出重复交出去。
//!
//! 这里把登记处变成一件**可传递的宿主资产**：调用方构造一份
//! [`ScriptSessions`]，装配时把同一份句柄交给 [`ScriptWorkspace`] 与
//! `DeskSurface`，于是「同一会话的脚本游标与 API 游标是同一份实例」
//! 在结构上成立，而不是靠调用顺序约定。
//!
//! # 为什么登记的是 `ScriptSession` 而不是裸 `ScanCursor`
//!
//! `ScriptSession` 把「会话句柄 + 捕获游标 + 取消 / 截止策略」绑成一个对象。
//! 只共享裸游标会让「构造即登记 reader」这件事重新分裂：两边各自注册一个
//! reader，各有各的起点，仍会漏读窗口期输出。共享 `ScriptSession` 则连
//! reader 的注册点也只有一份。
//!
//! # 截止点 / 取消令牌不随登记处走
//!
//! 登记处里放的是**中性策略**（`ScriptSession::new`）的那一份。脚本侧每次
//! 取句柄时用 [`ScriptSession::for_run`] 派生一份带本次运行策略的视图：
//! 会话句柄、捕获游标、读序列化门都是 `Arc` 克隆，只有 `deadline` / `cancel`
//! 换成本次运行的。因此 API 侧的一次调用不会延长某次脚本的墙钟预算，
//! 脚本结束也不会给后续 API 调用留下一个已经过期的截止点。
//!
//! # 并发读的序列化策略
//!
//! 共享一份游标后，两个前端同时 `read_until` 会让同一个 `ScanCursor` 被两条
//! 线程各推一次，谁拿到哪段文本取决于调度——这不是可接受的语义。
//! [`ScriptSession`] 因此带一把随克隆共享的 `read_gate`：所有**消费式**读
//! 原语（`read` / `read_until` / `wait_any` / `wait_idle` / `settle`）在整段
//! 等待期间持门，第二个前端排队到前一个取完（或超时）再读。不合并、不抢占，
//! 调用方看到的是稳定的「先到先读」。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use term_core::session_runtime::SessionId;

use crate::session::ScriptSession;

/// 一份可在多个前端之间传递的按会话游标登记处。
///
/// `Clone` 只克隆 `Arc` 句柄：所有克隆指向同一张表、同一批 `ScriptSession`。
#[derive(Clone)]
pub struct ScriptSessions {
    map: Arc<Mutex<HashMap<SessionId, ScriptSession>>>,
}

impl ScriptSessions {
    /// 宿主构造的一份**新**登记处。用于进程级装配（`desk serve` 的 accept
    /// 循环、脚本侧的独立工作区）与单测隔离。
    pub fn new() -> Self {
        Self {
            map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 宿主级那一份（进程只造一次）。
    ///
    /// 界面里的 `RunOptions::default()` 与外部 Python 的装配都取这里的句柄，
    /// 所以即使调用方分处不同的模块，脚本与 API 仍共用同一张游标表。
    /// `main` 在启动时显式取一次，把「宿主只有一份」变成可见的装配事实。
    pub fn shared() -> &'static Self {
        static SHARED: OnceLock<ScriptSessions> = OnceLock::new();
        SHARED.get_or_init(Self::new)
    }

    /// 同一张表的另一个句柄（避免调用方写 `clone()` 时看不出意图）。
    pub fn handle(&self) -> Self {
        self.clone()
    }

    /// 已登记则取回，未登记返回 `None`。**绝不**退回别的会话。
    pub fn get(&self, id: SessionId) -> Option<ScriptSession> {
        self.lock().get(&id).cloned()
    }

    /// 一条会话是否已经在这张表里。
    pub fn contains(&self, id: SessionId) -> bool {
        self.lock().contains_key(&id)
    }

    /// 取一份已登记的会话；缺席时**只调用一次** `make` 并原子入表。
    ///
    /// 闭包在锁内执行，两条线程同时抢同一条会话时只会有一个 `ScriptSession`
    /// 被构造——构造即登记 reader，重复构造会留下两个起点不同的 reader，
    /// 那正是本登记处要消灭的漏洞。`make` 失败（会话不在池子里）时原样把
    /// 错误交回去，由调用方报 `not_found`，表里不留半个条目。
    pub fn get_or_try_insert<E, F>(&self, id: SessionId, make: F) -> Result<ScriptSession, E>
    where
        F: FnOnce() -> Result<ScriptSession, E>,
    {
        let mut map = self.lock();
        if let Some(existing) = map.get(&id) {
            return Ok(existing.clone());
        }
        let session = make()?;
        Ok(map.entry(id).or_insert(session).clone())
    }

    /// 缺席时才构造；已存在时保留原条目（它的读游标可能已经推进过）。
    /// 与 [`Self::get_or_try_insert`] 同一条纪律：闭包在锁内、至多跑一次。
    pub fn get_or_insert_with<F>(&self, id: SessionId, make: F) -> ScriptSession
    where
        F: FnOnce() -> ScriptSession,
    {
        let mut map = self.lock();
        map.entry(id).or_insert_with(make).clone()
    }

    /// 摘掉一条会话的游标。关会话时调用，避免死句柄攥着会话本体不放。
    pub fn remove(&self, id: SessionId) -> Option<ScriptSession> {
        self.lock().remove(&id)
    }

    /// 登记条数（测试与诊断用）。
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<SessionId, ScriptSession>> {
        match self.map.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Default for ScriptSessions {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ScriptSessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptSessions")
            .field("sessions", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fake_session;
    use std::time::Duration;

    #[test]
    fn one_session_is_built_once_and_all_handles_see_the_same_cursor() {
        let registry = ScriptSessions::new();
        let (session, manip) = fake_session(7);
        let mut builds = 0;
        let mut make = || {
            builds += 1;
            Ok::<_, ()>(ScriptSession::with_max_wait(
                Arc::clone(&session),
                Duration::from_secs(2),
            ))
        };
        let first = registry.get_or_try_insert(7, &mut make).expect("首次建成功");
        let second = registry.get_or_try_insert(7, &mut make).expect("复用已有");
        assert_eq!(builds, 1, "同一会话只建一次 ScriptSession");
        assert_eq!(registry.len(), 1);
        assert!(registry.get(8).is_none(), "不认识别的会话，也不能退回 7");

        manip.push_output("first#second#");
        assert_eq!(first.read_until("#", 2.0, None).expect("第一段"), "first#");
        assert_eq!(second.read_until("#", 2.0, None).expect("续读"), "second#");
    }
}
