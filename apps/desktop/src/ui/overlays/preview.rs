//! 预览窗（`docs/design/05-files-and-transfer.md` 的「从 Filer 看一眼文件」）。
//!
//! 三件事，各占一段：
//!
//! 1. **纯投影**（[`content_of`] / [`needs_truncation_note`] / [`human_bytes`] /
//!    [`basename_of`]）：不碰 egui、不碰「另一个文件」，输入路径或内容，输出
//!    可以直接画的东西。这一半有 14 条单测（`preview_tests.rs`）。
//! 2. **状态**（[`PreviewState`]）：`content` 是**已经读好**的结果，绘制期不再
//!    碰文件系统。图片纹理同理——只在开窗那一刻解码一次（见下）。
//! 3. **绘制**（[`TerminalWindow::draw_preview`]）：固定矩形 + 固定 id，与其余
//!    对话框同一套首帧即最终布局。
//!
//! # 为什么 `content` 是「读好的结果」而不是「一个路径」
//!
//! 绘制每帧跑几十次。让绘制去 `read_head` 意味着每帧一次 `openat` + 64 KiB 读
//! ——滚动一下就把磁盘读穿。更糟的是图片：每帧 `load_texture` 会**真的泄漏显存**
//! （egui 的纹理表只增不减）。所以形态定死：打开时读一次、解一次，之后只读。
//!
//! # 读文件为什么在 `ui/` 而不是更下层
//!
//! 读字节那一层确实在 `term_io::preview`（IO 归 IO）。这里留下的是「把三个
//! 纯函数的结果拼成一个枚举」——它没有句柄、没有线程，与 `filer.rs` 调
//! `local_dir::list` 是同一条边界。
//!
//! # 已知取舍
//!
//! - **不做语法高亮**：预览窗要回答「这是不是我要找的那个文件」，不是当编辑器用。
//! - **不跟随文件变化**：打开那一刻的快照就是全部。远端文件尤其如此——它的
//!   本地副本本来就是一次性的。
//! - **解码失败不重试、不降级**：保留 `Image`（宽高来自魔数嗅探）并画占位框，
//!   因为「这是张 640×480 的 PNG，但我们读不懂它的像素」本身也是有用的信息。

use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;
use std::path::{Path, PathBuf};
use term_app::i18n::Lang;
use term_io::preview::{self, Charset, ContentKind, ImageFormat};

const CONTENT_MIN: egui::Vec2 = egui::vec2(600.0, 496.0);
/// 正文区高度：留出标题行 / 截断提示 / 按钮行的空间后剩下的那一块。
const BODY_H: f32 = 396.0;

/// 整文件的解码上限。`term_io::preview` 只读 64 KiB 头，图片却要整份解码——
/// 这是本文件唯一会「把大文件整个读进内存」的地方，所以两条闸门都得有。
const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
/// 解出来的像素上限（16 Mpx ≈ 4096×4096）。压缩率高的 PNG 可以做到
/// 「16 MiB 文件 → 1.6 GB 的 RGBA」，只限字节数是不够的。
const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;

/// 预览窗里正在显示什么。
///
/// `content` 是**已经读好**的结果：绘制期不再碰文件系统（见模块注释）。
pub(in crate::ui) enum PreviewContent {
    Text {
        /// 已经解码成 `String`、已经按 200 行裁过的正文。
        body: String,
        /// 用户看到的不是全部（文件更长 / 行数被裁）。
        truncated: bool,
        charset: Charset,
    },
    Image {
        format: ImageFormat,
        /// 嗅探读不出尺寸时为 `0`——照 `term_io::preview` 的口径不猜。
        width: u32,
        height: u32,
    },
    /// 二进制：只报大小，让人自己决定用十六进制看还是交给外部程序。
    Binary { size: u64 },
    Empty,
    /// 读失败（不存在 / 是目录 / 没权限 / 其余 IO）。`message` 直接来自
    /// `HeadError::message()`，是终端层口径的英文诊断，与其余错误行一致。
    Error { message: String },
}

