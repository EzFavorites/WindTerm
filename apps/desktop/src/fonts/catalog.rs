//! 本机已装字面的清单：设置页 `Font family 1–4` / `Font weight` 的取值来源。
//!
//! # 为什么需要这一层
//!
//! `mod.rs` 里的 `TerminalFonts` 原先只知道**三五个写死的路径**
//! （`Menlo.ttc` / `Hiragino Sans GB.ttc` / `Consolas` …）。那是「能显示字」
//! 的最低要求，不是「能选字」：设置页那四行字族框只能显示自动挑到的那一个，
//! 用户想换成 IBM Plex Mono 或 JetBrains Mono 没有入口。
//!
//! 这一层做两件事，都不碰 egui：
//!
//! 1. **枚举**：扫系统字体目录，把每个文件里的每一面解析成
//!    「字族名 + 字重 + 是否等宽 + 在哪（路径 / `.ttc` 第几面）」。
//! 2. **选面**：给定字族与目标字重，挑出最接近的那一面（跳过斜体）。
//!
//! # 为什么要 `ttf-parser`
//!
//! 想知道「这个文件属于哪个字族、是不是粗体、是不是等宽」，只有两条路：
//! 读字体自己的 `name` / `OS/2` / `post` 表，或者从文件名猜。文件名猜不准
//! （`Menlo.ttc` 里四个面字重各不相同，`Supplemental` 里一堆 `* Bold Italic`），
//! 而表解析是有边界条件的二进制格式。`ttf-parser` 本来就在依赖图里
//! （`epaint → ab_glyph → owned_ttf_parser → ttf-parser`），提成直接依赖
//! 不多编一行代码。
//!
//! # 代价：扫一次约 1 秒
//!
//! 本机 745 个字体文件、约 1.2 GB，全部读一遍热缓存 0.8s、冷缓存 2s 左右。
//! 所以**不在启动路径上扫**：`TerminalFonts` 把清单存成 `Option`，只有用户
//! 真的拉开字族下拉（或者 prefs 里存着一个非默认字族、启动时要解析）才
//! `Catalog::scan()` 一次，之后本次运行一直复用。默认安装（四项全空）的
//! 启动路径一次 IO 都不多。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ttf_parser::name_id;

/// 目标字重缺省值（OS/2 `usWeightClass` 的 Regular）。
pub const DEFAULT_WEIGHT: u16 = 400;

/// 一个字体文件最多解析几面。`.ttc` 里常见 2–8 面，给 32 是防御：
/// 真有病态文件写了 300 面，别让一次枚举把内存吃光。
const MAX_FACES_PER_FILE: u32 = 32;

/// 单个文件读入上限。字体动辄几 MB，但没人会用一个 64 MB 的等宽字体；
/// 超了直接跳过——比把整份读进来再发现解析失败省事。
const MAX_FILE_BYTES: u64 = 64 << 20;

/// 比这还小的文件不是字体（`read_face` 里同一条下限）。
const MIN_FILE_BYTES: u64 = 1024;

/// 整个清单的上限（面数）。本机约 800 面，给 4096 是防止某个目录里堆了
/// 几千个文件时把一次枚举拖成好几秒。
const MAX_TOTAL_FACES: usize = 4096;

/// 一个字面（字体文件里的某一面）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Face {
    pub path: PathBuf,
    /// `.ttc` / `.otc` 里的面序号；单文件字体是 0。
    pub index: u32,
    /// OS/2 `usWeightClass`：100 Thin … 400 Regular … 700 Bold … 900 Black。
    pub weight: u16,
    /// 该面自报的子族名（`Regular` / `Bold` / `Bold Italic` …），下拉文案用它。
    pub style: String,
}

impl Face {
    /// 斜体面不参与选面：终端要正体，选到 Italic 只会让整屏字歪着。
    ///
    /// 判据看子族名而不是 `OS/2.fsSelection` 的 ITALIC 位：`ttf-parser` 的
    /// `is_italic()` 读的是 `head.macStyle`，而很多字体（尤其系统自带的
    /// `.ttc`）只填 `fsSelection`。名字里的 `Italic` / `Oblique` 两个词在
    /// 实际字体里覆盖得最全，误判也只是少一个候选。
    pub fn is_italic(&self) -> bool {
        let s = self.style.to_ascii_lowercase();
        s.contains("italic") || s.contains("oblique")
    }
}

