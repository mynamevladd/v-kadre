// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appearance;
mod demo;
mod detector;
mod engine;
mod framing;
mod i18n;
mod logger;
mod lower_third;
mod obs;
mod rgb;
mod settings;
mod tracker;
mod ui;
mod watchdog;

slint::include_modules!();

use settings::{Settings, APP_NAME, VERSION};

/// `VKadre.exe --selftest`: checks the bundled model and the GPU, result goes to the log.
fn selftest() -> i32 {
    use detector::{Detect, PoseDetector};
    let s = Settings::load();
    let r = (|| -> anyhow::Result<String> {
        let d = PoseDetector::new(&s.model_path(), s.use_gpu)?;
        d.detect(&rgb::RgbImage::new(640, 360, 0), 0.4)?;
        Ok(d.device_name())
    })();
    match r {
        Ok(dev) => {
            log::info!("SELFTEST OK: model {}, device {dev}", s.model);
            println!("SELFTEST OK: {dev}");
            0
        }
        Err(e) => {
            log::error!("SELFTEST FAILED: {e:#}");
            eprintln!("SELFTEST FAILED: {e:#}");
            1
        }
    }
}

fn main() {
    logger::init();
    log::info!("{APP_NAME} {VERSION} starting (Rust)");
    if std::env::args().any(|a| a == "--selftest") {
        std::process::exit(selftest());
    }
    // `--demo picture.jpg`: the picture plays the camera, no OBS needed
    if let Err(e) = demo::init_from_args() {
        log::error!("{e:#}");
        eprintln!("{e:#}");
        std::process::exit(2);
    }
    let mut settings = Settings::load();
    if demo::active() {
        demo::apply(&mut settings);
    }
    if let Err(e) = ui::run(settings) {
        log::error!("UI error: {e:#}");
        std::process::exit(1);
    }
}
