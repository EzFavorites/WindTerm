//! 「我们的文件放在哪」——全仓库唯一知道这件事的地方。
//!
//! # 为什么值得单独一个 crate（它只有十几行）
//!
//! 2026-11-12 的底层收敛：`term-runtime` 原来是一个 7579 行的杂货箱，
//! 里面 `persist`（会话库）、`log`（会话日志）、`prefs`（界面偏好）、
//! `layout_state`（窗口结构）各自都在算「profile 目录在哪」。
//!
//! 拆成两个 crate 时出现了唯一一条跨团依赖：`log`（会话线程侧）需要
//! `profile_dir`，而它属于持久化侧。如果让它依赖持久化层，就会出现
//! 「**为了知道日志写哪儿，会话线程要拖进 SQLite**」——依赖方向反了。
//!
//! 所以把这条规则提到两者**之下**：`term-paths` 只依赖 `dirs`，
//! `term-profile` 与 `term-io` 都依赖它，彼此不依赖。
//!
//! # 为什么一个函数就够
//!
//! 其余路径都是「profile 目录 + 一个文件名」，由各自的 crate 拼
//! （`term_profile::persist::sessions_path` 等）。这里只回答目录，
//! 不回答「目录里有什么」——后者每加一个文件就要改这个 crate，
//! 而文件名是各模块自己的知识。

use std::path::{Path, PathBuf};

/// 应用在平台配置目录下的子目录名。
///
/// 2026-11-18 之前这里直接返回 `dirs::config_dir()`，于是 macOS 上
/// `sessions.db` / `prefs.json` / `layout.json` 全被**平铺**写进
/// `~/Library/Application Support/` 根——和别的应用抢同一个目录，还留下一堆
/// 原子写的临时文件。用户裁定收进子目录，旧数据由 [`startup_tidy`] 复制过来。
const APP_DIR: &str = "WindTerm";

/// 首次启动「Choose Profiles Directory」写下的选择：一行绝对路径。
///
/// 它必须放在**选择之前就确定的位置**（平台配置目录/WindTerm），不能放进
/// 用户选中的 profile 目录里——否则下次启动还没读到它就算不出 profile 目录。
const CHOICE_FILE: &str = "profile-dir.txt";

/// profile 目录：会话库、保险库、偏好、布局、日志都在它下面。
///
/// 与 `sessions.db` / `vault.ztv` 同一个地方的原因很实际：用户要备份或整体搬走
/// 配置时，只有一个目录要处理。`WINDTERM_PROFILE_DIR` 因此也能把它们全部
/// 重定向到临时目录（测试用，也是「带一份便携配置」用的——macOS 上
/// `dirs::config_dir()` 忽略 `XDG_CONFIG_HOME`，所以覆盖必须显式）。
pub fn profile_dir() -> Option<PathBuf> {
    if let Ok(override_dir) = std::env::var("WINDTERM_PROFILE_DIR") {
        return Some(PathBuf::from(override_dir));
    }
    if let Some(chosen) = read_profile_dir_choice() {
        return Some(chosen);
    }
    Some(platform_dir()?.join(APP_DIR))
}

/// 首次启动的目录选择是否已经做过。
///
/// 环境变量覆盖也算「做过」：测试与便携模式用它重定向 profile 目录，不该再弹
/// 一次首次启动对话框。
pub fn has_profile_dir_choice() -> bool {
    std::env::var_os("WINDTERM_PROFILE_DIR").is_some()
        || choice_file()
            .is_some_and(|p| std::fs::read_to_string(p).is_ok_and(|s| !s.trim().is_empty()))
}

/// 读出用户选中的 profile 目录（不含首尾空白的空行视为没选）。
pub fn read_profile_dir_choice() -> Option<PathBuf> {
    let text = std::fs::read_to_string(choice_file()?).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| PathBuf::from(line))
}

/// 落下用户选中的 profile 目录。先写临时名再改名，中途失败不留下半截选择。
pub fn write_profile_dir_choice(dir: &Path) -> bool {
    let Some(file) = choice_file() else {
        return false;
    };
    write_choice_to(&file, dir)
}

fn choice_file() -> Option<PathBuf> {
    Some(platform_dir()?.join(APP_DIR).join(CHOICE_FILE))
}

fn write_choice_to(file: &Path, dir: &Path) -> bool {
    let Some(parent) = file.parent() else {
        return false;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return false;
    }
    let tmp = file.with_extension("tmp");
    let line = format!("{}\n", dir.to_string_lossy());
    if std::fs::write(&tmp, line).is_err() {
        return false;
    }
    std::fs::rename(&tmp, file).is_ok()
}

