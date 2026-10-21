use crate::i18n::{DEFAULT_LOCALE, I18n, LoadOptions, LocalizedMessage};
use crate::{batch::BatchOptions, detection::DetectionSettings};
use clap::{CommandFactory, FromArgMatches, Parser};
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(version)]
pub struct Cli {
    pub folder: Option<PathBuf>,
    #[arg(long)]
    pub lang: Option<String>,
    #[arg(long)]
    pub lang_file: Option<PathBuf>,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(short, long)]
    pub recursive: bool,
    #[arg(long, default_value_t = 15., allow_hyphen_values = true)]
    pub max_angle: f64,
    #[arg(long, default_value_t = 10., allow_hyphen_values = true)]
    pub min_line_percent: f64,
    #[arg(long, default_value_t = 50., allow_hyphen_values = true)]
    pub min_line_pixels: f64,
    #[arg(long, default_value_t = 2400)]
    pub analysis_size: u32,
    #[arg(long, default_value = "lanczos")]
    pub resample: String,
    #[arg(long, default_value_t = 0.75, allow_hyphen_values = true)]
    pub merge_angle: f64,
    #[arg(long, default_value_t = 0.12, allow_hyphen_values = true)]
    pub merge_distance_percent: f64,
    #[arg(long, default_value_t = 1.5, allow_hyphen_values = true)]
    pub merge_distance_pixels: f64,
    #[arg(long, default_value_t = 0.5, allow_hyphen_values = true)]
    pub merge_gap_percent: f64,
    #[arg(long, default_value_t = 6., allow_hyphen_values = true)]
    pub merge_gap_pixels: f64,
    #[arg(long)]
    pub allow_large_images: bool,
    #[arg(long, requires = "folder")]
    pub json: bool,
}
impl Cli {
    pub fn settings(&self) -> Result<DetectionSettings, clap::Error> {
        self.settings_localized(&I18n::default())
    }
    pub fn settings_localized(&self, i18n: &I18n) -> Result<DetectionSettings, clap::Error> {
        let s = DetectionSettings {
            max_angle: self.max_angle,
            min_line_percent: self.min_line_percent,
            min_line_pixels: self.min_line_pixels,
            analysis_size: self.analysis_size,
            resample: self.resample.trim().to_lowercase(),
            merge_angle: self.merge_angle,
            merge_distance_percent: self.merge_distance_percent,
            merge_distance_pixels: self.merge_distance_pixels,
            merge_gap_percent: self.merge_gap_percent,
            merge_gap_pixels: self.merge_gap_pixels,
        };
        let result = s.validate().and_then(|()| {
            if (1. ..=30.).contains(&s.max_angle) {
                Ok(())
            } else {
                Err(LocalizedMessage::new("cli.error.max_angle_range").into())
            }
        });
        if let Err(e) = result {
            let mut message = i18n.render_anyhow(&e);
            for name in [
                "max_angle",
                "min_line_percent",
                "min_line_pixels",
                "analysis_size",
                "resample",
                "merge_angle",
                "merge_distance_percent",
                "merge_distance_pixels",
                "merge_gap_percent",
                "merge_gap_pixels",
            ] {
                message = message.replace(name, &format!("--{}", name.replace('_', "-")));
            }
            let mut error = Self::localized_command(i18n)
                .error(clap::error::ErrorKind::ValueValidation, message.clone());
            error.insert(
                clap::error::ContextKind::Custom,
                clap::error::ContextValue::String(message),
            );
            return Err(error);
        }
        Ok(s)
    }

