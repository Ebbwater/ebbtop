//! EbbTop: native btop-style monitor window for remote hosts.
//!
//! The window title starts generic; the app retitles it per active source.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod store;
mod update;
mod widgets;

/// Best-effort crash reporter: the Windows GUI subsystem hides stderr, so a
/// panic would otherwise vanish with the window. This appends the panic
/// payload and source location to ebbtop.log, then chains to the default
/// hook (terminal runs keep their output too).
fn install_crash_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut dir =
                dirs::data_local_dir().ok_or_else(|| std::io::Error::other("no data dir"))?;
            dir.push("ebbtop");
            std::fs::create_dir_all(&dir)?;
            dir.push("ebbtop.log");
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&dir)?;
            writeln!(f, "{:?} PANIC: {info}", std::time::SystemTime::now())?;
            Ok(())
        })();
        prev(info);
    }));
}

fn main() -> eframe::Result {
    install_crash_hook();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EBBTOP")
            .with_app_id("ebbtop")
            .with_icon(egui::IconData {
                width: 64,
                height: 64,
                rgba: include_bytes!("../assets/ebbtop.rgba").to_vec(),
            })
            .with_inner_size([960.0, 1000.0])
            .with_min_inner_size([480.0, 420.0]),
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native("ebbtop", opts, Box::new(|cc| Ok(Box::new(app::App::new(cc.egui_ctx.clone())))))
}
