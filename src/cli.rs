use crate::{batch::BatchOptions, detection::DetectionSettings};
use clap::{CommandFactory, Parser};
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(version, about = "扫描偏角检测：正数表示顺时针，输入图不修改。")]
pub struct Cli {
    pub folder: Option<PathBuf>,
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
    #[arg(long, help = "允许处理超过默认像素保护上限的可信本地扫描图")]
    pub allow_large_images: bool,
    #[arg(
        long,
        requires = "folder",
        help = "以 JSON 格式输出检测结果，结构与 details.json 相同"
    )]
    pub json: bool,
}
impl Cli {
    pub fn settings(&self) -> Result<DetectionSettings, clap::Error> {
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
                anyhow::bail!("max_angle 必须在 1～30 之间")
            }
        });
        if let Err(e) = result {
            let mut message = e.to_string();
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
            return Err(Self::command().error(clap::error::ErrorKind::ValueValidation, message));
        }
        Ok(s)
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
