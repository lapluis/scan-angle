use crate::{
    detection::{DetectionReason, DetectionSettings, Status},
    i18n::{I18n, LocalizedMessage},
    imaging,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    fmt,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const FIELDS: [&str; 9] = [
    "文件",
    "页码",
    "偏差角度_度_顺时针为正",
    "转正角度_度_顺时针为正",
    "状态",
    "合并直线数",
    "角度离散_度",
    "说明",
    "预览",
];
pub const EXTENSIONS: [&str; 7] = ["tif", "tiff", "png", "jpg", "jpeg", "bmp", "webp"];
#[derive(Debug, Clone)]
pub struct Row {
    pub file: String,
    pub page: Option<usize>,
    pub angle: Option<f64>,
    pub correction: Option<f64>,
    pub status: Status,
    pub count: usize,
    pub spread: Option<f64>,
    pub reason: RowReason,
    pub preview: String,
    pub fragment_count: Option<usize>,
    pub merged_count: Option<usize>,
}

/// Explanation attached to a [`Row`].
///
/// Detection outcomes keep their typed [`DetectionReason`]. Read or processing
/// failures keep the full [`anyhow::Error`] behind an [`Arc`] so rows remain
/// cloneable and can be re-rendered in another language without losing the
/// error chain.
#[derive(Debug, Clone)]
pub enum RowReason {
    Detection(DetectionReason),
    Error(Arc<anyhow::Error>),
}

impl RowReason {
    /// Render in the language selected by `i18n`.
    pub fn render(&self, i18n: &I18n) -> String {
        match self {
            Self::Detection(reason) => reason.render(i18n),
            Self::Error(error) => i18n.render_anyhow(error),
        }
    }
}

impl fmt::Display for RowReason {
    /// Legacy Chinese default, matching the text exported before localization.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Detection(reason) => reason.fmt(formatter),
            Self::Error(error) => error.fmt(formatter),
        }
    }
}

impl From<DetectionReason> for RowReason {
    fn from(reason: DetectionReason) -> Self {
        Self::Detection(reason)
    }
}

#[derive(Debug, Clone)]
pub enum Progress {
    Log(LocalizedMessage),
    Files(usize, usize),
}
#[derive(Debug, Clone)]
pub struct BatchOptions {
    pub source: PathBuf,
    /// `None` 表示只在命令行报告结果，不写入任何文件。
    pub destination: Option<PathBuf>,
    pub recursive: bool,
    pub settings: DetectionSettings,
    pub allow_large_images: bool,
}
fn canonicalize_input(path: &Path) -> Result<PathBuf> {
    dunce::canonicalize(path).map_err(|error| {
        LocalizedMessage::new("batch.error.input_missing").context(anyhow::Error::new(error))
    })
}

