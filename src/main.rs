//! Aurora — a modern SSH / SFTP / RDP / Serial client.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod i18n;
mod app;
mod backend;
mod history;
mod keygen;
mod model;
mod nav2_dyn;
mod nav2_order;
mod nav_model;
mod nav_perf;
mod nav_query;
mod nav_tree;
mod store;
mod terminal;
mod theme;
mod themeimport_convert;
mod themeimport_parse;
mod ui;
mod update;
mod whatsnew_data;

fn main() -> eframe::Result {
    env_logger::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Aurora")
            .with_app_id("aurora-term")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 480.0])
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