/// 一个字族（同名的若干面收在一起）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Family {
    pub name: String,
    pub faces: Vec<Face>,
    /// 固定宽度。视觉基线那句 `Bold type indicates fixed-width fonts:` 说的就是这个。
    pub monospace: bool,
}

/// 本机字面清单。按字族名（忽略大小写）索引。
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    families: Vec<Family>,
}

impl Catalog {
    /// 扫系统字体目录。**只在需要时调用**（见模块头那段「代价」）。
    pub fn scan() -> Self {
        let mut files = Vec::new();
        for dir in font_dirs() {
            collect_font_files(&dir, 0, &mut files);
        }
        Self::from_files(files)
    }

    /// 从给定的文件清单建表。`scan()` 的纯函数骨架，测试直接喂路径。
    pub fn from_files(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut families: Vec<Family> = Vec::new();
        let mut faces = 0usize;
        for path in paths {
            if faces >= MAX_TOTAL_FACES {
                break;
            }
            let Some(bytes) = read_font_file(&path) else {
                continue;
            };
            let count = ttf_parser::fonts_in_collection(&bytes)
                .unwrap_or(1)
                .min(MAX_FACES_PER_FILE);
            for face_index in 0..count {
                let Ok(face) = ttf_parser::Face::parse(&bytes, face_index) else {
                    continue;
                };
                let Some(name) = family_name(&face) else {
                    continue;
                };
                let style = style_name(&face).unwrap_or_else(|| "Regular".to_string());
                let weight = weight_value(face.weight());
                let mono = face.is_monospaced() || looks_monospaced(&name);
                let key = name.to_lowercase();
                let slot = *index.entry(key).or_insert_with(|| {
                    families.push(Family {
                        name: name.clone(),
                        faces: Vec::new(),
                        monospace: mono,
                    });
                    families.len() - 1
                });
                let family = &mut families[slot];
                family.monospace |= mono;
                // 同名同重同斜的正体只留第一份：`Supplemental` 与用户目录里
                // 常有一模一样的字体，列两遍会让下拉看起来坏了。
                if !family
                    .faces
                    .iter()
                    .any(|f| f.weight == weight && f.style.eq_ignore_ascii_case(&style))
                {
                    family.faces.push(Face {
                        path: path.clone(),
                        index: face_index,
                        weight,
                        style,
                    });
                    faces += 1;
                }
                if faces >= MAX_TOTAL_FACES {
                    break;
                }
            }
        }
        families.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        });
        Self { families }
    }

    pub fn families(&self) -> &[Family] {
        &self.families
    }

    pub fn is_empty(&self) -> bool {
        self.families.is_empty()
    }

    /// 按名字找字族（忽略大小写）。设置页显示「找不到」时要的是 `None`，
    /// 不是悄悄换成别的字族。
    pub fn find(&self, name: &str) -> Option<&Family> {
        let key = name.trim().to_lowercase();
        if key.is_empty() {
            return None;
        }
        self.families
            .iter()
            .find(|f| f.name.to_lowercase() == key)
    }

    /// 在字族里挑最接近 `weight` 的**正体**面。
    ///
    /// 距离相同取更轻的那一面：用户把 400 的字体设成 700，而家族只有
    /// 400/600 时，600 是「更接近 Bold」的那一个；但反过来（只有 400/800、
    /// 目标是 600）同理取 800？——不，两个都差 200，此时取**更轻**的 400：
    /// 屏幕字重宁可偏细也不能偏粗，粗一档整屏就糊。
    pub fn nearest_face(&self, family: &str, weight: u16) -> Option<&Face> {
        let family = self.find(family)?;
        family
            .faces
            .iter()
            .filter(|f| !f.is_italic())
            .min_by_key(|f| (f.weight.abs_diff(weight), f.weight))
    }

    /// 字族里可选的字重（去重、升序、只算正体），带下拉文案。
    ///
    /// 文案优先用字体自报的子族名（`SemiBold` / `Medium`），没有就按 OS/2
    /// 的档位名兜底——总有字体把子族名写成 `Regular` 了事。
    pub fn weights(&self, family: &str) -> Vec<(u16, String)> {
        let Some(family) = self.find(family) else {
            return Vec::new();
        };
        let mut out: Vec<(u16, String)> = Vec::new();
        for face in &family.faces {
            if face.is_italic() {
                continue;
            }
            if !out.iter().any(|(w, _)| *w == face.weight) {
                out.push((face.weight, face.style.clone()));
            }
        }
        out.sort_by_key(|(w, _)| *w);
        for (w, label) in &mut out {
            if label.is_empty() || label.eq_ignore_ascii_case("Regular") && *w != DEFAULT_WEIGHT {
                *label = canonical_weight(*w).to_string();
            }
        }
        out
    }
}

