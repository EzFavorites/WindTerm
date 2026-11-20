//! 预览窗的纯层单测（`mod.rs` 靠 `#[path]` 挂回 `overlays/preview.rs`）。
//!
//! 只测**不碰 egui** 的那一半：`content_of` 的六种结局、`human_bytes` 的五档、
//! 截断判据、路径拆分。它们全是「输入路径 / 字节，输出可以直接画的东西」——
//! 这一层红了就是门禁真的被撞了，跟渲染器版本无关。
//!
//! 夹具沿用 `term_io::preview` 的写法（`temp_dir()` + 纳秒后缀 + `Drop` 清理），
//! 不引 `tempfile`：那会给本 crate 的依赖表多一条只为测试存在的东西。

use super::*;
use std::path::PathBuf;

// ---- 夹具 ----------------------------------------------------------------

/// 一次性临时目录。`Drop` 里整个删掉——测试失败时也不会在 `/tmp` 留垃圾。
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("windterm-previewui-{tag}-{nanos}"));
        std::fs::create_dir_all(&path).expect("临时目录要建得出来");
        TempDir { path }
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path.join(name);
        std::fs::write(&path, bytes).expect("临时文件要写得进去");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// 最小 PNG 头：8 字节签名 + 长度 + `IHDR` + 宽 + 高。
///
/// 没有像素数据，所以**解码**一定失败——`content_of` 只嗅魔数，正好覆盖
/// 「认得格式但解不开」这个真实形态。
fn png_header(width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    out.extend_from_slice(&13u32.to_be_bytes());
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&[0x08, 0x02, 0x00, 0x00, 0x00]);
    out
}

// ---- human_bytes：五档 ---------------------------------------------------

#[test]
fn human_bytes_covers_bytes_kib_and_mib_edges() {
    // 五档各一条。1023 是「不足 1 KiB 就报字节」的最后一格，1024 是翻转点，
    // 1.5 MiB 是「保留一位小数」的常规路径。
    assert_eq!(human_bytes(0), "0 B");
    assert_eq!(human_bytes(1), "1 B");
    assert_eq!(human_bytes(1023), "1023 B");
    assert_eq!(human_bytes(1024), "1.0 KiB");
    assert_eq!(human_bytes(1024 * 1024 + 512 * 1024), "1.5 MiB");
    // GiB 那一档不是死代码：日志文件真的会到 1 GB 以上。
    assert_eq!(human_bytes(1024 * 1024 * 1024), "1.0 GiB");
}

// ---- content_of：六种结局 -------------------------------------------------

#[test]
fn a_real_text_file_is_text_and_not_truncated() {
    let dir = TempDir::new("text");
    let path = dir.file("notes.txt", b"hello\nworld\n");
    match content_of(&path) {
        PreviewContent::Text {
            body,
            truncated,
            charset,
        } => {
            assert_eq!(body, "hello\nworld");
            assert!(!truncated, "这么短的文件不该被判成截断");
            assert_eq!(charset, Charset::Utf8);
        }
        _ => panic!("普通文本文件必须走 Text 通道"),
    }
}

#[test]
fn an_empty_file_reports_empty_not_text() {
    let dir = TempDir::new("empty");
    let path = dir.file("zero.txt", b"");
    assert!(
        matches!(content_of(&path), PreviewContent::Empty),
        "空文件要显示「空文件」，不是一片空白"
    );
}

#[test]
fn a_nul_byte_makes_it_binary_and_the_size_is_the_whole_file() {
    let dir = TempDir::new("binary");
    let body = vec![0x00, 0x01, 0x02, 0x03];
    let path = dir.file("blob.bin", &body);
    match content_of(&path) {
        PreviewContent::Binary { size } => assert_eq!(size, body.len() as u64),
        _ => panic!("含 0x00 的内容必须判成二进制"),
    }
}

#[test]
fn a_missing_path_is_an_error_with_a_readable_message() {
    let dir = TempDir::new("missing");
    let path = dir.path.join("nope.txt");
    match content_of(&path) {
        PreviewContent::Error { message } => {
            assert!(!message.is_empty(), "错误行不能是空串");
            assert!(message.contains("no such file"), "实际：{message}");
        }
        _ => panic!("不存在的路径必须走 Error"),
    }
}

#[test]
fn a_directory_is_an_error_not_a_crash() {
    let dir = TempDir::new("isdir");
    match content_of(&dir.path) {
        PreviewContent::Error { message } => {
            assert!(message.contains("directory"), "实际：{message}");
        }
        _ => panic!("目录必须走 Error，而不是空内容或二进制"),
    }
}