/// 平台配置目录（`profile_dir` 的**上一级**）。只有迁移需要它。
///
/// 单独留一个函数是为了让「旧位置」有唯一出处：迁移逻辑与测试都从这里拿，
/// 不要在别处再写一遍 `dirs::config_dir()`——那正是这次要收掉的东西。
fn platform_dir() -> Option<PathBuf> {
    dirs::config_dir().or_else(|| dirs::home_dir().map(|h| h.join(".config")))
}

/// 旧版平铺在配置目录根下的文件名（含 SQLite 的两个旁路文件与更早的明文格式）。
///
/// 顺序无所谓：只做「目标不存在才复制」，不删任何东西。
const LEGACY_FILES: &[&str] = &[
    "sessions.db",
    "sessions.db-wal",
    "sessions.db-shm",
    "prefs.json",
    "layout.json",
    "vault.ztv",
    // 之前的会话库格式：读到就仍能被新代码读出来，所以一并搬。
    "user.sessions",
];

/// 一次启动期整理的结果：搬了几个文件、清掉几个残留临时文件。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tidy {
    pub migrated: usize,
    pub cleaned: usize,
}

/// 启动期一次性整理：`profile_dir` 换到子目录后的**搬迁**，加上原子写残留的清理。
///
/// 两条都只做「加法」与「删自己写坏的东西」：搬迁是复制（旧文件原地不动，
/// 用户自己决定何时清），清理只认 `.xxx.tmp-…` 这个我们自己的前缀，
/// 且只删**一小时前**的——正在写的另一个实例还在用它的临时文件。
///
/// `WINDTERM_PROFILE_DIR` 在场时整体跳过：测试、截图与便携模式都用它指向
/// 一次性目录，那些场合既不该读用户的旧数据，更不该往真实目录里写。
pub fn startup_tidy() -> Tidy {
    if std::env::var_os("WINDTERM_PROFILE_DIR").is_some() {
        return Tidy::default();
    }
    let Some(target) = profile_dir() else {
        return Tidy::default();
    };
    let migrated = match platform_dir() {
        // 平台配置目录就是我们自己时（极少见的 home 兜底），没有「旧位置」可搬。
        Some(legacy) if legacy != target => migrate_between(&legacy, &target),
        _ => 0,
    };
    let mut cleaned = clean_between(&target, std::time::Duration::from_secs(3600));
    // 旧位置也清一遍：搬迁之后不会再往根目录写新临时文件，但历史残留得收掉。
    // 那里是**共享目录**，所以只认我们自己的四类前缀（见 [`TMP_PREFIXES`]），
    // 绝不按「所有 .xxx.tmp-」批量删——那会动到别的应用正在写的中间文件。
    if let Some(legacy) = platform_dir() {
        if legacy != target {
            cleaned += clean_between(&legacy, std::time::Duration::from_secs(3600));
        }
    }
    Tidy { migrated, cleaned }
}

/// 把 `legacy` 下的存量文件复制到 `target`，**只在目标不存在时**复制。
fn migrate_between(legacy: &std::path::Path, target: &std::path::Path) -> usize {
    let mut moved = 0;
    for name in LEGACY_FILES {
        let from = legacy.join(name);
        let to = target.join(name);
        if !from.is_file() || to.exists() {
            continue;
        }
        if std::fs::create_dir_all(target).is_err() {
            return moved;
        }
        // 先写临时名再改名，中途失败不会留下一个「看起来完整」的半截库。
        let tmp = to.with_file_name(format!(".{name}.migrating"));
        if std::fs::copy(&from, &tmp).is_ok() && std::fs::rename(&tmp, &to).is_ok() {
            moved += 1;
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    // 用户脚本是**自己写的内容**，不是缓存，一并搬（只搬顶层文件，不跟软链）。
    moved + copy_dir(&legacy.join("scripts"), &target.join("scripts"))
}

/// 只复制顶层普通文件；同名目标存在时跳过。返回复制成功的个数。
fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(from) else {
        return 0;
    };
    let mut moved = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let target = to.join(entry.file_name());
        if target.exists() {
            continue;
        }
        if std::fs::create_dir_all(to).is_ok() && std::fs::copy(&path, &target).is_ok() {
            moved += 1;
        }
    }
    moved
}