/// OS/2 字重档位的标准名。`ttf-parser` 的 `Weight` 枚举映射成数值之后，
/// 下拉里还是要有话说（`400` 单看不知道是常规体）。
pub fn canonical_weight(w: u16) -> &'static str {
    match w {
        100 => "Thin",
        200 => "ExtraLight",
        300 => "Light",
        400 => "Regular",
        500 => "Medium",
        600 => "SemiBold",
        700 => "Bold",
        800 => "ExtraBold",
        900 => "Black",
        _ => "Custom",
    }
}

/// `ttf-parser` 的 `Weight` 枚举 → OS/2 的数值档位。
///
/// `Other(v)` 是字体里写了非标准值（比如 350）的情形，原样带出来——
/// 下拉里显示 `Custom` 比硬塞进 400 诚实。
fn weight_value(w: ttf_parser::Weight) -> u16 {
    use ttf_parser::Weight::*;
    match w {
        Thin => 100,
        ExtraLight => 200,
        Light => 300,
        Normal => 400,
        Medium => 500,
        SemiBold => 600,
        Bold => 700,
        ExtraBold => 800,
        Black => 900,
        Other(v) => v,
    }
}

/// 从 `name` 表取字族名：先找排版字族名（id 16），再退回字体名（id 1）。
///
/// id 16 是「Roboto Mono」这种真正的家族名，id 1 在有些字体里带字重后缀
/// （`Menlo Bold`），拿它当家族名会把一个字族拆成四个，下拉里出现四行
/// 同名不同重的东西。
fn family_name(face: &ttf_parser::Face) -> Option<String> {
    pick_name(face, &[name_id::TYPOGRAPHIC_FAMILY, name_id::FAMILY])
}

/// 子族名：id 17（排版子族名）优先，退回 id 2。
fn style_name(face: &ttf_parser::Face) -> Option<String> {
    pick_name(face, &[name_id::TYPOGRAPHIC_SUBFAMILY, name_id::SUBFAMILY])
}

/// 按 `ids` 的顺序挑第一个有值的名字；同一个 id 有多条记录时优先
/// Unicode 编码 + 英文。
fn pick_name(face: &ttf_parser::Face, ids: &[u16]) -> Option<String> {
    let names = face.names();
    for id in ids {
        let mut best: Option<(u8, String)> = None;
        for i in 0..names.len() {
            let Some(record) = names.get(i) else {
                continue;
            };
            if record.name_id != *id {
                continue;
            }
            let Some(text) = name_text(&record) else {
                continue;
            };
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            // 0 = Unicode 英文，1 = Unicode 其它语言，2 = 老 Mac 编码。
            let rank = if record.is_unicode() && record.language_id == 0x0409 {
                0
            } else if record.is_unicode() {
                1
            } else {
                2
            };
            if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                best = Some((rank, text));
            }
        }
        if let Some((_, text)) = best {
            return Some(text);
        }
    }
    None
}

