//! L3（`docs/design-specs/O-crash-hardening.md` §3d / §3e）：`layout.json` 的
//! **落盘元信息**——最近一次写盘的时间与上一次的退出形态。
//!
//! # 为什么它们不进 `SavedLayout`
//!
//! 与 `schema_version` 同一性质：它们是**文件的属性**，不是布局的语义字段。塞进
//! 结构体就要在每个构造点（含十几处测试）写一个自己根本没有意见的数字，而那个
//! 数字一旦被某个调用点写成旧值，文件就带着错的时间戳落盘——比现在这样更难守。
//!
//! # 为什么不和 `layout_state` 放一个文件
//!
//! 那一份已经顶到 R9 的单文件行数上限，而这两件事的关注点本来就不同：一个是
//! 「布局长什么样」，一个是「这份文件什么时候写的、上次是不是正常退出」。
//!
//! 调用方仍然只认 `term_profile::layout_state` 一个入口——它在 `layout_state.rs`
//! 里把这些名字再导出了一遍。

use std::path::Path;

use crate::layout_state::layout_path;

/// `last_exit_kind` 的「正常退出」。写成字符串而不是枚举：将来多一种形态
/// （比如 `"killed"`）不该让老程序读不出整份文件。
pub const EXIT_CLEAN: &str = "clean";
/// 没走正常收口（panic / 被外部杀掉 / 断电）。`layout.json` 里缺这一格也算。
pub const EXIT_CRASH: &str = "crash";

/// 恢复提示的时效窗口：超过这个岁数的「非正常退出」不再提示——三天前的一次
/// 崩溃与今天的启动无关，弹出来只是噪音。
pub const RECENT_WINDOW_SECS: u64 = 300;

/// 落盘元信息。两个 `Option` 都表示「文件里没有这一格」（老文件 / 从没写过）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayoutMeta {
    /// 最近一次写盘的 Unix 秒。
    pub saved_at: Option<u64>,
    /// 上次退出形态：`EXIT_CLEAN` / `EXIT_CRASH` / `None`（被打断）。
    pub exit_kind: Option<String>,
}

impl LayoutMeta {
    /// 上次是不是走了正常退出。缺这一格 = 不是。
    pub fn is_clean(&self) -> bool {
        self.exit_kind.as_deref() == Some(EXIT_CLEAN)
    }

    /// 要不要在启动时提示「已恢复最近留存点」。
    ///
    /// `now` 由宿主注入（本 crate 不读墙钟，ADR-0014 的同一口径）：判断需要
    /// 「现在几点」，而那是宿主的事实。
    pub fn should_announce(&self, now: u64) -> bool {
        let Some(at) = self.saved_at else {
            return false;
        };
        !self.is_clean() && now.saturating_sub(at) < RECENT_WINDOW_SECS
    }
}

/// 只改 `last_exit_kind`，其它字段（含时间戳）一个字节不动。
///
/// 退出路径**只能**走这里：那时标签已经全部收口，`SavedLayout` 代表的是一份空
/// 布局——拿它去 `save` 等于把用户昨天下次启动要恢复的东西抹掉。
pub fn mark_exit(kind: &str) -> bool {
    patch_exit(Some(kind))
}

/// 把上一次运行的退出形态**消费掉**（启动时调用）。
///
/// 语义：那条事实只对「紧接着的这一次启动」有意义。不清掉的话，一次干净退出
/// 之后又被 `kill -9` 的运行会被判成 clean（因为文件里还留着上一轮的 clean），
/// 该提示的时候反而静默。
pub fn clear_exit() -> bool {
    patch_exit(None)
}

/// 读落盘元信息。文件不存在 / 读不出 / 不是对象 → `None`（与 `load` 同一口径：
/// 没有就是没有，不猜）。
pub fn load_meta() -> Option<LayoutMeta> {
    match layout_path() {
        Some(path) => load_meta_from(&path),
        None => None,
    }
}

fn patch_exit(kind: Option<&str>) -> bool {
    match layout_path() {
        Some(path) => patch_exit_at(&path, kind),
        None => false,
    }
}