pub fn default_output(source: &Path) -> Result<PathBuf> {
    let source = canonicalize_input(source)?;
    let name = source.file_name().unwrap_or_default().to_string_lossy();
    Ok(source
        .parent()
        .unwrap_or(&source)
        .join(format!("{name}_角度结果")))
}
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}
fn inside(path: &Path, dir: &Path) -> bool {
    if path.starts_with(dir) {
        return true;
    }
    if cfg!(windows) {
        let p = path.to_string_lossy().to_lowercase();
        let mut d = dir.to_string_lossy().to_lowercase();
        d.push(std::path::MAIN_SEPARATOR);
        return p.starts_with(&d);
    }
    false
}
fn failure(
    file: &str,
    page: Option<usize>,
    error: anyhow::Error,
    s: &DetectionSettings,
) -> (Row, Value) {
    let reason = RowReason::Error(Arc::new(error));
    // JSON and CSV keep the legacy Display text byte-for-byte.
    let error = reason.to_string();
    (
        Row {
            file: file.into(),
            page,
            angle: None,
            correction: None,
            status: Status::ReadFailed,
            count: 0,
            spread: None,
            reason,
            preview: String::new(),
            fragment_count: None,
            merged_count: None,
        },
        json!({"file":file,"page":page,"angle":null,"correction":null,"status":Status::ReadFailed,"count":0,"spread":null,"reason":error.clone(),"fragment_count":0,"merged_count":0,"thresholds_px":null,"merge":null,"merged_lines":[],"lines":[],"settings":s,"error":error}),
    )
}
fn process_page(
    file: &str,
    page: usize,
    path: &Path,
    index: usize,
    image: image::DynamicImage,
    destination: Option<&Path>,
    s: &DetectionSettings,
) -> Result<(Row, Value)> {
    let (result, small, lines) = imaging::estimate(&image, s)?;
    let preview = if let Some(destination) = destination {
        let preview = imaging::draw_preview(&small, &result.merged_lines, &lines);
        let name = format!(
            "{index:05}_{}_p{page}.jpg",
            path.file_stem().unwrap_or_default().to_string_lossy()
        );
        let writer = BufWriter::new(File::create(destination.join("previews").join(&name))?);
        image::codecs::jpeg::JpegEncoder::new_with_quality(writer, 90).encode_image(&preview)?;
        format!("previews/{name}")
    } else {
        String::new()
    };
    let row = Row {
        file: file.into(),
        page: Some(page),
        angle: result.angle,
        correction: result.correction,
        status: result.status,
        count: result.count,
        spread: result.spread,
        reason: result.reason.into(),
        preview,
        fragment_count: Some(result.fragment_count),
        merged_count: Some(result.merged_count),
    };
    let mut detail = serde_json::to_value(result)?;
    let obj = detail.as_object_mut().unwrap();
    obj.insert("file".into(), json!(file));
    obj.insert("page".into(), json!(page));
    obj.insert("settings".into(), json!(s));
    obj.insert("lines".into(), json!(lines));
    Ok((row, detail))
}
pub fn run_batch(options: &BatchOptions, progress: impl FnMut(Progress)) -> Result<Vec<Row>> {
    let collect_details = options.destination.is_some();
    Ok(execute(options, collect_details, progress)?.0)
}

pub fn run_batch_details(
    options: &BatchOptions,
    progress: impl FnMut(Progress),
) -> Result<(Vec<Row>, Vec<Value>)> {
    execute(options, true, progress)
}

