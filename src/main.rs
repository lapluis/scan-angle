#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]
#[cfg(not(feature = "gui"))]
use clap::CommandFactory;
use clap::Parser;
#[cfg(feature = "gui")]
use scan_angle::gui;
use scan_angle::{batch::run_batch, cli::Cli, report};

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
    let cli = Cli::parse();
    let settings = cli.settings().unwrap_or_else(|e| e.exit());
    let result = (|| -> anyhow::Result<()> {
        if let Some(options) = cli.batch_options(settings.clone())? {
            let rows = run_batch(&options, |_| {})?;
            println!("{}", report::format_table(&rows));
        } else {
            #[cfg(feature = "gui")]
            gui::run(cli, settings)?;
            #[cfg(not(feature = "gui"))]
            Cli::command()
                .error(
                    clap::error::ErrorKind::MissingRequiredArgument,
                    "无 GUI 版本必须提供输入文件夹，例如：\n    scan-angle <输入目录> [--output <输出目录>]",
                )
                .exit();
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("错误: {error:#}");
        std::process::exit(1);
    }
}
