//! The window's clipboard.
//!
//! egui owns clipboard access, so a program's `OSC 52` request (which the
//! original also answers) has to be routed back through the UI thread: the
//! emulator parks the request on the terminal, the frame handler performs it
//! with the egui context, and reads answer from the text egui last copied.

use eframe::egui;

/// Perform a clipboard write the emulator asked for.
pub fn write(ctx: &egui::Context, text: String) {
    ctx.copy_text(text);
}

/// The text egui last copied, which is what an `OSC 52` read should report.
pub fn read(ctx: &egui::Context) -> String {
    ctx.output(|o| o.copied_text.clone())
}
