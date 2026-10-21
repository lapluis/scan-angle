use scan_angle::{
    batch::{run_batch, run_batch_details},
    cli::Cli,
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
    let args: Vec<_> = std::env::args_os().collect();
    let language = Cli::language_or_exit(&args);
    let cli = Cli::parse_localized(&args, &language, true)
        .unwrap_or_else(|error| Cli::exit_with_parse_error(error, &language, true));
    let settings = cli
        .settings_localized(&language)
        .unwrap_or_else(|error| Cli::exit_with_parse_error(error, &language, true));
    let result = (|| -> anyhow::Result<()> {
        let options = cli
            .batch_options(settings)?
            .expect("the command-line parser requires a folder");
        if cli.json {
            let (_, details) = run_batch_details(&options, |_| {})?;
            print_stdout(&serde_json::to_string_pretty(&details)?)?;
        } else {
            let rows = run_batch(&options, |_| {})?;
            print_stdout(&report::format_table_localized(&rows, &language))?;
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
