use scan_angle::{
    batch::{self, BatchOptions, Progress},
    detection::{DetectionSettings, estimate_segments},
    imaging,
};
use serde_json::Value;
use std::{fs, path::PathBuf};
fn fixture(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    assert!(
        path.exists(),
        "Missing fixture {}. Run: uv run --locked scripts/generate-fixtures.py",
        path.display()
    );
    path
}
fn assert_json_close(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => assert!(
            (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 0.00011,
            "{path}: {a} != {b}"
        ),
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                assert_json_close(a, b, &format!("{path}[{i}]"));
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(a.len(), b.len(), "{path}");
            for (k, b) in b {
                assert_json_close(&a[k], b, &format!("{path}.{k}"));
            }
        }
        _ => assert_eq!(a, b, "{path}"),
    }
}
#[test]
fn python_pipeline_goldens() {
    let cases: Value =
        serde_json::from_slice(&fs::read(fixture("pipeline_golden.json")).unwrap()).unwrap();
    for (i, c) in cases.as_array().unwrap().iter().enumerate() {
        let segments = serde_json::from_value::<Vec<[f64; 4]>>(c["segments"].clone()).unwrap();
        let settings = serde_json::from_value(c["settings"].clone()).unwrap();
        let (r, lines) = estimate_segments(
            &segments,
            c["height"].as_u64().unwrap() as u32,
            c["width"].as_u64().unwrap() as u32,
            &settings,
        )
        .unwrap();
        assert_json_close(
            &serde_json::to_value(r).unwrap(),
            &c["expected"],
            &format!("case {i}"),
        );
        assert_json_close(
            &serde_json::to_value(lines).unwrap(),
            &c["lines"],
            &format!("lines {i}"),
        );
    }
}
#[test]
fn python_image_angles_and_classification() {
    let cases: Value =
        serde_json::from_slice(&fs::read(fixture("images_golden.json")).unwrap()).unwrap();
    for c in cases.as_array().unwrap() {
        let name = c["file"].as_str().unwrap();
        let image = image::open(fixture(name)).unwrap();
        let (r, _, _) = imaging::estimate(&image, &DetectionSettings::default()).unwrap();
        assert_eq!(r.status, c["status"].as_str().unwrap(), "{name}: {r:?}");
        match (c["angle"].as_f64(), r.angle) {
            (Some(w), Some(g)) => assert!((w - g).abs() <= 0.12, "{name}: expected {w}, got {g}"),
            (None, None) => {}
            _ => panic!("{name}: expected {:?}, got {:?}", c["angle"], r.angle),
        }
    }
}
#[test]
fn format_frames_and_exif_match_python() {
    let cases: Value =
        serde_json::from_slice(&fs::read(fixture("formats/manifest.json")).unwrap()).unwrap();
    for c in cases.as_array().unwrap() {
        let name = c["file"].as_str().unwrap();
        let mut pages = Vec::new();
        imaging::visit_frames(
            &fixture(&format!("formats/{name}")),
            false,
            &mut |page, image| {
                assert_eq!(page, pages.len() + 1);
                pages.push(image.unwrap());
            },
        )
        .unwrap();
        let expected = c["pages"].as_array().unwrap();
        assert_eq!(pages.len(), expected.len(), "{name}");
        for (page, e) in pages.iter().zip(expected) {
            let im = page.to_rgb8();
            assert_eq!(
                (im.width(), im.height()),
                (
                    e["width"].as_u64().unwrap() as u32,
                    e["height"].as_u64().unwrap() as u32
                ),
                "{name}"
            );
            for (i, (x, y)) in [
                (10, 10),
                (im.width() - 10, 10),
                (10, im.height() - 10),
                (im.width() - 10, im.height() - 10),
            ]
            .into_iter()
            .enumerate()
            {
                for ch in 0..3 {
                    let actual = im.get_pixel(x, y)[ch] as i64;
                    let expected = e["samples"][i][ch].as_i64().unwrap();
                    assert!(
                        (actual - expected).abs() <= 3,
                        "{name} sample {i} channel {ch}: {actual} != {expected}"
                    );
                }
            }
        }
    }
}
#[test]
fn batch_outputs_failures_progress_and_exclusion() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("input");
    let dest = source.join("results");
    fs::create_dir_all(&dest).unwrap();
    fs::copy(fixture("formats/tiff_L.tif"), source.join("多页.tif")).unwrap();
    fs::write(source.join("broken.png"), b"broken").unwrap();
    fs::copy(fixture("angle_0.png"), dest.join("excluded.png")).unwrap();
    fs::create_dir(source.join("nested")).unwrap();
    fs::copy(fixture("formats/color.bmp"), source.join("nested/a,b.bmp")).unwrap();
    let options = BatchOptions {
        source: source.clone(),
        destination: Some(dest.clone()),
        recursive: true,
        settings: DetectionSettings::default(),
        allow_large_images: false,
    };
    let mut counts = Vec::new();
    let rows = batch::run_batch(&options, |p| {
        if let Progress::Files(n, t) = p {
            counts.push((n, t));
        }
    })
    .unwrap();
    assert_eq!(counts, vec![(0, 3), (1, 3), (2, 3), (3, 3)]);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows.iter().filter(|r| r.status == "读取失败").count(), 1);
    assert!(!rows.iter().any(|r| r.file.contains("excluded")));
    let bytes = fs::read(dest.join("angles.csv")).unwrap();
    assert_eq!(&bytes[..3], &[0xef, 0xbb, 0xbf]);
    assert!(bytes.windows(2).any(|w| w == b"\r\n"));
    let mut reader = csv::Reader::from_reader(&bytes[3..]);
    assert_eq!(
        reader.headers().unwrap().iter().collect::<Vec<_>>(),
        batch::FIELDS
    );
    assert_eq!(reader.records().count(), 4);
    let details: Value =
        serde_json::from_slice(&fs::read(dest.join("details.json")).unwrap()).unwrap();
    assert_eq!(details.as_array().unwrap().len(), 4);
    for d in details.as_array().unwrap() {
        assert!(d["settings"].is_object());
        assert!(d["lines"].is_array());
    }
    for row in rows {
        if row.status != "读取失败" {
            assert!(dest.join(row.preview).is_file());
        }
    }
    let mut bad = options.clone();
    bad.destination = Some(source);
    assert!(batch::run_batch(&bad, |_| {}).is_err());
    bad.destination = Some(temp.path().to_owned());
    let error = batch::run_batch(&bad, |_| {}).unwrap_err().to_string();
    assert!(error.contains("上级目录"), "{error}");
    assert!(!temp.path().join("angles.csv").exists());
    assert!(!temp.path().join("previews").exists());
    let not_created = temp.path().join("not-created");
    bad.destination = Some(not_created.clone());
    bad.settings.min_line_pixels = f64::NAN;
    assert!(batch::run_batch(&bad, |_| {}).is_err());
    assert!(!not_created.exists());
}