/// 绘制期收集的一次性提示。
///
/// 存 **key** 而不是译好的句子：用户在预览窗开着时切语言，提示要跟着变。
/// 平台给的原因（`io::Error` 的文本）没法进词表，单独一支带着。
pub(in crate::ui) enum Note {
    /// 直接翻译这条 i18n key。
    Key(&'static str),
    /// 「系统默认程序打开失败」+ 平台给的原因。
    OpenFailed(String),
}

/// 预览窗的全部状态。
///
/// 不进 `ViewState`：它带着一个 `TextureHandle`（显存句柄）与一段已解码的正文，
/// `ViewState` 只装领域语义与开关。**可见性**那一个 bool 仍在
/// `ViewState::show_preview`——两个字段恒定同步，写入点只有
/// [`TerminalWindow::open_preview_local`] 与 [`TerminalWindow::close_preview`]。
pub(in crate::ui) struct PreviewState {
    pub open: bool,
    /// 文件名（远端路径取最后一段）。窗口标题就是它。
    pub title: String,
    /// 展示用路径：本地是绝对路径，远端是 `host:/path`。
    ///
    /// 与 `local` 分开是刻意的：远端文件真实存在于 `local`（临时副本），但屏上
    /// 要显示的是**它从哪来**。合成一个字段就必然要么骗用户（显示临时路径），
    /// 要么骗系统调用（拿 `host:/path` 去 open）。
    pub path: String,
    pub content: PreviewContent,
    /// 只有解码成功才有。`draw_preview` **只读不建**（每帧建会泄漏显存）。
    pub texture: Option<egui::TextureHandle>,
    /// 系统默认程序要的真实路径。远端文件是下载到临时目录的那份副本。
    pub local: Option<PathBuf>,
    /// 一次性提示（解码失败 / 打开失败 / 太大）。`None` = 不画。
    pub note: Option<Note>,
}

impl PreviewState {
    /// 出厂态：关着、没有任何内容。
    fn closed() -> Self {
        PreviewState {
            open: false,
            title: String::new(),
            path: String::new(),
            content: PreviewContent::Empty,
            texture: None,
            local: None,
            note: None,
        }
    }
}

impl Default for PreviewState {
    fn default() -> Self {
        PreviewState::closed()
    }
}

/// 把 `read_head` 的结果翻成 [`PreviewContent`]。不碰 egui、不碰文件系统
/// **之外**的东西（`read_head` 自己那一次读是它的本分）。
///
/// 调用口径只有一条：`read_head` → `sniff`（判类型）→ 文本走 `head_text`
/// （**它内部已经嗅过一遍**，不要再嗅第二次去取字符集——上面那次 `sniff` 的
/// 结果就是字符集）。
pub(in crate::ui) fn content_of(path: &Path) -> PreviewContent {
    let head = match preview::read_head(path, 0) {
        Ok(head) => head,
        Err(err) => {
            return PreviewContent::Error {
                message: err.message(),
            }
        }
    };
    match preview::sniff(&head.bytes) {
        ContentKind::Empty => PreviewContent::Empty,
        // 图片只报嗅探出来的格式与宽高；像素在 `decode_texture` 里单独解，
        // 因为那需要整份文件，而这里只读到了头。
        ContentKind::Image {
            format,
            width,
            height,
        } => PreviewContent::Image {
            format,
            width,
            height,
        },
        ContentKind::Binary => PreviewContent::Binary {
            size: file_size(path, &head),
        },
        ContentKind::Text { charset } => match preview::head_text(&head) {
            Some((body, truncated)) => PreviewContent::Text {
                body,
                truncated,
                charset,
            },
            // `head_text` 只在「不是文本」时给 `None`，而上面已经判成文本了。
            // 走到这里说明两次嗅探的结果不一致（理论上不是不可能：`head_text`
            // 会重新嗅一遍 `bytes`）。按二进制处理比 panic 好——用户仍然能看到
            // 大小并交给外部程序。
            None => PreviewContent::Binary {
                size: file_size(path, &head),
            },
        },
    }
}

/// 文件总字节数。**不用 `head.bytes.len()`**：被截断时那只是 64 KiB。
///
/// `metadata` 失败就退回「读到了多少」，因为 `content_of` 唯一会走到这里的
/// 路径是「刚读成功」，此时的长度至少是个下界，而不是 0。
fn file_size(path: &Path, head: &preview::Head) -> u64 {
    if !head.truncated {
        return head.bytes.len() as u64;
    }
    std::fs::metadata(path)
        .map(|meta| meta.len())
        .unwrap_or(head.bytes.len() as u64)
}

/// 截断提示文案的判定：只有 `truncated` 为真时才要提示。
///
/// 图片 / 二进制 / 空文件 / 读失败各有自己的说法，不该再叠一句「只显示了开头」。
pub(in crate::ui) fn needs_truncation_note(content: &PreviewContent) -> bool {
    matches!(
        content,
        PreviewContent::Text {
            truncated: true,
            ..
        }
    )
}

/// 字节数的人类可读形态（`1.2 MiB`）。
///
/// 二进制与空文件都用它；图片尺寸不走这里（那是像素不是字节）。
/// 口径：`< 1024` 原样报字节（`1023 B` 比 `1.0 KiB` 更不误导），其余保留
/// 一位小数。单位是**数据**不是界面文案，不进词表（与 `term_sftp::format_size`
/// 同一处理）。
pub(in crate::ui) fn human_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = n as f64;
    if n < 1024 {
        format!("{n} B")
    } else if bytes < MIB {
        format!("{:.1} KiB", bytes / KIB)
    } else if bytes < GIB {
        format!("{:.1} MiB", bytes / MIB)
    } else {
        format!("{:.1} GiB", bytes / GIB)
    }
}

