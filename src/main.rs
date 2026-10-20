#![cfg_attr(windows, windows_subsystem = "windows")]
use clap::Parser;
use scan_angle::{
    batch::{Progress, run_batch},
    cli::Cli,
    gui,
};
fn main() {
    // A double-click opens only the GUI; CLI invocations reuse their parent terminal.
    #[cfg(windows)]
    if std::env::args_os().len() > 1 {
        unsafe {
            let _ = windows::Win32::System::Console::AttachConsole(
                windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
            );
        }
    }
    let cli = Cli::parse();
    let settings = cli.settings().unwrap_or_else(|e| e.exit());
    let result = (|| -> anyhow::Result<()> {
        if let Some(options) = cli.batch_options(settings.clone())? {
            run_batch(&options, |p| {
                if let Progress::Log(s) = p {
                    println!("{s}")
                }
            })?;
        } else {
            gui::run(cli, settings)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("错误: {error:#}");
        std::process::exit(1);
    }
}