#[test]
fn console_mode_writes_no_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("input");
    fs::create_dir_all(&source).unwrap();
    fs::copy(fixture("angle_0.png"), source.join("angle.png")).unwrap();
    fs::copy(fixture("blank.png"), source.join("blank.png")).unwrap();
    fs::write(source.join("broken.png"), b"broken").unwrap();
    let options = BatchOptions {
        source: source.clone(),
        destination: None,
        recursive: false,
        settings: DetectionSettings::default(),
        allow_large_images: false,
    };
    let rows = batch::run_batch(&options, |_| {}).unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.preview.is_empty()));
    assert!(rows.iter().any(|r| r.status == "需人工确认"));
    assert!(rows.iter().any(|r| r.status == "读取失败"));
    let entries: Vec<_> = fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries, vec![source]);
    assert!(!temp.path().join("input_角度结果").exists());
}
#[test]
fn all_filters_and_no_upscaling() {
    let im = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(800, 600, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 100])
    }));
    for filter in scan_angle::detection::RESAMPLES {
        let small = scan_angle::resample::thumbnail(&im, 400, filter);
        assert_eq!(small.dimensions(), (400, 300));
        let same = scan_angle::resample::thumbnail(&im, 1000, filter);
        assert_eq!(same.dimensions(), (800, 600));
    }
}

#[test]
fn resize_pixels_match_python() {
    let cases: Value =
        serde_json::from_slice(&fs::read(fixture("resize/manifest.json")).unwrap()).unwrap();
    for case in cases.as_array().unwrap() {
        let source = image::open(fixture(&format!(
            "resize/{}",
            case["source"].as_str().unwrap()
        )))
        .unwrap();
        let expected = image::open(fixture(&format!(
            "resize/{}",
            case["expected"].as_str().unwrap()
        )))
        .unwrap()
        .to_rgb8();
        let actual =
            scan_angle::resample::thumbnail(&source, 256, case["filter"].as_str().unwrap());
        assert_eq!(actual.dimensions(), expected.dimensions());
        let max = actual
            .as_raw()
            .iter()
            .zip(expected.as_raw())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(
            max <= 1,
            "{} {}: maximum channel difference {max}",
            case["source"],
            case["filter"]
        );
    }
}

#[test]
fn executable_cli_contract() {
    use std::process::Command;
    let binary = env!("CARGO_BIN_EXE_scan-angle");
    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--merge-gap-pixels"));
    let invalid = Command::new(binary)
        .args(["--max-angle", "NaN"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("--max-angle"));
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("output");
    let batch = Command::new(binary)
        .arg(fixture("formats"))
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        batch.status.success(),
        "{}",
        String::from_utf8_lossy(&batch.stderr)
    );
    let details: Value =
        serde_json::from_slice(&fs::read(output.join("details.json")).unwrap()).unwrap();
    assert!(details.as_array().unwrap().len() >= 30);
    assert!(
        details
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["status"] != "读取失败")
    );
}

#[test]
fn executable_console_only_without_output() {
    use std::process::Command;
    let binary = env!("CARGO_BIN_EXE_scan-angle");
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    fs::create_dir_all(&source).unwrap();
    fs::copy(fixture("angle_0.png"), source.join("angle.png")).unwrap();
    fs::copy(fixture("blank.png"), source.join("blank.png")).unwrap();
    let run = Command::new(binary).arg(&source).output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    let header = stdout.lines().next().unwrap_or_default();
    for column in ["文件", "页", "转正角度（°）", "检测 / 合并 / 入选", "状态"] {
        assert!(header.contains(column), "{header}");
    }
    assert_eq!(stdout.matches("转正角度（°）").count(), 1, "{stdout}");
    assert!(stdout.contains("angle.png"), "{stdout}");
    assert!(stdout.contains("-0.000"), "{stdout}");
    assert!(stdout.contains("0 / 0 / 0"), "{stdout}");
    assert!(stdout.contains("需人工确认"), "{stdout}");
    assert!(stdout.contains("可信"), "{stdout}");
    assert!(!stdout.contains('\t'), "{stdout}");
    assert!(!stdout.contains("偏差"), "{stdout}");
    assert!(!stdout.contains("第 1 页"), "{stdout}");
    assert!(!stdout.contains("未指定 --output"), "{stdout}");
    assert!(!stdout.contains("没有足够的长直线"), "{stdout}");
    let entries: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries, vec![source]);
    assert!(!dir.path().join("input_角度结果").exists());
}
