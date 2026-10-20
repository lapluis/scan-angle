#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]
#[cfg(feature = "gui")]
use scan_angle::gui;
use scan_angle::{
    batch::{run_batch, run_batch_details},
    cli::Cli,
    i18n::I18n,
    report,
};
use std::io::{self, Write};

fn print_stdout(text: &str) -> anyhow::Result<()> {
    match writeln!(io::stdout().lock(), "{text}") {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn main() {
    // A double-click opens only the GUI; CLI invocations reuse their parent terminal.
    #[cfg(all(windows, feature = "gui"))]
    if std::env::args_os().len() > 1 {
        unsafe {
            let _ = windows::Win32::System::Console::AttachConsole(
                windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
            );
        }
    }
    let args: Vec<_> = std::env::args_os().collect();
    let language = Cli::load_language(&args).unwrap_or_else(|error| {
        let fallback = Cli::bootstrap(&args)
            .0
            .and_then(|id| I18n::with_locale(&id).ok())
            .unwrap_or_default();
        eprintln!(
            "{}: {}",
            fallback.translate("app.error_prefix"),
            fallback.translate_message(&error)
        );
        std::process::exit(2);
    });
    for warning in language.warning_texts() {
        eprintln!("{warning}");
    }
    let exit_parse = |error: clap::Error| -> ! {
        let text = Cli::error_text(&error, &language);
        if error.use_stderr() {
            eprint!("{text}");
        } else {
            print!("{text}");
        }
        std::process::exit(error.exit_code());
    };
    let cli = Cli::parse_localized(&args, &language).unwrap_or_else(|e| exit_parse(e));
    let settings = cli
        .settings_localized(&language)
        .unwrap_or_else(|e| exit_parse(e));
    let result = (|| -> anyhow::Result<()> {
        if let Some(options) = cli.batch_options(settings.clone())? {
            if cli.json {
                let (_, details) = run_batch_details(&options, |_| {})?;
                print_stdout(&serde_json::to_string_pretty(&details)?)?;
            } else {
                let rows = run_batch(&options, |_| {})?;
                print_stdout(&report::format_table_localized(&rows, &language))?;
            }
        } else {
            #[cfg(feature = "gui")]
            gui::run_localized(cli, settings, language.clone())?;
            #[cfg(not(feature = "gui"))]
            {
                eprintln!(
                    "{}: {}",
                    language.translate("app.error_prefix"),
                    language.translate("cli.error.no_gui_requires_folder")
                );
                std::process::exit(2);
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!(
            "{}: {}",
            language.translate("app.error_prefix"),
            language.render_anyhow(&error)
        );
        std::process::exit(1);
    }
}
