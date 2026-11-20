//! Font loading and cell metrics.
//!
//! The original's `text.fontFamily1` is "Roboto Mono" with a fallback list; this
//! build loads a real monospace face plus a CJK face from the system. The grid's
//! cell size is measured from the font itself (`glyph_width('M')`,
//! `row_height`) instead of guessed, and a CJK glyph is treated as two cells
//! wide only when the loaded face really advances it by more than one cell.
//!
//! # 2026-11-19：四槽字族 + 字重（取值来自 `catalog` 子模块）
//!
//! 设置页 `Font family 1–4` 与 `Font weight` 从灰显接成真控件之后，这一层不再
//! 只认写死的三条候选路径，而是按**用户选的字族**装字：
//!
//! - 四槽就是视觉基线的四行：槽 0 是主字面，槽 1–3 是缺字时的回退（CJK 通常在
//!   槽 1）。空槽 = 自动（槽 0 走内置等宽候选，槽 1 走内置 CJK 候选）。
//! - 字重作用于**槽 0**：`Font weight` 是「终端主体字」的粗细，回退字族按各自
//!   的常规面装——把 Hiragino 的 W6 拿来配 Menlo Bold 只会让中英混排忽粗忽细。
//! - 网格用**独立的字族名** `windterm-terminal`，不直接占 `Monospace`：UI 的
//!   菜单/标签也挂在 `Monospace` 上（`kit::type_scale`），字重若直接装进
//!   `Monospace`，用户把终端设成 Bold 会把整个界面一起加粗。

pub mod catalog;

use catalog::{Catalog, DEFAULT_WEIGHT};
use eframe::egui;

/// 四槽字族的槽数（视觉基线 `Font family 1–4`）。
pub const FAMILY_SLOTS: usize = 4;

/// 网格使用的字族名。装进 `FontDefinitions::families` 的那个键。
const TERMINAL_FAMILY: &str = "windterm-terminal";

/// Point size used for the grid (`text.fontSize` in a conventional terminal's config).
///
/// 直接引 `term_core::config` 的那一份：设置页的 0 哨兵回落、命令中心的
/// 「默认字号」都认它，渲染层再抄一个 13.0 就成了第二份真相——改一处、
/// 另一处悄悄不跟。
pub const TERMINAL_FONT_SIZE: f32 = term_core::config::DEFAULT_FONT_SIZE;

/// R14-b：字号缩放的上下限（太小会糊成一团，太大占满屏）。
///
/// 区间与设置页（`Font ▸ Font size` 的滑块）和 `term_runtime::prefs` 的
/// `FONT_SIZE_MIN/MAX` **必须一致**：页面按 6–48 画，渲染层却夹在 8–40，
/// 用户拖到 44 就会看到 44、画成 40。缩放仍只在 [MIN, MAX] 内 ±1 步进。
pub const MIN_FONT_SIZE: f32 = 6.0;
pub const MAX_FONT_SIZE: f32 = 48.0;

/// Candidates in preference order; the first readable file wins. `u32` is the
/// face index inside a `.ttc` collection.
///
/// 为什么是函数而不是 `const` 表：Windows 的系统目录未必在 `C:`，路径要从
/// `%SystemRoot%` 拼出来——常量表拼不了。
#[cfg(not(windows))]
fn mono_candidates() -> Vec<(String, u32)> {
    [
        "/System/Library/Fonts/Menlo.ttc",
        "/Library/Fonts/Monaco.ttf",
        "/System/Library/Fonts/Supplemental/Courier New.ttf",
    ]
    .iter()
    .map(|path| ((*path).to_string(), 0))
    .collect()
}

/// Consolas 是 Vista 起的等宽默认；Lucida Console 是从 XP 就有的兜底。
#[cfg(windows)]
fn mono_candidates() -> Vec<(String, u32)> {
    windows_fonts(&["consola.ttf", "lucon.ttf"])
}

#[cfg(not(windows))]
fn cjk_candidates() -> Vec<(String, u32)> {
    [
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/System/Library/Fonts/Songti.ttc",
        "/System/Library/Fonts/PingFang.ttc",
    ]
    .iter()
    .map(|path| ((*path).to_string(), 0))
    .collect()
}