/// 原子写残留的**完整前缀**白名单：`.<文件名>.tmp-`。
///
/// 只认这四个，不按「所有 `.xxx.tmp-`」批量删——旧位置是平台配置目录的**根**，
/// 同一层还有别的应用，宽口径会删掉别人正在写的中间文件。
const TMP_PREFIXES: &[&str] = &[
    ".layout.json.tmp-",
    ".prefs.json.tmp-",
    ".sessions.db.tmp-",
    ".vault.ztv.tmp-",
];

/// 这个文件名是不是我们自己写坏的中间产物。
///
/// 两种：原子写残留 `.<文件名>.tmp-…`，以及搬迁用的 `.<文件名>.migrating`
/// （复制成功、改名失败时会留下；进程被杀也会）。**两种都只认白名单里的文件名**，
/// 因为清理会扫到旧位置——那是平台配置目录的根，同一层还有别的应用。
fn is_own_temp(name: &str) -> bool {
    if TMP_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
        return true;
    }
    let Some(stem) = name.strip_suffix(".migrating") else {
        return false;
    };
    stem.strip_prefix('.')
        .is_some_and(|base| LEGACY_FILES.contains(&base))
}

/// 删掉 `dir` 里超过 `min_age` 的自有残留（见 [`is_own_temp`]）。
///
/// 判据只看文件名与年龄，不看内容——它们是 [`write_atomic`] 与 [`migrate_between`]
/// 的中间产物，不是用户数据。年龄下限是为了不误删另一个实例正在写的那一份。
fn clean_between(dir: &std::path::Path, min_age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_own_temp(&name) {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= min_age);
        if old_enough && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 脚本库目录：`profile_dir()/scripts`。
///
/// 新建脚本默认落这里，而不是工作目录——否则从任何 CWD 启动，「新建脚本」都会
/// 写进一个随启动目录漂移的地方，用户第二次就找不到自己的脚本了。目录本身按需
/// 创建（`script_run::save_source` 写之前 `create_dir_all`），这里只回答路径。
pub fn scripts_dir() -> Option<PathBuf> {
    Some(profile_dir()?.join("scripts"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量是全进程共享的，所以所有读写 `WINDTERM_PROFILE_DIR` 的用例都要
    /// 串起来跑——用一把**模块级**锁把「设 → 读 → 还原」包住，否则并发的两条
    /// 用例会互相看见对方的目录（早先每个用例各持一把锁，等于没锁）。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_profile_dir<F: FnOnce()>(dir: &str, check: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var("WINDTERM_PROFILE_DIR").ok();
        std::env::set_var("WINDTERM_PROFILE_DIR", dir);
        check();
        match previous {
            Some(v) => std::env::set_var("WINDTERM_PROFILE_DIR", v),
            None => std::env::remove_var("WINDTERM_PROFILE_DIR"),
        }
    }

    #[test]
    fn the_override_wins_over_the_platform_config_dir() {
        with_profile_dir("/tmp/term-paths-probe", || {
            let got = profile_dir();
            assert_eq!(got.as_deref(), Some(std::path::Path::new("/tmp/term-paths-probe")));
        });
    }

    #[test]
    fn a_written_profile_dir_choice_round_trips() {
        let root = std::env::temp_dir().join(format!(
            "term-paths-choice-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let file = root.join(APP_DIR).join(CHOICE_FILE);
        let chosen = Path::new("/tmp/my-wind-profile/.wind");
        assert!(write_choice_to(&file, chosen));
        let read = std::fs::read_to_string(&file).unwrap();
        assert!(read.starts_with("/tmp/my-wind-profile/.wind"));
        assert!(read.trim().ends_with(".wind"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scripts_dir_sits_under_the_profile_dir() {
        with_profile_dir("/tmp/term-paths-scripts-probe", || {
            let got = scripts_dir();
            assert_eq!(
                got.as_deref(),
                Some(std::path::Path::new("/tmp/term-paths-scripts-probe/scripts"))
            );
        });
    }

    /// 没覆盖时，默认目录必须是平台配置目录**下的子目录**。
    ///
    /// 这条钉的是 2026-11-18 的收口：此前 `profile_dir()` 直接返回
    /// `dirs::config_dir()`，`sessions.db` 等被平铺进
    /// `~/Library/Application Support/` 根（和别的应用抢目录）。
    #[test]
    fn the_default_profile_dir_sits_under_the_app_subfolder() {
        // 先拿同一把锁再判断：否则「读 env → 判空 → 再读 profile_dir」之间
        // 可能被另一条用例插进来设覆盖，拿到的就不是默认路径了。
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if std::env::var_os("WINDTERM_PROFILE_DIR").is_some()
            || read_profile_dir_choice().is_some()
        {
            // 用户已经选过目录（或环境变量覆盖）时，默认路径分支根本不走。
            return;
        }
        let dir = profile_dir();
        let dir = dir.expect("平台配置目录应当可得");
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some(APP_DIR));
        assert_eq!(dir.parent(), platform_dir().as_deref());
    }

    /// 每个用例一个独立临时目录。
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "term-paths-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 搬迁是**复制**：旧文件原地不动，目标拿到同名的副本。
    #[test]
    fn migrating_copies_legacy_files_and_leaves_the_originals() {
        let legacy = scratch("mig-legacy");
        let target = scratch("mig-target");
        std::fs::write(legacy.join("sessions.db"), b"db").expect("写旧库");
        std::fs::write(legacy.join("prefs.json"), b"{}").expect("写旧偏好");
        std::fs::write(legacy.join("unrelated.txt"), b"x").expect("写无关文件");
        std::fs::create_dir_all(legacy.join("scripts")).expect("建 scripts");
        std::fs::write(legacy.join("scripts/a.rhai"), b"print(1)").expect("写脚本");

        let moved = migrate_between(&legacy, &target);

        assert_eq!(moved, 3, "两个数据文件 + 一个脚本");
        assert_eq!(std::fs::read(target.join("sessions.db")).expect("新库"), b"db");
        assert_eq!(std::fs::read(target.join("prefs.json")).expect("新偏好"), b"{}");
        assert!(target.join("scripts/a.rhai").is_file(), "用户脚本要一起搬");
        assert!(!target.join("unrelated.txt").exists(), "只搬白名单里的文件");
        assert!(legacy.join("sessions.db").is_file(), "旧文件必须原地保留");
    }

    /// 目标已有同名文件时不覆盖：新目录里的数据永远优先（用户可能已经跑过新版）。
    #[test]
    fn migrating_never_overwrites_a_newer_file() {
        let legacy = scratch("mig2-legacy");
        let target = scratch("mig2-target");
        std::fs::write(legacy.join("sessions.db"), b"old").expect("写旧库");
        std::fs::write(target.join("sessions.db"), b"new").expect("写新库");

        let moved = migrate_between(&legacy, &target);

        assert_eq!(moved, 0);
        assert_eq!(std::fs::read(target.join("sessions.db")).expect("新库"), b"new");
    }

    /// 清理只认我们自己写的临时名，且只删够老的。
    #[test]
    fn cleanup_removes_only_our_tmp_leftovers() {
        let dir = scratch("clean");
        std::fs::write(dir.join(".layout.json.tmp-1-2-3"), b"x").expect("写残留");
        // 搬迁中途被杀会留下这个（复制成功、改名没做完）。
        std::fs::write(dir.join(".sessions.db-shm.migrating"), b"x").expect("写搬迁残留");
        std::fs::write(dir.join("sessions.db"), b"db").expect("写库");
        std::fs::write(dir.join(".DS_Store"), b"x").expect("写杂物");
        // 旧位置是共享目录，别的应用也会在那一层留 `.xxx.tmp-`：必须原样留着。
        std::fs::write(dir.join(".foreign-config.tmp-1-2-3"), b"x").expect("写别人的临时文件");

        // 年龄下限给 0：等价于「全都算老」。
        assert_eq!(clean_between(&dir, std::time::Duration::ZERO), 2);
        assert!(!dir.join(".layout.json.tmp-1-2-3").exists(), "残留该被删掉");
        assert!(!dir.join(".sessions.db-shm.migrating").exists(), "搬迁残留也该被删掉");
        assert!(dir.join("sessions.db").is_file(), "用户数据不许动");
        assert!(dir.join(".DS_Store").is_file(), "不是我们的临时名，不许动");
        assert!(
            dir.join(".foreign-config.tmp-1-2-3").is_file(),
            "别的应用的临时文件不许动"
        );
    }

    /// 一小时内写过的不删——可能是另一个实例正在写的文件。
    #[test]
    fn cleanup_keeps_fresh_temp_files() {
        let dir = scratch("clean-fresh");
        std::fs::write(dir.join(".prefs.json.tmp-9-9-9"), b"x").expect("写残留");

        let removed = clean_between(&dir, std::time::Duration::from_secs(3600));

        assert_eq!(removed, 0);
        assert!(dir.join(".prefs.json.tmp-9-9-9").is_file());
    }
}
