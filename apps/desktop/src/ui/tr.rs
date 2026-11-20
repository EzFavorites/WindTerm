//! 投影层的取词入口：**所有界面文案都从这里过**，不要在绘制处直接写中文。
//!
//! 单独一个文件是因为：`ui/mod.rs` 已经贴着 500 行上限了。
//! 这里只回答两件事——「当前是哪门语言」「这句英文在中文里怎么说」。
//!
//! `tr()` 只读、不缓存：`ViewState.lang` 每帧由 reducer 唯一写入，切换语言
//! 不需要任何「清空文案缓存」的额外步骤（这也是不用 key 枚举的一个好处）。

use crate::ui::*;
use term_app::i18n::{self, Lang};

/// 给脱离 `TerminalWindow` 的绘制函数用的取词入口（Find 栏、浮层面板等）。
/// 它们只拿到一个 `Lang`，但取词规则必须与 `TerminalWindow::tr` 完全同一份。
pub(in crate::ui) fn t(lang: Lang, en: &'static str) -> &'static str {
    i18n::t(lang, en)
}

impl TerminalWindow {
    /// 当前界面语言。领域状态，不是投影态。
    pub(in crate::ui) fn lang(&self) -> Lang {
        self.view.lang
    }

    /// 英文原文 → 当前语言的文案。缺词退回英文原文。
    pub(in crate::ui) fn tr(&self, en: &'static str) -> &'static str {
        i18n::t(self.view.lang, en)
    }
}