/// `msyh.ttc` 的 0 号面是微软雅黑 Regular（Vista 起自带）；SimSun 兜底更老的系统。
/// 没有这一条，Windows 上终端里的中文全是方框——egui 自带字形不含 CJK。
#[cfg(windows)]
fn cjk_candidates() -> Vec<(String, u32)> {
    windows_fonts(&["msyh.ttc", "msyh.ttf", "simsun.ttc"])
}

/// `%SystemRoot%\Fonts\<name>`，保持传入顺序。
///
/// 回落 `C:\Windows` 只是防御：Windows 装在别的盘时 `SystemRoot` 一定存在，
/// 真拿不到这个变量也就只能试默认位置了。
#[cfg(windows)]
fn windows_fonts(names: &[&str]) -> Vec<(String, u32)> {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    names
        .iter()
        .map(|name| (format!(r"{root}\Fonts\{name}"), 0))
        .collect()
}

fn read_face(path: &str, index: u32) -> Option<egui::FontData> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 1024 {
        return None;
    }
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index;
    Some(data)
}

fn first_readable(candidates: &[(String, u32)]) -> Option<(egui::FontData, String)> {
    for (path, index) in candidates {
        if let Some(data) = read_face(path, *index) {
            let label = std::path::Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            return Some((data, label));
        }
    }
    None
}

/// 一槽装好的字面。
#[derive(Clone)]
pub struct Slot {
    /// 生效的字族名（自动挑到的，或用户选的那个）。
    pub family: String,
    /// 生效的字重（OS/2 档位）：网格实际用哪一档。
    pub weight: u16,
    /// 这一槽的**常规面**字重。网格那份字重面与它相同时不必另存一份文件。
    pub regular_weight: u16,
    /// 该面自报的子族名，设置页的悬停里显示。
    pub style: String,
    data: egui::FontData,
}

/// 一槽的「请求 → 生效」对照，给设置页显示「没装，回落到了谁」。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlotInfo {
    /// 用户选的名字（空 = 自动）。
    pub requested: String,
    /// 真正装上的字族名。
    pub family: String,
    /// 真正装上的字重。
    pub weight: u16,
    /// 选的名字没装，已经回落到内置候选。
    pub fell_back: bool,
}

/// 下拉里的一行：字族名 + 是否等宽（等宽的在列表里画粗体，
/// 对应视觉基线那句 `Bold type indicates fixed-width fonts:`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FamilyChoice {
    pub name: String,
    pub monospace: bool,
}

/// 内置候选：槽 0（等宽）与槽 1（CJK）。
type Builtin = (Option<(egui::FontData, String)>, Option<(egui::FontData, String)>);

pub struct TerminalFonts {
    /// 四槽的目标字族名（空 = 自动）。与 `Prefs.font_family` 同序。
    want: [String; FAMILY_SLOTS],
    /// 目标字重（OS/2 档位）。只作用于槽 0。
    want_weight: u16,
    /// 四槽实际装上的字面。
    slots: [Option<Slot>; FAMILY_SLOTS],
    /// 本机字面清单。`None` = 还没扫过（见 `catalog` 模块头的「代价」）。
    catalog: Option<Catalog>,
    /// 下拉用的字族快照（名字 + 是否等宽），与 `catalog` 同时填。
    ///
    /// 单独存一份 `Vec` 而不是每帧从 `catalog` 映射：设置页每帧都要画像素，
    /// 八百个 `String` 每帧克隆一遍纯属浪费；而 `&Catalog` 一借出去，页面
    /// 里就再也拿不到 `&mut fonts` 做别的判断。
    choices: Vec<FamilyChoice>,
    /// R14-b：当前字号（点）。出厂默认 `TERMINAL_FONT_SIZE`，缩放时改这里，
    /// 由 `install` 侦测变化后重装字形、`measure_once` 重新度量 cell 盒。
    size: f32,
    /// Cell width in logical points, measured from the mono face.
    pub cell_w: f32,
    /// Row height in logical points, measured from the mono face.
    pub cell_h: f32,
    /// How many cells one CJK glyph covers, measured (1 or 2).
    pub cjk_cells: usize,
    /// Set once `install` has handed the definitions to egui.
    installed: bool,
    /// The device pixel ratio the definitions were installed for. A window
    /// moving to another screen changes it, which asks for a re-install and a
    /// fresh measurement.
    installed_ppp: f32,
    /// The point size the definitions were installed for (R14-b：缩放后必须重装)。
    installed_size: f32,
    /// 这份 `FontDefinitions` 已经量过 cell 盒。
    measured: bool,
    /// `windterm-terminal` 这个名字**已经在这个上下文里量过一次**，可以拿来
    /// 取字形了。见 `font_id` 的注释：没量过（例如测试直接画面板、没走
    /// `install`）时退回 `Monospace`，否则 `epaint` 会 panic。
    ready: bool,
}