#[test]
fn a_png_header_is_recognised_as_a_png_with_its_size() {
    let dir = TempDir::new("png");
    let path = dir.file("logo.png", &png_header(640, 480));
    match content_of(&path) {
        PreviewContent::Image {
            format,
            width,
            height,
        } => {
            assert_eq!(format, ImageFormat::Png);
            assert_eq!((width, height), (640, 480), "尺寸来自 IHDR");
        }
        _ => panic!("PNG 必须走 Image 通道"),
    }
}

#[test]
fn a_long_text_is_flagged_as_truncated() {
    let dir = TempDir::new("long");
    // 250 行 > `PREVIEW_MAX_LINES`（200）：行数裁剪必须被看见。
    let body: Vec<u8> = (0..250)
        .flat_map(|i| format!("line {i}\n").into_bytes())
        .collect();
    let path = dir.file("long.log", &body);
    match content_of(&path) {
        PreviewContent::Text { body, truncated, .. } => {
            assert_eq!(body.lines().count(), 200, "正文要裁到 200 行");
            assert!(truncated, "裁掉了 50 行，必须告诉用户还有更多");
        }
        _ => panic!("长文本必须走 Text 通道"),
    }
}

#[test]
fn a_text_file_bigger_than_the_head_window_is_flags_truncated() {
    let dir = TempDir::new("huge");
    // 超过 64 KiB 头窗口、但行数不到 200：截断标记只能来自字节数。
    let body = vec![b'x'; term_io::preview::DEFAULT_HEAD_BYTES + 1];
    let path = dir.file("one-line.txt", &body);
    match content_of(&path) {
        PreviewContent::Text { truncated, .. } => assert!(truncated, "64 KiB 之后还有内容"),
        _ => panic!("这么长的一行仍是文本"),
    }
}

// ---- needs_truncation_note ----------------------------------------------

#[test]
fn the_truncation_note_only_fires_for_truncated_text() {
    let truncated = PreviewContent::Text {
        body: String::from("a"),
        truncated: true,
        charset: Charset::Utf8,
    };
    let whole = PreviewContent::Text {
        body: String::from("a"),
        truncated: false,
        charset: Charset::Utf8,
    };
    assert!(needs_truncation_note(&truncated));
    assert!(!needs_truncation_note(&whole));
    // 其余四种各有自己的说法，不该再叠一句「只显示了开头」。
    for other in [
        PreviewContent::Image {
            format: ImageFormat::Png,
            width: 1,
            height: 1,
        },
        PreviewContent::Binary { size: 9 },
        PreviewContent::Empty,
        PreviewContent::Error {
            message: String::from("x"),
        },
    ] {
        assert!(
            !needs_truncation_note(&other),
            "非文本不该有截断提示：{}",
            content_label(&other)
        );
    }
}

/// 断言失败时给个人能读的标签（`PreviewContent` 没实现 `Debug`：它带着
/// `egui::TextureHandle` 的兄弟类型口径，故意不加）。
fn content_label(content: &PreviewContent) -> &'static str {
    match content {
        PreviewContent::Text { .. } => "Text",
        PreviewContent::Image { .. } => "Image",
        PreviewContent::Binary { .. } => "Binary",
        PreviewContent::Empty => "Empty",
        PreviewContent::Error { .. } => "Error",
    }
}

// ---- 标题 / 路径拆分 -----------------------------------------------------

#[test]
fn the_title_is_the_last_path_segment_for_local_and_remote_paths() {
    // 远端 `host:/a/b/c.txt`
    assert_eq!(basename_of("host:/a/b/c.txt"), "c.txt");
    // 本地绝对路径
    assert_eq!(basename_of("/Users/unique/notes.md"), "notes.md");
    // 裸文件名
    assert_eq!(basename_of("c.txt"), "c.txt");
    // 尾斜杠（目录形态）不该给出空标题
    assert_eq!(basename_of("/tmp/dir/"), "dir");
    assert_eq!(basename_of("/"), "/", "根目录没有名字时退回整条路径");
    // Windows 远端（SFTP 服务端可能是 Windows）同样只取最后一段
    assert_eq!(basename_of(r"C:\Users\x\a.txt"), "a.txt");
}

