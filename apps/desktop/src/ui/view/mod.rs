//! R2（view / 纯计算层）：可序列化的几何与文案规格，零 egui 依赖。
//!
//! `1280×937`（第一批，1× 物理像素），egui 逻辑点 = 物理 × 0.6，
//! 即 `768×562`。本模块的常数一律是**egui 逻辑点**，可直接被 R3 绘制层消费，
//! 也可序列化成 JSON 换渲染后端（ADR-0021 §2 的四类产物契约）。

pub mod trigger_pages_spec;
pub mod trigger_spec;