impl Default for TerminalFonts {
    fn default() -> Self {
        Self::load()
    }
}

impl TerminalFonts {
    pub fn load() -> Self {
        Self::load_with(Default::default(), DEFAULT_WEIGHT)
    }

    /// 按落盘的偏好装字。启动时调一次（`construct.rs`）。
    ///
    /// 字族名非空会触发一次清单扫描——只有「用户真的选过字族」才会走到，
    /// 默认安装四槽全空，一次 IO 都不多。
    pub fn load_with(families: [String; FAMILY_SLOTS], weight: u16) -> Self {
        let mut fonts = Self {
            want: families,
            want_weight: normalize_weight(weight),
            slots: Default::default(),
            catalog: None,
            choices: Vec::new(),
            size: TERMINAL_FONT_SIZE,
            cell_w: 7.8,
            cell_h: 16.0,
            cjk_cells: 2,
            installed: false,
            installed_ppp: 0.0,
            installed_size: TERMINAL_FONT_SIZE,
            measured: false,
            ready: false,
        };
        fonts.reload();
        fonts
    }

    /// 按 `want` / `want_weight` 重新装四槽。
    ///
    /// 选过的字族要在清单里查，所以「有任一槽非空」时先扫一次（之后复用）。
    fn reload(&mut self) {
        if self.want.iter().any(|n| !n.trim().is_empty()) {
            self.scan_catalog();
        }
        let builtin = (first_readable(&mono_candidates()), first_readable(&cjk_candidates()));
        for i in 0..FAMILY_SLOTS {
            self.slots[i] = self.load_slot(i, &builtin);
        }
        self.measured = false;
    }

    /// 装一槽。`builtin` 是零号槽（等宽）与一号槽（CJK）的自动候选。
    fn load_slot(&self, slot: usize, builtin: &Builtin) -> Option<Slot> {
        let want = self.want[slot].trim().to_string();
        // 字重只作用于主字族（见模块头）。
        let target = if slot == 0 { self.want_weight } else { DEFAULT_WEIGHT };
        // ① 用户点名了字族：在清单里查；查不到（或文件读不出来）就往下走自动。
        if !want.is_empty() {
            if let Some(found) = self.face_slot(&want, target) {
                return Some(found);
            }
        }
        // ② 自动 / 回落：槽 0 用内置等宽，槽 1 用内置 CJK，槽 2–3 空着。
        let (data, label) = match slot {
            0 => builtin.0.clone(),
            1 => builtin.1.clone(),
            _ => None,
        }?;
        // 内置候选的文件名几乎就是字族名（`Menlo.ttc` → `Menlo`）。清单里有
        // 它，就按目标字重重新挑一面——否则「自动 + Bold」会变成一个什么都
        // 不做的设置（默认安装正是自动，那等于字重永远无效）。
        if slot == 0 {
            if let Some(found) = self.face_slot(&label, target) {
                return Some(found);
            }
        }
        Some(Slot {
            family: label,
            weight: DEFAULT_WEIGHT,
            regular_weight: DEFAULT_WEIGHT,
            style: "Regular".to_string(),
            data,
        })
    }