/// 取一条 `name` 记录的文本。
///
/// `Name::to_string()` 只认 Unicode（UTF-16BE）。**系统自带字体经常只写
/// Macintosh 平台的名字**——`Menlo.ttc` 的 id 1/2 就是 Mac Roman 单字节，
/// 严格按 Unicode 读会得到 `None`，于是「Menlo 没装」这种荒唐结论。
/// Mac Roman 的前 128 个码位与 ASCII 相同，而字族名几乎全是 ASCII，
/// 所以这里按 Latin-1 直读做兜底；非 ASCII 的少数情况仍会走 Windows 那条
/// Unicode 记录（有就用英文名）。
fn name_text(record: &ttf_parser::name::Name) -> Option<String> {
    if let Some(text) = record.to_string() {
        return Some(text);
    }
    if record.platform_id == ttf_parser::PlatformId::Macintosh {
        let text: String = record.name.iter().map(|b| *b as char).collect();
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

/// `post` 表没标等宽时的名字兜底。
///
/// 只认几个几乎不会误伤的词：等宽字体的家族名里几乎一定带 `Mono` /
/// `Courier` / `Console`，而比例字体不会。
fn looks_monospaced(name: &str) -> bool {
    let n = name.to_lowercase();
    ["mono", "courier", "consol", "menlo", "monaco", "fixed"]
        .iter()
        .any(|k| n.contains(k))
}

/// 读字体文件，超过上下限直接跳过（不读进内存）。
fn read_font_file(path: &Path) -> Option<Vec<u8>> {
    let len = std::fs::metadata(path).ok()?.len();
    if !(MIN_FILE_BYTES..=MAX_FILE_BYTES).contains(&len) {
        return None;
    }
    std::fs::read(path).ok()
}

/// 各平台的系统字体目录，**用户目录在前**：同名字族先被用户那份占住，
/// 于是「我装的 JetBrains Mono」不会被系统里某个同名旧版本顶掉。
#[cfg(target_os = "macos")]
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("Library/Fonts"));
    }
    dirs.push(PathBuf::from("/Library/Fonts"));
    dirs.push(PathBuf::from("/System/Library/Fonts/Supplemental"));
    dirs.push(PathBuf::from("/System/Library/Fonts"));
    dirs
}

#[cfg(windows)]
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
    }
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    dirs.push(PathBuf::from(root).join("Fonts"));
    dirs
}

#[cfg(all(unix, not(target_os = "macos")))]
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".fonts"));
        dirs.push(home.join(".local/share/fonts"));
    }
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs.push(PathBuf::from("/usr/local/share/fonts"));
    dirs
}

