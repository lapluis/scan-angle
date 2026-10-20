use crate::{detection::DetectionSettings, imaging};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
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
    pub status: String,
    pub count: usize,
    pub spread: Option<f64>,
    pub reason: String,
    pub preview: String,
    pub fragment_count: Option<usize>,
    pub merged_count: Option<usize>,
}
#[derive(Debug, Clone)]
pub enum Progress {
    Log(String),
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
pub fn default_output(source: &Path) -> Result<PathBuf> {
    let source = dunce::canonicalize(source).context("输入文件夹不存在")?;
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
    error: impl std::fmt::Display,
    s: &DetectionSettings,
) -> (Row, Value) {
    let error = error.to_string();
    (
        Row {
            file: file.into(),
            page,
            angle: None,
            correction: None,
            status: "读取失败".into(),
            count: 0,
            spread: None,
            reason: error.clone(),
            preview: String::new(),
            fragment_count: None,
            merged_count: None,
        },
        json!({"file":file,"page":page,"angle":null,"correction":null,"status":"读取失败","count":0,"spread":null,"reason":error,"fragment_count":0,"merged_count":0,"thresholds_px":null,"merge":null,"merged_lines":[],"lines":[],"settings":s,"error":error}),
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
        status: result.status.clone(),
        count: result.count,
        spread: result.spread,
        reason: result.reason.clone(),
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
    let source = dunce::canonicalize(&options.source).context("输入文件夹不存在")?;
    if !source.is_dir() {
        bail!("输入文件夹不存在");
    }
    let destination = match &options.destination {
        Some(destination) => {
            // Canonicalize existing aliases before creating output, including junctions.
            if let Ok(dest) = dunce::canonicalize(destination)
                && same_path(&source, &dest)
            {
                bail!("输出文件夹必须与输入文件夹不同");
            }
            fs::create_dir_all(destination)?;
            let destination = dunce::canonicalize(destination)?;
            if same_path(&source, &destination) {
                bail!("输出文件夹必须与输入文件夹不同");
            }
            if inside(&source, &destination) {
                bail!("输出文件夹不能是输入文件夹的上级目录");
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
        bail!("文件夹中没有支持的图片");
    }
    let mut rows = Vec::new();
    let mut details = Vec::new();
    progress(Progress::Files(0, files.len()));
    for (index, path) in files.iter().enumerate() {
        let file = path.strip_prefix(&source)?.to_string_lossy().into_owned();
        progress(Progress::Log(format!(
            "[{}/{}] {file}",
            index + 1,
            files.len()
        )));
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
                r.status.clone(),
                r.count.to_string(),
                r.spread.map_or(String::new(), float_text),
                r.reason.clone(),
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