pub(crate) fn patch_exit_at(path: &Path, kind: Option<&str>) -> bool {
    // 没有布局文件就没有「上次退出」可言：不为一条元信息凭空造一份空布局。
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let Some(map) = value.as_object_mut() else {
        return false;
    };
    match kind {
        Some(kind) => {
            map.insert("last_exit_kind".to_string(), serde_json::Value::from(kind));
        }
        None => {
            map.remove("last_exit_kind");
        }
    }
    let Ok(json) = serde_json::to_string_pretty(&value) else {
        return false;
    };
    crate::persist::write_atomic(path, json.as_bytes())
}

pub(crate) fn load_meta_from(path: &Path) -> Option<LayoutMeta> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let map = value.as_object()?;
    Some(LayoutMeta {
        saved_at: map
            .get("last_saved_at")
            .and_then(serde_json::Value::as_u64),
        exit_kind: map
            .get("last_exit_kind")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_state::SavedLayout;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// 与 `layout_state` 的测试同形：一份独立的临时目录 + 其中的 `layout.json`。
    fn tmp() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "windterm-layout-meta-{}-{}-{}",
            std::process::id(),
            nanos,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(crate::layout_state::LAYOUT_FILE)
    }

    /// L3（O 设计稿 §3d/§3e）：带戳落盘 + 「要不要提示」的判定。
    ///
    /// 三个分支都要钉：非 clean 且新鲜 → 提示；clean → 不提示（用户是正常退出，
    /// 不该被吓一跳）；太旧 → 不提示（三天前的崩溃与今天的启动无关）。
    #[test]
    fn a_stamped_save_announces_only_an_unclean_recent_exit() {
        let path = tmp();
        assert!(crate::layout_state::save_to_stamped(&path, &SavedLayout::single("a"), Some(1_000)));
        let meta = load_meta_from(&path).expect("盖了戳就该读得出来");
        assert_eq!(meta.saved_at, Some(1_000));
        assert_eq!(meta.exit_kind, None, "运行期落盘不该写退出形态");
        // 距落盘 20s、且没写过 clean → 该提示。
        assert!(meta.should_announce(1_020));
        // 距落盘 10 分钟 → 时效外，不提示。
        assert!(!meta.should_announce(1_000 + RECENT_WINDOW_SECS + 1));

        // 正常退出之后：即使刚写盘也不提示。
        assert!(patch_exit_at(&path, Some(EXIT_CLEAN)));
        let clean = load_meta_from(&path).expect("改完还能读");
        assert!(clean.is_clean());
        assert!(!clean.should_announce(1_001));

        // 从没写过时间戳的老文件 → 无从判断新鲜度，不提示。
        let bare = LayoutMeta::default();
        assert!(!bare.should_announce(1_000));
    }

    /// 退出路径**只改元信息**：布局本身（标签 / 树 / 停靠）一个字节不动。
    ///
    /// 这条守的是一个真实事故面——`on_exit` 里标签已经全部收口，谁要是图省事
    /// 在那里 `save` 一份当前 `SavedLayout`，写下去的就是一份空布局。
    #[test]
    fn marking_the_exit_keeps_the_layout_and_clearing_consumes_the_fact() {
        let path = tmp();
        let layout = SavedLayout::single("prod-web");
        assert!(crate::layout_state::save_to_stamped(&path, &layout, Some(500)));
        assert!(patch_exit_at(&path, Some(EXIT_CLEAN)));
        assert_eq!(crate::layout_state::load_from(&path), Some(layout.clone()), "退出标记不能动布局");
        assert_eq!(
            load_meta_from(&path).and_then(|m| m.exit_kind),
            Some(EXIT_CLEAN.to_string())
        );

        // 下一次启动把它消费掉：那条事实只对紧接着的这一次启动有意义。
        assert!(patch_exit_at(&path, None));
        assert_eq!(
            load_meta_from(&path).and_then(|m| m.exit_kind),
            None,
            "消费之后不该还留着上一轮的 clean"
        );
        assert_eq!(crate::layout_state::load_from(&path), Some(layout), "消费也不能动布局");
    }
}