/// 递归收集字体文件。深度上限 4：字体目录最深也就 `fonts/truetype/<厂家>/`，
/// 再深就不是字体树了，跟着符号链接绕圈的风险却真实存在。
fn collect_font_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            collect_font_files(&path, depth + 1, out);
        } else if is_font_file(&path) {
            out.push(path);
        }
    }
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "ttf" | "ttc" | "otf" | "otc"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个只有两个面的最小字族，用来测「按字重挑面」的规则——
    /// 不依赖本机装了什么。
    fn family(faces: &[(u16, &str)]) -> Family {
        Family {
            name: "Test Mono".to_string(),
            monospace: true,
            faces: faces
                .iter()
                .enumerate()
                .map(|(i, (w, style))| Face {
                    path: PathBuf::from(format!("/f{i}")),
                    index: 0,
                    weight: *w,
                    style: (*style).to_string(),
                })
                .collect(),
        }
    }

    fn catalog_with(faces: &[(u16, &str)]) -> Catalog {
        Catalog {
            families: vec![family(faces)],
        }
    }

    #[test]
    fn the_nearest_weight_wins_over_the_first_face() {
        let c = catalog_with(&[(400, "Regular"), (700, "Bold")]);
        assert_eq!(c.nearest_face("test mono", 700).unwrap().weight, 700);
        assert_eq!(c.nearest_face("TEST MONO", 400).unwrap().weight, 400);
        // 600 距 700 更近。
        assert_eq!(c.nearest_face("Test Mono", 600).unwrap().weight, 700);
    }

    #[test]
    fn ties_go_to_the_lighter_face() {
        // 只有 400/800、目标 600：两个都差 200，取轻的那个——屏幕字宁可细
        // 也不能糊。
        let c = catalog_with(&[(400, "Regular"), (800, "ExtraBold")]);
        assert_eq!(c.nearest_face("Test Mono", 600).unwrap().weight, 400);
        assert_eq!(c.nearest_face("Test Mono", 500).unwrap().weight, 400);
    }

    #[test]
    fn italic_faces_are_never_chosen() {
        let c = catalog_with(&[(400, "Italic"), (400, "Regular")]);
        assert_eq!(c.nearest_face("Test Mono", 400).unwrap().style, "Regular");
        // 只有斜体时宁可不选，也不要拿歪字去铺终端。
        let only_italic = catalog_with(&[(700, "Bold Italic")]);
        assert!(only_italic.nearest_face("Test Mono", 700).is_none());
    }

    #[test]
    fn a_family_that_is_not_installed_is_reported_as_missing() {
        // 「找不到」必须是 `None`：调用方要靠它回落到内置候选，并在设置页
        // 标出这个字族没装（而不是悄悄换成别的）。
        let c = catalog_with(&[(400, "Regular")]);
        assert!(c.find("No Such Font").is_none());
        assert!(c.nearest_face("No Such Font", 400).is_none());
        assert!(c.weights("No Such Font").is_empty());
        assert!(c.find("   ").is_none(), "空白名不算字族");
    }

    #[test]
    fn weights_are_deduped_sorted_and_skip_italics() {
        let c = catalog_with(&[
            (700, "Bold"),
            (400, "Regular"),
            (400, "Italic"),
            (500, "Medium"),
        ]);
        let w = c.weights("Test Mono");
        assert_eq!(w.iter().map(|(v, _)| *v).collect::<Vec<_>>(), vec![400, 500, 700]);
        assert_eq!(w[2].1, "Bold", "文案用字体自报的子族名");
    }

    #[test]
    fn anonymous_weights_get_a_readable_name() {
        // 有些字体把 500 的子族名也写成 `Regular`；下拉里两行都叫 Regular
        // 就是坏掉的 UI，退回档位名。
        let c = catalog_with(&[(500, "Regular")]);
        assert_eq!(c.weights("Test Mono")[0].1, "Medium");
        assert_eq!(canonical_weight(350), "Custom");
    }

    #[test]
    fn monospace_names_are_recognised_when_the_post_table_is_silent() {
        // `post.is_monospaced` 缺失时靠名字兜底：视觉基线那句「粗体 = 等宽」
        // 全靠这个判据，猜错了整列加粗都不对。
        assert!(looks_monospaced("JetBrains Mono"));
        assert!(looks_monospaced("Consolas"));
        assert!(!looks_monospaced("Helvetica Neue"));
        assert!(!looks_monospaced("Songti SC"));
    }

    #[test]
    fn scanning_a_directory_that_does_not_exist_yields_an_empty_catalog() {
        // 枚举失败不是错误：清单空着，设置页就把下拉画成空的并说明原因，
        // 终端仍走内置候选——这条路径必须不 panic。
        let mut files = Vec::new();
        collect_font_files(Path::new("/no/such/font/dir"), 0, &mut files);
        assert!(files.is_empty());
        assert!(Catalog::from_files(files).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_real_system_catalog_finds_the_shipped_defaults() {
        // 本机真扫一次（约 1s，只在测试里做）：默认安装要用的两个字族
        // 必须能被找到，否则「自动挑到 Menlo」这句话在新机器上就不成立。
        let c = Catalog::scan();
        assert!(!c.is_empty(), "系统字体目录里至少要有可解析的字面");
        for name in ["Menlo", "Hiragino Sans GB"] {
            let f = c
                .find(name)
                .unwrap_or_else(|| panic!("默认字族 {name} 没被枚举到"));
            assert!(f.monospace || name.contains("Hiragino"), "{name} 的等宽标记");
            assert!(!f.faces.is_empty());
        }
        // 系统里一定有斜体面，但选面规则永远不选它。
        let menlo = &c.find("Menlo").unwrap().faces;
        assert!(menlo.iter().any(|f| f.is_italic()), "Menlo 里应当有斜体面");
        assert!(!c.nearest_face("Menlo", 400).unwrap().is_italic());
    }
}