fn execute(
    options: &BatchOptions,
    collect_details: bool,
    mut progress: impl FnMut(Progress),
) -> Result<(Vec<Row>, Vec<Value>)> {
    options.settings.validate()?;
    let source = canonicalize_input(&options.source)?;
    if !source.is_dir() {
        bail!(LocalizedMessage::new("batch.error.input_missing"));
    }
    let destination = match &options.destination {
        Some(destination) => {
            // Canonicalize existing aliases before creating output, including junctions.
            if let Ok(dest) = dunce::canonicalize(destination)
                && same_path(&source, &dest)
            {
                bail!(LocalizedMessage::new("batch.error.output_same_as_input"));
            }
            fs::create_dir_all(destination)?;
            let destination = dunce::canonicalize(destination)?;
            if same_path(&source, &destination) {
                bail!(LocalizedMessage::new("batch.error.output_same_as_input"));
            }
            if inside(&source, &destination) {
                bail!(LocalizedMessage::new("batch.error.output_parent_of_input"));
            }
            fs::create_dir_all(destination.join("previews"))?;
            Some(destination)
        }
        None => None,
    };
    let depth = if options.recursive { usize::MAX } else { 1 };
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(&source)
        .max_depth(depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            destination
                .as_deref()
                .is_none_or(|destination| !inside(e.path(), destination))
        })
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if EXTENSIONS.contains(&ext.as_str()) {
            files.push(path.to_owned());
        }
    }
    files.sort_by_cached_key(|p| {
        if cfg!(windows) {
            p.to_string_lossy().to_lowercase()
        } else {
            p.to_string_lossy().into_owned()
        }
    });
    if files.is_empty() {
        bail!(LocalizedMessage::new("batch.error.no_supported_images"));
    }
    let mut rows = Vec::new();
    let mut details = Vec::new();
    progress(Progress::Files(0, files.len()));
    for (index, path) in files.iter().enumerate() {
        let file = path.strip_prefix(&source)?.to_string_lossy().into_owned();
        progress(Progress::Log(
            LocalizedMessage::new("batch.progress.file")
                .with_param("current", index + 1)
                .with_param("total", files.len())
                .with_param("file", &file),
        ));
        let result = imaging::visit_frames(path, options.allow_large_images, &mut |page, image| {
            let item = image.and_then(|image| {
                process_page(
                    &file,
                    page,
                    path,
                    index + 1,
                    image,
                    destination.as_deref(),
                    &options.settings,
                )
            });
            let (row, detail) =
                item.unwrap_or_else(|error| failure(&file, Some(page), error, &options.settings));
            rows.push(row);
            if collect_details {
                details.push(detail);
            }
        });
        if let Err(error) = result {
            let (row, detail) = failure(&file, None, error, &options.settings);
            rows.push(row);
            if collect_details {
                details.push(detail);
            }
        }
        progress(Progress::Files(index + 1, files.len()));
    }
    if let Some(destination) = &destination {
        let mut file = BufWriter::new(File::create(destination.join("angles.csv"))?);
        file.write_all(&[0xef, 0xbb, 0xbf])?;
        let mut writer = csv::WriterBuilder::new()
            .terminator(csv::Terminator::CRLF)
            .from_writer(file);
        writer.write_record(FIELDS)?;
        for r in &rows {
            writer.write_record([
                r.file.clone(),
                r.page.map_or(String::new(), |x| x.to_string()),
                r.angle.map_or(String::new(), float_text),
                r.correction.map_or(String::new(), float_text),
                r.status.to_string(),
                r.count.to_string(),
                r.spread.map_or(String::new(), float_text),
                r.reason.to_string(),
                r.preview.clone(),
            ])?;
        }
        writer.flush()?;
        let mut writer = BufWriter::new(File::create(destination.join("details.json"))?);
        serde_json::to_writer_pretty(&mut writer, &details)?;
        writer.flush()?;
    }
    Ok((rows, details))
}
fn float_text(n: f64) -> String {
    if n.fract() == 0. {
        format!("{n:.1}")
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_reason_keeps_legacy_text_and_renders_translations() {
        let reason = RowReason::from(DetectionReason::NearLimit);
        assert_eq!(reason.to_string(), "角度接近检测范围上限；建议扩大范围复核");
        let english = I18n::default().select("en-US").unwrap();
        assert_eq!(
            reason.render(&english),
            "The angle is close to the detection limit; consider widening the range"
        );

        let error = RowReason::Error(Arc::new(anyhow::anyhow!("disk unavailable")));
        assert_eq!(error.to_string(), "disk unavailable");
        assert_eq!(error.render(&english), "disk unavailable");
    }

    #[test]
    fn localized_error_keeps_os_cause_and_legacy_display() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("does-not-exist");
        let error = canonicalize_input(&missing).unwrap_err();
        assert_eq!(error.to_string(), "输入文件夹不存在");
        assert!(error.chain().count() > 1, "{error:#}");

        let english = I18n::default().select("en-US").unwrap();
        let rendered = english.render_anyhow(&error);
        assert!(
            rendered.starts_with("Input folder does not exist: "),
            "{rendered}"
        );
        assert!(rendered.len() > "Input folder does not exist: ".len());
    }

    #[test]
    fn progress_log_uses_structured_message() {
        let message = LocalizedMessage::new("batch.progress.file")
            .with_param("current", 2)
            .with_param("total", 5)
            .with_param("file", "scan.png");
        assert_eq!(message.to_string(), "[2/5] scan.png");
    }
}
