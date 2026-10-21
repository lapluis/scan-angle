#![cfg_attr(windows, windows_subsystem = "windows")]

use scan_angle::{cli::Cli, gui};

fn main() {
    // A console attached to a parent terminal makes argument errors visible
    // when the GUI executable is started from a shell.
    #[cfg(windows)]
    if std::env::args_os().len() > 1 {
        unsafe {
            let _ = windows::Win32::System::Console::AttachConsole(
                windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
            );
        }
    }
    let args: Vec<_> = std::env::args_os().collect();
    let language = Cli::language_or_exit(&args);
    let cli = Cli::parse_localized(&args, &language, false)
        .unwrap_or_else(|error| Cli::exit_with_parse_error(error, &language, false));
    let settings = cli
        .settings_localized(&language)
        .unwrap_or_else(|error| Cli::exit_with_parse_error(error, &language, false));
    if let Err(error) = gui::run_localized(cli, settings, language.clone()) {
        eprintln!(
            "{}: {}",
            language.translate("app.error_prefix"),
            language.render_anyhow(&error)
        );
        std::process::exit(1);
    }
}