/// 展示用文件名：远端 `host:/a/b/c.txt` 与本地 `/a/b/c.txt` 都取最后一段。
///
/// 尾斜杠（目录形态）不算名字——先削掉它再取最后一段，否则 `/tmp/dir/` 会
/// 得出空串。整条路径都没有名字（`/`）时退回原路径，总比标题空着强。
pub(in crate::ui) fn basename_of(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let leaf = trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .trim();
    if leaf.is_empty() {
        path.to_string()
    } else {
        leaf.to_string()
    }
}

/// 字符集的展示名。这是**数据**（文件的编码），不是界面文案，不进词表。
fn charset_label(charset: Charset) -> &'static str {
    match charset {
        Charset::Utf8 => "UTF-8",
        Charset::Utf8Bom => "UTF-8 BOM",
        Charset::Utf16Le => "UTF-16 LE",
        Charset::Utf16Be => "UTF-16 BE",
        Charset::Latin1 => "ISO-8859-1",
    }
}

/// 图片格式的展示名，同样不进词表。
fn format_label(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::Bmp => "BMP",
        ImageFormat::WebP => "WebP",
    }
}

/// 整份读 + 解码 + 上纹理。**只在开窗那一刻调一次**。
///
/// `Err` 里是 i18n key（调用方给 `PreviewState::note`）：解码失败与「太大不解码」
/// 是两种不同的坏，用户能做的事也不同（换文件 / 换个程序看）。
fn decode_texture(ctx: &egui::Context, path: &Path) -> Result<egui::TextureHandle, &'static str> {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > MAX_IMAGE_BYTES {
        return Err("Image too large to preview");
    }
    let bytes = std::fs::read(path).map_err(|_| "Cannot decode this image")?;
    // 先只读尺寸：压缩率高的 PNG 可以「16 MiB 文件 → 几百 MB 的 RGBA」，
    // 只限文件字节数拦不住，必须在分配像素缓冲之前再判一次。
    let dims = image::io::Reader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| "Cannot decode this image")?
        .into_dimensions()
        .map_err(|_| "Cannot decode this image")?;
    if u64::from(dims.0) * u64::from(dims.1) > MAX_IMAGE_PIXELS {
        return Err("Image too large to preview");
    }
    let rgba = image::load_from_memory(&bytes)
        .map_err(|_| "Cannot decode this image")?
        .to_rgba8();
    let (width, height) = (rgba.width() as usize, rgba.height() as usize);
    let color = egui::ColorImage::from_rgba_unmultiplied([width, height], rgba.as_raw());
    Ok(ctx.load_texture(
        format!("preview-{}", path.display()),
        color,
        egui::TextureOptions::LINEAR,
    ))
}

/// 绘制期间收集的动作。闭包外统一施加（与 `overlays/onboarding.rs` 同一套）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Close,
    OpenExternally,
}

impl TerminalWindow {
    /// 打开本地文件的预览。读 + 解码都在这一帧完成一次。
    pub(in crate::ui) fn open_preview_local(&mut self, path: &Path) {
        self.show_preview_at(path, None);
    }

    /// 打开一个**远端**文件：`local` 是刚下载到临时目录的副本，`display` 是
    /// 屏上要显示的 `host:/path`（见 [`PreviewState::path`] 的注释）。
    pub(in crate::ui) fn open_preview_remote(&mut self, display: &str, local: &Path) {
        self.show_preview_at(local, Some(display.to_string()));
    }

