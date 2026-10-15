//! Aurora — a modern SSH / SFTP / RDP / Serial client.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod backend;
mod keygen;
mod model;
mod store;
mod terminal;
mod theme;
mod ui;

fn main() -> eframe::Result {
    env_logger::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Aurora")
            .with_app_id("aurora-term")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([860.0, 540.0])
            .with_drag_and_drop(true)
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .unwrap_or_default(),
            ),
        ..Default::default()
    };
    eframe::run_native(
        "Aurora",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
