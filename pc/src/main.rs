#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DroidTransfer")
            .with_inner_size([1000.0, 640.0])
            .with_min_inner_size([760.0, 460.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "DroidTransfer",
        options,
        Box::new(|cc| {
            let ctx = cc.egui_ctx.clone();
            let core = droidtransfer::core::Core::start(Box::new(move || ctx.request_repaint()))
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
            Ok(Box::new(ui::App::new(cc, core)))
        }),
    )
}