    /// 清单里按字族名取一槽：`data` 装**常规面**（UI 也用它），`weight` 记
    /// 目标字重的实际档位（网格用 `weighted_primary` 另装一份）。
    ///
    /// 返回 `None` = 这个字族不在清单里，或者文件读不出来——调用方继续回落。
    fn face_slot(&self, family: &str, target: u16) -> Option<Slot> {
        let catalog = self.catalog.as_ref()?;
        let named = catalog.find(family)?;
        let regular = catalog.nearest_face(family, DEFAULT_WEIGHT)?;
        let effective = catalog.nearest_face(family, target)?;
        let data = read_face(&regular.path.to_string_lossy(), regular.index)?;
        Some(Slot {
            family: named.name.clone(),
            weight: effective.weight,
            regular_weight: regular.weight,
            style: effective.style.clone(),
            data,
        })
    }

    /// 一槽的「请求 → 生效」对照（设置页用）。
    pub fn slot_info(&self, slot: usize) -> SlotInfo {
        let requested = self.want.get(slot).cloned().unwrap_or_default();
        let (family, weight) = self
            .slots
            .get(slot)
            .and_then(|s| s.as_ref())
            .map(|s| (s.family.clone(), s.weight))
            .unwrap_or_default();
        let fell_back =
            !requested.trim().is_empty() && !family.eq_ignore_ascii_case(requested.trim());
        SlotInfo {
            requested,
            family,
            weight,
            fell_back,
        }
    }

    /// 一槽装上的子族名（悬停里显示「Regular / Bold」）。
    pub fn slot_style(&self, slot: usize) -> Option<&str> {
        self.slots
            .get(slot)
            .and_then(|s| s.as_ref())
            .map(|s| s.style.as_str())
    }

    /// 本机字面清单。第一次调用会扫一次（约 1s），之后复用。
    ///
    /// 设置页进入 `Text ▸ Font` 时调；不在启动路径上。
    pub fn ensure_catalog(&mut self) {
        let had = self.scan_catalog();
        if had {
            // 清单刚到：之前「没装」的判断可能全错（名字其实装着，只是清单还空着），
            // 四槽按新清单重解析一次。
            self.reload();
        }
    }

    /// 扫清单（幂等）。返回「这次真的扫了」——调用方据此决定要不要重装字面。
    fn scan_catalog(&mut self) -> bool {
        if self.catalog.is_some() {
            return false;
        }
        let catalog = Catalog::scan();
        self.choices = catalog
            .families()
            .iter()
            .map(|f| FamilyChoice {
                name: f.name.clone(),
                monospace: f.monospace,
            })
            .collect();
        self.catalog = Some(catalog);
        true
    }

    /// 下拉要列的（字族, 是否等宽）。清单还没扫过时是空的——设置页会先
    /// `ensure_catalog`，所以真实调用点看到的永远不是空表。
    pub fn families(&self) -> &[FamilyChoice] {
        &self.choices
    }

    /// 换一槽的字族。`""` = 回到自动。返回是否真的变了。
    pub fn set_family(&mut self, slot: usize, family: &str) -> bool {
        if slot >= FAMILY_SLOTS || self.want[slot] == family {
            return false;
        }
        self.want[slot] = family.to_string();
        self.reload();
        true
    }

    /// 换字重（只作用于槽 0）。返回是否真的变了。
    pub fn set_weight(&mut self, weight: u16) -> bool {
        let weight = normalize_weight(weight);
        if self.want_weight == weight {
            return false;
        }
        self.want_weight = weight;
        self.reload();
        true
    }

    pub fn weight(&self) -> u16 {
        self.want_weight
    }

    /// 主字族能选的（字重, 文案）。没装 / 清单里没有该字族时给一个 Regular
    /// 兜底，让下拉至少能显示当前值。
    pub fn weight_choices(&self) -> Vec<(u16, String)> {
        let primary = self.slot_info(0).family;
        let mut list = self
            .catalog
            .as_ref()
            .map(|c| c.weights(&primary))
            .unwrap_or_default();
        if list.is_empty() {
            list.push((
                DEFAULT_WEIGHT,
                catalog::canonical_weight(DEFAULT_WEIGHT).to_string(),
            ));
        }
        list
    }