#[test]
fn the_charset_and_format_labels_are_data_not_translated_chrome() {
    // 这两个标签是**数据**（文件编码 / 图片格式），不进词表；钉住它们保持原文，
    // 免得有人顺手把它塞进 `zh()` 之后中文界面上出现「UTF-8 BOM」的译文。
    assert_eq!(charset_label(Charset::Utf8), "UTF-8");
    assert_eq!(charset_label(Charset::Latin1), "ISO-8859-1");
    assert_eq!(format_label(ImageFormat::Png), "PNG");
    assert_eq!(format_label(ImageFormat::WebP), "WebP");
    // 字面量必须与 `term_io::preview` 的枚举一一对上（加格式时这里先红）。
    let all = [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Gif,
        ImageFormat::Bmp,
        ImageFormat::WebP,
    ];
    for format in all {
        assert!(!format_label(format).is_empty());
    }
}

// ---- 纹理解码的失败路径 --------------------------------------------------

#[test]
fn decoding_a_header_only_png_fails_instead_of_panicking() {
    // 这是「解码失败时保留 Image + 画占位框」那条分支的来源：只有 IHDR 的文件
    // 在 `image` 那里必然报错，而 `content_of` 已经先把它认成 PNG 了。
    let dir = TempDir::new("broken-png");
    let path = dir.file("broken.png", &png_header(4, 4));
    assert!(matches!(
        content_of(&path),
        PreviewContent::Image {
            format: ImageFormat::Png,
            width: 4,
            height: 4
        }
    ));
    // 没有 egui 上下文时不能调 `decode_texture`（它要 `&egui::Context`），
    // 所以这里只钉住「文件确实解不开」这个前提——真正的那条分支靠
    // `image::load_from_memory` 的 `Err`，它不需要上下文。
    let bytes = std::fs::read(&path).expect("刚写进去的文件读得回来");
    assert!(
        image::load_from_memory(&bytes).is_err(),
        "只有头的 PNG 不该被解出来"
    );
}

// ---- 远端预览的落点 ------------------------------------------------------

#[test]
fn a_remote_download_lands_under_the_temp_dir_with_its_basename() {
    // 远端预览的整条链：`temp_preview_path` 造落点 → SFTP 下载 → 开窗。
    // 这里只测第一环（后面两环要真连接），但落点错了后面全错。
    let path = crate::open_with::temp_preview_path("host:/srv/app/notes.txt")
        .expect("临时目录要建得出来");
    assert_eq!(
        path.file_name().and_then(|n| n.to_str()),
        Some("notes.txt")
    );
    assert!(path.parent().is_some_and(|p| p.is_dir()));
    let _ = std::fs::remove_dir_all(path.parent().unwrap_or(&std::env::temp_dir()));
}

// ---- 真绘制一帧 ----------------------------------------------------------

/// 唯一的**绘制**测试：拿真 PNG 开窗，跑一帧，再关掉。
///
/// 为什么值得一条：纯层测不到 `TextWrapMode` / 纹理 / 窗口几何这些 API 用法，
/// 而它们错了的后果是「运行时 panic」——四张视觉基线只有在有人肉眼跑截图时才会
/// 发现。这一帧把「开窗 → 解码上纹理 → 画 → 关窗」整条链走一遍，两个可见性
/// 字段是否同步也一并钉住。
#[test]
fn opening_a_real_png_decodes_a_texture_and_draws_one_frame() {
    let dir = TempDir::new("texture");
    // 用 `image` 自己编码：手写 PNG 字节的 CRC 错一位就变成「测解码失败」，
    // 而这里要测的是成功那条路。
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 40, 40, 255]))
        .write_to(&mut png, image::ImageOutputFormat::Png)
        .expect("编码 PNG");
    let path = dir.file("dot.png", png.get_ref());

    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut window = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
    window.open_preview_local(&path);
    assert!(
        window.view.show_preview && window.preview.open,
        "开窗要同时置两个位（两个字段描述同一件事）"
    );
    assert!(
        window.preview.texture.is_some(),
        "4×4 的真 PNG 必须解得开并上纹理"
    );
    // 一帧真绘制：API 用错会在这里炸，而不是等到用户打开预览窗。
    let _ = ctx.run(Default::default(), |ctx| window.draw_preview(ctx));
    window.close_preview();
    assert!(
        !window.view.show_preview && !window.preview.open,
        "关窗也要同时清两个位"
    );
    assert!(window.preview.texture.is_none(), "关窗要把显存句柄一起放掉");
}
