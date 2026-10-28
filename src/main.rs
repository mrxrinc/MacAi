mod app;
mod engine;
mod model;
mod process;
mod setup;
mod storage;

fn main() -> eframe::Result<()> {
    if let Some(argument) = std::env::args().nth(1) {
        match argument.as_str() {
            "--check" => {
                match setup::check(&std::sync::atomic::AtomicBool::new(false)) {
                    Ok(setup::Readiness::Ready) => {
                        println!("System model available; Apple terms accepted.")
                    }
                    Ok(setup::Readiness::LicenseRequired(_)) => {
                        eprintln!("Apple's terms have not been accepted. Open MacAi to review and accept them in the setup screen.");
                        std::process::exit(1);
                    }
                    Ok(setup::Readiness::ModelUnavailable(error)) => {
                        eprintln!("{error}");
                        std::process::exit(1);
                    }
                    Err(error) => {
                        eprintln!("{error}");
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            "--help" | "-h" => {
                println!("MacAi - private, offline chat with Apple's on-device model\n\nUsage: MacAi [--check | --help]\n\nRun without arguments to open the chat window.\n--check  Check local model availability without opening a window.\n\nChats: ~/Library/Application Support/MacAi/history.json");
                return Ok(());
            }
            _ => {
                eprintln!("Unknown argument: {argument}. Use --help for usage.");
                std::process::exit(2);
            }
        }
    }
    eframe::run_native(
        "MacAi",
        native_options(),
        Box::new(|cc| Ok(Box::new(app::MacAi::new(cc)))),
    )
}

fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("MacAi")
            .with_inner_size([1080.0, 780.0])
            .with_min_inner_size([760.0, 560.0])
            .with_app_id("local.macai.chat")
            .with_icon(app::icon_data()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dock_icon_is_explicitly_set_to_the_same_artwork_as_the_ui() {
        let icon = super::native_options()
            .viewport
            .icon
            .expect("an explicit icon prevents the framework's default Dock icon");
        let shared = crate::app::icon_data();
        assert_eq!((icon.width, icon.height), (shared.width, shared.height));
        assert_eq!(icon.rgba, shared.rgba);
    }
}