    /// Hand the faces to egui. Cheap after the first call: `set_fonts` is a
    /// no-op when the definitions have not changed.
    ///
    /// R14-b：字号变化也要重装——否则改了 `size` 却仍用旧字形度量，
    /// 网格会按旧 cell 尺寸排版，缩放「看不出来」。
    ///
    /// # 返回值：这一帧有没有换定义
    ///
    /// `set_fonts` 只是把新定义挂在 `memory.new_font_definitions` 上，**下一帧
    /// 开头**才生效（egui-0.29 `context.rs:1730`）。所以返回 `true` 时，同帧的
    /// `measure_once` 量到的还是旧字形——调用方照 `if !install(..) { measure_once(..) }`
    /// 写（`install` 返回 `true` 的那一帧不量 cell 盒）。
    pub fn install(&mut self, ctx: &egui::Context) -> bool {
        let ppp = ctx.pixels_per_point();
        let size_changed = (self.installed_size - self.size).abs() > 0.001;
        if self.installed && (self.installed_ppp - ppp).abs() < 0.001 && !size_changed {
            return false;
        }
        let mut defs = egui::FontDefinitions::default();
        // egui 自带的那一套（Hack / Ubuntui-Light / emoji）作为最后兜底，
        // 先取出来再改 `families`，免得把自己刚插进去的读回来。
        let default_mono = defs
            .families
            .get(&egui::FontFamily::Monospace)
            .cloned()
            .unwrap_or_default();
        let default_prop = defs
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();

        if let Some(slot) = &self.slots[0] {
            defs.font_data
                .insert("windterm-w0".to_owned(), slot.data.clone());
        }
        for (i, slot) in self.slots.iter().enumerate().skip(1) {
            if let Some(slot) = slot {
                defs.font_data
                    .insert(format!("windterm-w{i}"), slot.data.clone());
            }
        }

        // 槽 0（UI 常规）→ 槽 1–3（回退）→ egui 自带兜底。
        let mut mono_stack: Vec<String> = Vec::new();
        if self.slots[0].is_some() {
            mono_stack.push("windterm-w0".to_owned());
        }
        for i in 1..FAMILY_SLOTS {
            if self.slots[i].is_some() {
                mono_stack.push(format!("windterm-w{i}"));
            }
        }
        mono_stack.extend(default_mono.clone());
        defs.families
            .insert(egui::FontFamily::Monospace, mono_stack);

        // 网格：槽 0 的目标字重面打头，回退与 UI 共用。目标字重就是常规面那个
        // 重量时不另存一份文件（字体动辄几 MB），直接用 `windterm-w0`。
        let mut term_stack: Vec<String> = Vec::new();
        if self.slots[0].is_some() {
            match self.weighted_primary() {
                Some(weighted) => {
                    defs.font_data
                        .insert("windterm-w0-weight".to_owned(), weighted);
                    term_stack.push("windterm-w0-weight".to_owned());
                }
                None => term_stack.push("windterm-w0".to_owned()),
            }
        }
        for i in 1..FAMILY_SLOTS {
            if self.slots[i].is_some() {
                term_stack.push(format!("windterm-w{i}"));
            }
        }
        term_stack.extend(default_mono);
        defs.families
            .insert(egui::FontFamily::Name(TERMINAL_FAMILY.into()), term_stack);

        let mut proportional: Vec<String> = Vec::new();
        if self.slots[1].is_some() {
            proportional.push("windterm-w1".to_owned());
        }
        proportional.extend(default_prop);
        defs.families
            .insert(egui::FontFamily::Proportional, proportional);

        ctx.set_fonts(defs);
        self.installed = true;
        self.installed_ppp = ppp;
        self.installed_size = self.size;
        // The new definitions activate on the next pass: measure again then.
        self.measured = false;
        true
    }

