//! 自己拿截图，不用 eframe 的 `EFRAME_SCREENSHOT_TO`。
//!
//! # 为什么不用 eframe 那条路
//!
//! eframe 的机制是：每一帧结束后判 `cumulative_pass_nr() == 2`，成立就
//! `read_screen_rgba` 读屏、写 PNG、退出。两个坑都实测过：
//!
//! 1. **它读的是「上一帧」的画面。** 读屏发生在 `swap_buffers` **之后**，
//!    双缓冲下拿到的还是上一帧的内容。画面静止时看不出来（两帧一样），
//!    但任何「首帧和后续帧不同」的组件都会出问题——`Grid` 首帧是
//!    `sizing_pass().invisible()`（只量尺寸、不画），于是冻出来的
//!    `dialog_shortcuts` 基线里表格是**空的**，而屏幕上其实是好的。
//! 2. **一次 pass 丢弃就能让它存一张全透明图。** `Grid` / `ScrollArea` 首帧
//!    会 `request_discard`，egui 在**同一帧内**再跑一个 pass（计数 0 → 2），
//!    正好命中 eframe 的保存条件，存下来的那一帧是被丢弃的、形状全空的帧。
//!
//! # 这里的做法
//!
//! 走 egui 的运行时截图命令：`ViewportCommand::Screenshot` → 后端在
//! `paint_and_update_textures` 之后、`swap_buffers` 之前读屏，所以拿到的是
//! **刚画完的那一帧**；下一帧以 `Event::Screenshot` 事件送回。我们自己决定
//! 「画到第几帧再要图」，于是可以等到布局稳定（`SETTLE_FRAMES`）再拍。
//!
//! 判据仍然是环境变量（`WINDTERM_SCREENSHOT_TO`），因为 `ui` 层拿不到 argv。

use std::path::{Path, PathBuf};

/// 拍图之前先画几帧。第 0 帧是 egui 的首次布局（`Grid` / `ScrollArea` 在这
/// 一帧请求丢弃并只量尺寸），从第 1 帧起才是「屏幕上看到的那个样子」。
/// 取 2 是留一帧余量：字体图集上传、`Area` 尺寸收敛都在这之前完成。
const SETTLE_FRAMES: u32 = 2;

/// 一次截图任务：目标路径 + 已经画到第几帧 + 命令是否已发出。
pub struct Shot {
    path: PathBuf,
    frame: u32,
    requested: bool,
}

impl Shot {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            frame: 0,
            requested: false,
        }
    }

    /// 每帧调一次（在 `draw` 之后）：收图 → 存盘 → 退出；没到点就继续数帧。
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(image) = take_screenshot(ctx) {
            match save_png(&self.path, &image) {
                Ok(()) => {
                    // 与 eframe 的做法一致：写完就退出，别留一个空窗口。
                    // 退出码 0 是「图已经落盘」的唯一凭据，脚本据此判红绿。
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("截图失败：{}：{e}", self.path.display());
                    std::process::exit(1);
                }
            }
        }

        if self.requested {
            // 命令发出去了，等下一帧把图送回来。
            ctx.request_repaint();
            return;
        }

        self.frame += 1;
        if self.frame >= SETTLE_FRAMES {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        // 不 request_repaint 的话，静止画面下 eframe 会等到下一帧才继续，
        // 于是这个过程可能停在第 1 帧上再也不动。
        ctx.request_repaint();
    }
}

/// 从这一帧的输入里取出截图事件（后端在上一帧画完后放进来的）。
fn take_screenshot(ctx: &egui::Context) -> Option<std::sync::Arc<egui::ColorImage>> {
    ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Screenshot { image, .. } => Some(image.clone()),
            _ => None,
        })
    })
}

/// `ColorImage` → PNG。预乘 alpha 不在这里处理：egui 给的是直通 RGBA。
fn save_png(path: &Path, image: &egui::ColorImage) -> Result<(), image::ImageError> {
    let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        rgba.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
    }
    image::save_buffer(
        path,
        &rgba,
        image.size[0] as u32,
        image.size[1] as u32,
        image::ColorType::Rgba8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_shot_waits_for_the_settle_frames() {
        // 这条守的是「别一拍就退出」：第 0 帧的画面是 sizing pass，
        // 拍下来就是一张只画了一半的窗口。
        let shot = Shot::new("/tmp/never-written.png");
        assert_eq!(shot.frame, 0);
        assert!(!shot.requested);
        // 首帧是 egui 的 sizing pass：一拍就退出的实现会把它当成品。
        const { assert!(SETTLE_FRAMES >= 2) };
    }

    #[test]
    fn png_keeps_size_and_alpha() {
        let dir = std::env::temp_dir();
        let path = dir.join("zterm-shot-unit.png");
        let mut pixels = Vec::new();
        for i in 0..4u8 {
            pixels.push(egui::Color32::from_rgba_unmultiplied(i * 60, 10, 20, 255));
        }
        let image = egui::ColorImage {
            size: [2, 2],
            pixels,
        };
        save_png(&path, &image).expect("写 PNG");
        let back = image::open(&path).expect("读回 PNG").to_rgba8();
        assert_eq!(back.dimensions(), (2, 2));
        assert_eq!(back.get_pixel(1, 0).0, [60, 10, 20, 255]);
        let _ = std::fs::remove_file(&path);
    }
}