    /// Scan only language options before Clap handles help/errors. Other value
    /// options are consumed so a path or value cannot masquerade as a flag.
    pub fn bootstrap(args: &[OsString]) -> (Option<String>, Option<PathBuf>) {
        let mut lang = None;
        let mut file = None;
        let mut index = 1;
        while index < args.len() {
            let value = args[index].to_string_lossy();
            if value == "--" {
                break;
            }
            if let Some(locale) = value.strip_prefix("--lang=") {
                lang = Some(locale.to_owned());
            } else if value == "--lang" || value == "--lang-file" {
                if let Some(next) = args.get(index + 1)
                    && !next.to_string_lossy().starts_with('-')
                {
                    if value == "--lang" {
                        lang = Some(next.to_string_lossy().into_owned());
                    } else {
                        file = Some(PathBuf::from(next));
                    }
                    index += 1;
                }
            } else if args[index].as_encoded_bytes().starts_with(b"--lang-file=") {
                // Preserve native path encoding, including non-UTF8 Unix paths.
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::{OsStrExt, OsStringExt};
                    file = Some(PathBuf::from(OsString::from_wide(
                        &args[index].encode_wide().skip(12).collect::<Vec<_>>(),
                    )));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    file = Some(PathBuf::from(OsString::from_vec(
                        args[index].as_encoded_bytes()[12..].to_vec(),
                    )));
                }
            } else if matches!(
                value.as_ref(),
                "--output"
                    | "-o"
                    | "--max-angle"
                    | "--min-line-percent"
                    | "--min-line-pixels"
                    | "--analysis-size"
                    | "--resample"
                    | "--merge-angle"
                    | "--merge-distance-percent"
                    | "--merge-distance-pixels"
                    | "--merge-gap-percent"
                    | "--merge-gap-pixels"
            ) {
                index += 1;
            }
            index += 1;
        }
        (lang, file)
    }

    pub fn load_language(args: &[OsString]) -> Result<I18n, LocalizedMessage> {
        let (locale, file) = Self::bootstrap(args);
        let selected = match (&locale, &file) {
            (Some(locale), _) => locale.clone(),
            (None, Some(path)) => I18n::from_language_file(path)?.selected_locale().to_owned(),
            _ => DEFAULT_LOCALE.into(),
        };
        let mut options = LoadOptions::discovered();
        options.language_file = file;
        let service = I18n::load(&selected, &options)?;
        if !service.has_locale(&selected) {
            return Err(LocalizedMessage::new("i18n.error.unavailable_locale")
                .with_param("locale", selected));
        }
        Ok(service)
    }

    pub fn localized_command(i18n: &I18n) -> clap::Command {
        Self::localized_command_impl(i18n, false)
    }

    fn localized_command_impl(i18n: &I18n, folder_required: bool) -> clap::Command {
        let mut cmd = Self::command()
            .about(i18n.translate("cli.about"))
            .disable_help_flag(true)
            .disable_version_flag(true)
            .help_template(format!(
                "{{about}}\n\n{} {{usage}}\n\n{{all-args}}",
                i18n.translate("cli.usage")
            ))
            .arg(
                clap::Arg::new("help")
                    .short('h')
                    .long("help")
                    .action(clap::ArgAction::Help)
                    .help(i18n.translate("cli.help.help")),
            )
            .arg(
                clap::Arg::new("version")
                    .short('V')
                    .long("version")
                    .action(clap::ArgAction::Version)
                    .help(i18n.translate("cli.help.version")),
            );
        if folder_required {
            cmd = cmd.mut_arg("folder", |arg| arg.required(true));
        }
        let ids: Vec<_> = cmd
            .get_arguments()
            .map(|a| a.get_id().to_string())
            .collect();
        for id in ids {
            let heading = if id == "folder" {
                "cli.arguments"
            } else {
                "cli.options"
            };
            cmd = cmd.mut_arg(&id, |a| a.help_heading(i18n.translate(heading)));
            if id != "help" && id != "version" {
                let defaults = cmd
                    .get_arguments()
                    .find(|a| a.get_id() == &id)
                    .unwrap()
                    .get_default_values()
                    .iter()
                    .map(|v| v.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut help = i18n.translate(&format!("cli.help.{id}"));
                if !defaults.is_empty() {
                    help.push_str(". ");
                    help.push_str(&i18n.translate_message(
                        &LocalizedMessage::new("cli.default").with_param("value", defaults),
                    ));
                }
                cmd = cmd.mut_arg(&id, |a| {
                    let takes_values = a.get_action().takes_values();
                    a.hide_default_value(takes_values).help(help)
                });
            }
        }
        cmd
    }

    /// Parse arguments for a frontend. The command-line executable requires a
    /// folder; the GUI may omit it and open its window instead.
    pub fn parse_localized(
        args: &[OsString],
        i18n: &I18n,
        folder_required: bool,
    ) -> Result<Self, clap::Error> {
        let matches =
            Self::localized_command_impl(i18n, folder_required).try_get_matches_from(args)?;
        Self::from_arg_matches(&matches)
    }

    /// Load the display language for a frontend, or report and exit on failure.
    pub fn language_or_exit(args: &[OsString]) -> I18n {
        let language = Self::load_language(args).unwrap_or_else(|error| {
            let fallback = Self::bootstrap(args)
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
        language
    }

    /// Print a localized parser error and exit with Clap's status code.
    pub fn exit_with_parse_error(error: clap::Error, i18n: &I18n, folder_required: bool) -> ! {
        let text = Self::error_text(&error, i18n, folder_required);
        if error.use_stderr() {
            eprint!("{text}");
        } else {
            print!("{text}");
        }
        std::process::exit(error.exit_code());
    }

    pub fn error_text(error: &clap::Error, i18n: &I18n, folder_required: bool) -> String {
        use clap::error::{ContextKind as C, ErrorKind as E};
        if matches!(error.kind(), E::DisplayHelp | E::DisplayVersion) {
            return error.to_string();
        }
        let get = |kind| error.get(kind).map(ToString::to_string).unwrap_or_default();
        let key = match error.kind() {
            E::UnknownArgument => "cli.error.unknown_argument",
            E::MissingRequiredArgument => "cli.error.missing_required",
            E::InvalidValue if get(C::InvalidValue).is_empty() => "cli.error.missing_value",
            E::InvalidValue | E::ValueValidation => "cli.error.invalid_value",
            E::ArgumentConflict => "cli.error.conflict",
            E::TooManyValues => "cli.error.unexpected_value",
            E::TooFewValues | E::WrongNumberOfValues => "cli.error.missing_value",
            _ => "cli.error.invalid_arguments",
        };
        let custom = get(C::Custom);
        let mut message = if !custom.is_empty() {
            custom
        } else {
            i18n.translate_message(
                &LocalizedMessage::new(key)
                    .with_param("argument", get(C::InvalidArg))
                    .with_param("value", get(C::InvalidValue))
                    .with_param("other", get(C::PriorArg)),
            )
        };
        for (kind, key) in [
            (C::SuggestedArg, "cli.suggestion"),
            (C::ValidValue, "cli.valid_values"),
        ] {
            let value = get(kind);
            if !value.is_empty() {
                message.push('\n');
                message.push_str(
                    &i18n.translate_message(&LocalizedMessage::new(key).with_param("value", value)),
                );
            }
        }
        format!(
            "{}: {}\n\n{} {}\n\n{}\n",
            i18n.translate("app.error_prefix"),
            message,
            i18n.translate("cli.usage"),
            Self::localized_command_impl(i18n, folder_required)
                .render_usage()
                .to_string()
                .trim_start_matches("Usage: "),
            i18n.translate("cli.help_hint")
        )
    }
    pub fn batch_options(
        &self,
        settings: DetectionSettings,
    ) -> anyhow::Result<Option<BatchOptions>> {
        self.folder
            .as_ref()
            .map(|source| {
                Ok(BatchOptions {
                    source: source.clone(),
                    destination: self.output.clone(),
                    recursive: self.recursive,
                    settings,
                    allow_large_images: self.allow_large_images,
                })
            })
            .transpose()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_dispatch() {
        let cli = Cli::try_parse_from(["scan-angle"]).unwrap();
        let s = cli.settings().unwrap();
        assert_eq!(s.analysis_size, 2400);
        assert!(cli.batch_options(s).unwrap().is_none());
    }
    #[test]
    fn json_requires_folder() {
        let cli = Cli::try_parse_from(["scan-angle", "input", "--json"]).unwrap();
        assert!(cli.json);
        assert!(Cli::try_parse_from(["scan-angle", "--json"]).is_err());
    }
    #[test]
    fn folder_without_output_uses_console_mode() {
        let cli = Cli::try_parse_from(["scan-angle", "input"]).unwrap();
        let options = cli.batch_options(cli.settings().unwrap()).unwrap().unwrap();
        assert!(options.destination.is_none());
        let cli = Cli::try_parse_from(["scan-angle", "input", "-o", "results"]).unwrap();
        let options = cli.batch_options(cli.settings().unwrap()).unwrap().unwrap();
        assert_eq!(options.destination, Some("results".into()));
    }
    #[test]
    fn invalid_settings() {
        for (flag, value) in [
            ("--max-angle", "0"),
            ("--max-angle", "31"),
            ("--min-line-percent", "NaN"),
            ("--min-line-pixels", "-1"),
            ("--merge-angle", "45"),
            ("--merge-gap-pixels", "inf"),
            ("--resample", "bad"),
        ] {
            let cli = Cli::try_parse_from(["scan-angle", flag, value]).unwrap();
            let error = cli.settings().unwrap_err();
            assert_eq!(error.exit_code(), 2);
            assert!(error.to_string().contains(flag));
        }
    }
    #[test]
    fn all_options_forwarded() {
        let cli = Cli::try_parse_from([
            "scan-angle",
            "--max-angle",
            "20",
            "--min-line-percent",
            "25",
            "--min-line-pixels",
            "75",
            "--analysis-size",
            "1600",
            "--resample",
            " HAMMING ",
            "--merge-angle",
            "1.2",
            "--merge-distance-percent",
            "0",
            "--merge-distance-pixels",
            "0",
            "--merge-gap-percent",
            "0",
            "--merge-gap-pixels",
            "0",
            "-r",
            "-o",
            "results",
            "--allow-large-images",
        ])
        .unwrap();
        let s = cli.settings().unwrap();
        assert_eq!(s.resample, "hamming");
        assert_eq!(s.max_angle, 20.);
        assert_eq!(s.min_line_percent, 25.);
        assert_eq!(s.min_line_pixels, 75.);
        assert_eq!(s.analysis_size, 1600);
        assert_eq!(s.merge_angle, 1.2);
        assert_eq!(s.merge_tolerances(1000, 1000), (0., 0.));
        assert!(cli.recursive && cli.allow_large_images);
        assert_eq!(cli.output, Some("results".into()));
    }
}