    /// 关窗。**既关状态又关开关**——两个字段描述同一件事，没有第三个写入者。
    pub(in crate::ui) fn close_preview(&mut self) {
        self.preview = PreviewState::closed();
        self.view.show_preview = false;
    }

    fn show_preview_at(&mut self, local: &Path, display: Option<String>) {
        let content = content_of(local);
        // 只有图片才解码。文本 / 二进制 / 空 / 报错都不该为纹理付代价。
        let (texture, note) = match &content {
            PreviewContent::Image { .. } => match decode_texture(&self.ctx, local) {
                Ok(texture) => (Some(texture), None),
                // 失败时保留 `Image`（宽高来自魔数嗅探）并画占位框：用户至少
                // 知道「这是张 640×480 的 PNG，只是我们解不开」。
                Err(key) => (None, Some(Note::Key(key))),
            },
            _ => (None, None),
        };
        let shown = display.unwrap_or_else(|| local.to_string_lossy().into_owned());
        self.preview = PreviewState {
            open: true,
            title: basename_of(&shown),
            path: shown,
            content,
            texture,
            local: Some(local.to_path_buf()),
            note,
        };
        self.view.show_preview = true;
        self.ctx.request_repaint();
    }

    /// 画预览窗。默认关闭时一行都不画（与其余对话框一致）。
    pub(in crate::ui) fn draw_preview(&mut self, ctx: &egui::Context) {
        if !self.view.show_preview || !self.preview.open {
            return;
        }
        let lang = self.view.lang;
        let title = self.preview.title.clone();
        let mut action: Option<Action> = None;
        // 状态只读、动作收集：窗口闭包持有 `self.preview` 的不可变借用，
        // 施加动作要等它结束（受控契约）。
        let open = DialogWindow::new(DialogId::Preview, &title).show(ctx, |ui| {
            draw_body(ui, lang, &self.preview, &mut action);
        });
        match action {
            Some(Action::Close) => self.close_preview(),
            Some(Action::OpenExternally) => self.open_externally(),
            None => {}
        }
        // 右上角 × 与 `Close` 走同一个结局。
        if !open {
            self.close_preview();
        }
    }

    /// 交给系统默认程序。失败的原因画回预览窗里（不 panic、不静默）。
    fn open_externally(&mut self) {
        // 按钮在 `local` 为空时是灰的，所以这里只是兜底：什么也不做，既不假装
        // 打开成功，也不为一条到不了的分支编一句用户永远看不到的文案。
        let Some(path) = self.preview.local.clone() else {
            return;
        };
        self.preview.note = match crate::open_with::open_path(&path) {
            Ok(()) => None,
            // 平台给的原因不翻译（它是 `io::Error` 的文本），前缀在绘制侧加：
            // 「哪个动作失败了」这句话只有一处拼，不会两边各拼一遍。
            Err(err) => Some(Note::OpenFailed(err.message())),
        };
    }
}