    /// 槽 0 的目标字重面。与常规面同一字重时返回 `None`（复用 `windterm-w0`）。
    ///
    /// 用**槽里生效的字族名**而不是 `want[0]`：默认安装是「自动」，`want[0]`
    /// 是空串，而自动挑到的 Menlo 一样有 Bold 档——用 `want[0]` 判断会让
    /// 「自动 + Bold」永远拿不到粗体。
    fn weighted_primary(&self) -> Option<egui::FontData> {
        let catalog = self.catalog.as_ref()?;
        let slot = self.slots[0].as_ref()?;
        let face = catalog.nearest_face(&slot.family, self.want_weight)?;
        if face.weight == slot.regular_weight {
            return None;
        }
        read_face(&face.path.to_string_lossy(), face.index)
    }

    /// Measure the cell box. Safe to call from the frame after `install`.
    ///
    /// # `defs_changed`：这一帧刚把新定义交给 egui
    ///
    /// `set_fonts` 下一帧才生效，所以这一帧量到的是**旧字体**的宽度。可这一帧
    /// 又不能不量：首帧的 cell 盒决定面板与网格的初始排版，缺了它，第一帧就会
    /// 按 7.8×16 的占位值把 PTY 按错误行列 resize 一次（实测面板里的 shell 会
    /// 因此少 3 行输出）。
    ///
    /// 所以：照量，但**不认账**——把 `measured` 置回 `false`，下一帧用真正生效
    /// 的定义再量一次。字号变更时两次结果相同（字号在 `FontId` 里，跟定义无关）；
    /// 字族变更时第二遍才准。`ready` 也只在定义已生效的那次量之后才置位。
    pub fn measure_once(&mut self, ctx: &egui::Context, defs_changed: bool) {
        if self.measured || !self.installed {
            return;
        }
        let want_cjk = self.slots[1].is_some();
        let mono_id = self.font_id();
        let (cell_w, cell_h, cjk_w) = ctx.fonts(|fonts| {
            let w = fonts.glyph_width(&mono_id, 'M');
            let h = fonts.row_height(&mono_id);
            let cjk = if want_cjk {
                fonts.glyph_width(&mono_id, '中')
            } else {
                0.0
            };
            (w, h, cjk)
        });
        if cell_w > 1.0 && cell_h > 1.0 {
            self.cell_w = cell_w.max(1.0);
            self.cell_h = cell_h.max(1.0);
        }
        if cjk_w > 1.0 {
            self.cjk_cells = if cjk_w > self.cell_w * 1.5 { 2 } else { 1 };
        }
        if defs_changed {
            // 这份数是用上一份定义量出来的：留给这一帧排版，下一帧重量。
            self.measured = false;
        } else {
            // 量到了 = 当前定义确实在这个上下文里生效了，网格可以从下一帧起
            // 用终端字族。
            self.measured = true;
            self.ready = true;
        }
    }

    pub fn measured(&self) -> bool {
        self.measured
    }

    /// The face a grid cell is painted with.
    ///
    /// 只有名字真的在某个上下文里生效过（`ready`）才用它：
    /// `windterm-terminal` 只存在于 `install` 交出去的那份定义里，而 `epaint`
    /// 对未绑定的字族是 `panic!`（`text/fonts.rs:631`）。有两类调用点必须容忍
    /// 「没走过 `install`」：定义刚交出去、下一帧才生效的那一帧；以及直接
    /// 调 `draw_panel_body` 画面板的测试。退回 `Monospace` 两者都安全——它由
    /// `apply_ui_style` 保证存在，代价只是「头两帧用 UI 的字量一下」。
    pub fn font_id(&self) -> egui::FontId {
        let family = if self.ready {
            egui::FontFamily::Name(TERMINAL_FAMILY.into())
        } else {
            egui::FontFamily::Monospace
        };
        egui::FontId {
            size: self.size,
            family,
        }
    }

    /// R14-b：当前字号（缩放后的值）。
    pub fn size(&self) -> f32 {
        self.size
    }

    /// R14-b：设定字号并强制下一帧重装 + 重新度量。
    ///
    /// 不直接改 `installed`：重装由 `install` 的「size 变了」分支负责，
    /// 这里只更新目标值；`draw` 每帧调 `install`，会自动追上新尺寸。
    pub fn set_size(&mut self, size: f32) {
        let size = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        if (self.size - size).abs() < 0.001 {
            return;
        }
        self.size = size;
        // 让度量失效：`install` 会因 size 变化重装，重装后 `measure_once`
        // 用新尺寸量出新的 cell 盒，网格随之回流（见 `draw_terminal_area` 的 cols/rows）。
        self.measured = false;
    }