/// 正文 + 页脚。所有可见文案都经 `t(lang, …)`（i18n 门禁：新句子必须进词表）。
fn draw_body(
    ui: &mut egui::Ui,
    lang: Lang,
    state: &PreviewState,
    action: &mut Option<Action>,
) {
    let s = &term_core::theme::CHROME_SCALE;
    let text_col = crate::theme::chrome(s.text);
    let dim = text_col.gamma_multiply(0.62);

    // 副标题：它从哪来（本地绝对路径 / `host:/path`）。右对齐那一格是「这是什么」
    // 的数据：文本报字符集，图片报格式与像素。二进制 / 空文件 / 读失败的大小
    // 与原因写在正文的占位区里，不在这儿重复一遍。
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&state.path).small().color(dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(fact) = fact_line(state) {
                ui.label(egui::RichText::new(fact).small().color(dim));
            }
        });
    });
    ui.add_space(4.0);

    match &state.content {
        PreviewContent::Text { body, .. } => {
            egui::ScrollArea::both()
                .max_height(BODY_H)
                .auto_shrink([false, false])
                .id_salt("preview_text")
                .show(ui, |ui| {
                    // `TextWrapMode::Extend`：不折行。折行会让一屏日志的行号与
                    // 终端里对不上，而预览窗的用处正是「照原样看一眼」。
                    ui.add(
                        egui::Label::new(egui::RichText::new(body).monospace())
                            .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
        }
        PreviewContent::Image {
            format,
            width,
            height,
        } => draw_image(ui, state, *format, *width, *height, dim),
        PreviewContent::Binary { size } => {
            placeholder(ui, BODY_H, t(lang, "Binary file"), &human_bytes(*size), dim);
        }
        PreviewContent::Empty => {
            placeholder(ui, BODY_H, t(lang, "Empty file"), &human_bytes(0), dim);
        }
        PreviewContent::Error { message } => {
            placeholder(
                ui,
                BODY_H,
                t(lang, "Cannot preview this file"),
                message,
                dim,
            );
        }
    }

    // 截断提示：只有真的截了才说（`needs_truncation_note` 是唯一判据）。
    if needs_truncation_note(&state.content) {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(t(lang, "Only the beginning is shown"))
                .small()
                .color(dim),
        );
    }
    if let Some(note) = &state.note {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(note_text(lang, note))
                .small()
                .color(dim),
        );
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        // 本地文件与已下载的远端文件都能开：`local` 是真实路径，远端那份在
        // 临时目录里，系统默认程序照样打得开。
        let ready = state.local.is_some();
        if ui
            .add_enabled(ready, egui::Button::new(t(lang, "Open Externally")))
            .clicked()
        {
            *action = Some(Action::OpenExternally);
        }
        if ui.button(t(lang, "Close")).clicked() {
            *action = Some(Action::Close);
        }
    });
}

/// 一行「这是什么」的数据：文本报字符集，图片报像素，二进制/空/错误不重复说。
fn fact_line(state: &PreviewState) -> Option<String> {
    match &state.content {
        PreviewContent::Text { charset, .. } => Some(charset_label(*charset).to_string()),
        PreviewContent::Image {
            format,
            width,
            height,
        } => Some(if *width == 0 || *height == 0 {
            // 宽高读不出来时**不猜**（`term_io::preview` 的口径）：只报格式。
            format_label(*format).to_string()
        } else {
            format!("{}  {width}×{height}", format_label(*format))
        }),
        _ => None,
    }
}

/// 图片区：解码成功就按比例缩进正文区，失败就画占位框。
fn draw_image(
    ui: &mut egui::Ui,
    state: &PreviewState,
    format: ImageFormat,
    width: u32,
    height: u32,
    dim: egui::Color32,
) {
    let Some(texture) = &state.texture else {
        // 占位框 + 一行说明：`note` 已经带着「无法解码」或「太大」的原因。
        placeholder(
            ui,
            BODY_H,
            format_label(format),
            &format!("{width}×{height}"),
            dim,
        );
        return;
    };
    let size = egui::vec2(texture.size()[0] as f32, texture.size()[1] as f32);
    // 等比缩进正文区：`Image::max_size` 会同时约束两个方向，不会拉变形。
    let scaled = egui::vec2(
        size.x.min(CONTENT_MIN.x - 8.0),
        size.y.min(BODY_H - 8.0),
    );
    ui.add(egui::Image::new(egui::load::SizedTexture::new(
        texture.id(),
        scaled,
    )));
}

/// 一块占位区：标题 + 一行细节，居中、带细边框。
///
/// 用绘图原语而不是 widget 堆叠，是为了它无论内容长短都占同一块高度——否则
/// 三行文案与一行文案的窗口高度会差 40 点，截图基线就不再可复现。
fn placeholder(
    ui: &mut egui::Ui,
    height: f32,
    title: &str,
    detail: &str,
    dim: egui::Color32,
) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, crate::theme::chrome(term_core::theme::CHROME_SCALE.line)),
    );
    painter.text(
        rect.center() - egui::vec2(0.0, 7.0),
        egui::Align2::CENTER_CENTER,
        title,
        crate::ui::type_scale::heading(),
        dim,
    );
    painter.text(
        rect.center() + egui::vec2(0.0, 11.0),
        egui::Align2::CENTER_CENTER,
        detail,
        crate::ui::type_scale::small(),
        dim,
    );
}

/// 提示语：key 走词表，平台原因原样附在「打不开」那句后面。
fn note_text(lang: Lang, note: &Note) -> String {
    match note {
        Note::Key(key) => t(lang, key).to_string(),
        Note::OpenFailed(detail) => format!(
            "{}: {detail}",
            t(lang, "Could not open with the system default program")
        ),
    }
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