    /// R14-b：相对步进缩放（dir>0 放大、dir<0 缩小、dir==0 复位到出厂默认）。
    ///
    /// 步进固定 1pt：终端字号对可读性敏感，连续缩放容易一步跨过头。
    pub fn zoom(&mut self, dir: i8) {
        let target = if dir == 0 {
            TERMINAL_FONT_SIZE
        } else {
            self.size + dir as f32
        };
        self.set_size(target);
    }
}

/// 0 当「没设过」→ 出厂 400；其余夹到 OS/2 的合法档位区间。
fn normalize_weight(weight: u16) -> u16 {
    if weight == 0 {
        DEFAULT_WEIGHT
    } else {
        weight.clamp(100, 900)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_weight_means_regular_and_garbage_is_clamped() {
        assert_eq!(normalize_weight(0), 400);
        assert_eq!(normalize_weight(700), 700);
        assert_eq!(normalize_weight(50), 100, "50 不可能是合法档位");
        assert_eq!(normalize_weight(5000), 900);
    }

    #[test]
    fn the_default_install_does_not_scan_the_font_directories() {
        // 启动路径上不许扫盘：默认四槽全空，清单必须是 `None`（懒加载）。
        // 这条测试钉的是「性能」这个判据：改成启动就扫，这里会立刻红。
        let fonts = TerminalFonts::load();
        assert!(fonts.catalog.is_none(), "默认安装不该扫字体目录");
        assert!(!fonts.slot_info(0).family.is_empty(), "槽 0 仍要有自动字面");
    }

    #[test]
    fn a_requested_family_that_is_not_installed_falls_back_and_says_so() {
        // 用户把名字手改成一个没装的字族：不能空着不画字，要回落到内置候选，
        // 并且 `fell_back` 让设置页能把「没装」说出来。
        let mut fonts = TerminalFonts::load();
        fonts.want[0] = "No Such Font 12345".to_string();
        fonts.reload();
        let info = fonts.slot_info(0);
        assert!(info.fell_back, "没装的名字必须标出来");
        assert!(!info.family.is_empty(), "回落之后仍要有字可画");
    }

    #[test]
    fn weight_choices_always_offer_at_least_regular() {
        // 清单还没扫、或者主字族不认识时，下拉不能是空的——空下拉看起来
        // 像「这字体没有字重」，实际只是还没查。
        let fonts = TerminalFonts::load();
        let choices = fonts.weight_choices();
        assert!(!choices.is_empty());
        assert_eq!(choices[0].0, DEFAULT_WEIGHT);
        assert_eq!(choices[0].1, "Regular");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn picking_an_installed_family_and_weight_takes_effect() {
        // 端到端：扫清单 → 选字族 → 选字重，一路走到 `slot_info`（设置页
        // 显示的就是它）。这条测试挡的是「下拉能点、画面不变」那类接线错。
        let mut fonts = TerminalFonts::load();
        fonts.ensure_catalog();
        assert!(!fonts.families().is_empty(), "清单要先有内容");

        assert!(fonts.set_family(0, "Hiragino Sans GB"), "换成已装的字族");
        let info = fonts.slot_info(0);
        assert_eq!(info.family, "Hiragino Sans GB");
        assert!(!info.fell_back);
        assert_eq!(info.requested, "Hiragino Sans GB");

        assert!(fonts.set_family(0, ""), "清空 = 回到自动");
        let auto = fonts.slot_info(0);
        assert!(auto.requested.is_empty());
        assert!(!auto.family.is_empty(), "自动仍有内置候选");

        assert!(fonts.set_weight(700), "换成 Bold");
        assert_eq!(fonts.slot_info(0).weight, 700);
        // 同一字族内换字重不该把字族名冲掉。
        assert_eq!(fonts.slot_info(0).family, auto.family);
    }
}
