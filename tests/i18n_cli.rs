use std::{
    path::Path,
    process::{Command, Output},
};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scan-angle"))
        .args(args)
        .output()
        .unwrap()
}
fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}
fn pack(locale: &str, messages: &str) -> String {
    let fallback = if locale == "en-US" { "zh-CN" } else { "en-US" };
    format!(
        "schema_version = 1\nlocale = {locale:?}\nname = \"Test language\"\nfallback = {fallback:?}\n[messages]\n{messages}\n"
    )
}
fn blank(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    image::RgbImage::from_pixel(64, 64, image::Rgb([255, 255, 255]))
        .save(dir.join("blank.png"))
        .unwrap();
}

#[test]
fn help_and_parser_errors_follow_selected_language() {
    let english = run(&["--lang", "en-US", "--help"]);
    assert!(english.status.success(), "{}", err(&english));
    assert!(out(&english).contains("Usage:"));
    assert!(out(&english).contains("Display language"));
    assert!(!out(&english).contains("扫描"));
    let chinese = run(&["--help"]);
    assert!(chinese.status.success());
    assert!(out(&chinese).contains("用法："));
    for (args, token) in [
        (vec!["--lang=en-US", "--max-angle", "NaN"], "--max-angle"),
        (
            vec!["--analysis-size", "oops", "--lang=en-US"],
            "--analysis-size",
        ),
        (
            vec!["--lang", "en-US", "--unknown-option"],
            "--unknown-option",
        ),
        (vec!["--lang", "en-US", "--json"], "FOLDER"),
        (
            vec!["--lang", "en-US", "--analysis-size"],
            "--analysis-size",
        ),
    ] {
        let result = run(&args);
        assert_eq!(result.status.code(), Some(2), "{args:?}: {}", err(&result));
        assert!(err(&result).contains(token), "{args:?}: {}", err(&result));
        assert!(err(&result).contains("Error:"), "{}", err(&result));
        assert!(!err(&result).contains("必须"), "{}", err(&result));
        assert!(result.stdout.is_empty());
    }
}

#[test]
fn explicit_file_selects_new_language_and_rejects_mismatch_or_corruption() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("custom language.toml");
    std::fs::write(
        &file,
        pack(
            "fr-FR",
            "\"cli.about\" = \"Analyse des images\"\n\"cli.help.help\" = \"Aide\"",
        ),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_scan-angle"))
        .arg("--lang-file")
        .arg(&file)
        .arg("--help")
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", err(&result));
    assert!(out(&result).contains("Analyse des images"));
    assert!(out(&result).contains("Aide"));
    assert!(out(&result).contains("Output directory")); // fallback
    let result = Command::new(env!("CARGO_BIN_EXE_scan-angle"))
        .args(["--lang", "en-US", "--lang-file"])
        .arg(&file)
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(err(&result).contains("fr-FR") && err(&result).contains("en-US"));
    std::fs::write(&file, "not valid = [").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_scan-angle"))
        .args(["--lang", "en-US", "--lang-file"])
        .arg(&file)
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert!(err(&result).contains("Language file"), "{}", err(&result));
}

#[test]
fn interface_language_does_not_change_json_or_csv_exports() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input");
    blank(&input);
    std::fs::write(input.join("broken.png"), b"not an image").unwrap();
    let mut outputs = Vec::new();
    for lang in ["zh-CN", "en-US"] {
        let destination = temp.path().join(lang);
        let result = Command::new(env!("CARGO_BIN_EXE_scan-angle"))
            .arg(&input)
            .args(["--lang", lang, "--json", "--output"])
            .arg(&destination)
            .output()
            .unwrap();
        assert!(result.status.success(), "{}", err(&result));
        let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value[0]["status"], "需人工确认");
        assert_eq!(value[1]["status"], "读取失败");
        outputs.push((
            value,
            std::fs::read(destination.join("angles.csv")).unwrap(),
            std::fs::read(destination.join("details.json")).unwrap(),
        ));
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn executable_adjacent_packs_work_from_another_directory_and_warn_on_stderr() {
    let temp = tempfile::tempdir().unwrap();
    let exe = temp
        .path()
        .join(if cfg!(windows) { "app.exe" } else { "app" });
    std::fs::copy(env!("CARGO_BIN_EXE_scan-angle"), &exe).unwrap();
    let locales = temp.path().join("locales");
    std::fs::create_dir(&locales).unwrap();
    std::fs::write(
        locales.join("fr-FR.toml"),
        pack(
            "fr-FR",
            "\"report.header.file\" = \"Fichier\"\n\"status.needs_review\" = \"A verifier\"",
        ),
    )
    .unwrap();
    std::fs::write(
        locales.join("en-US.toml"),
        pack("en-US", "\"report.header.file\" = \"Custom file\""),
    )
    .unwrap();
    std::fs::write(locales.join("broken.toml"), "bad = [").unwrap();
    let input = temp.path().join("input");
    blank(&input);
    let cwd = temp.path().join("cwd");
    std::fs::create_dir(&cwd).unwrap();
    let result = Command::new(&exe)
        .current_dir(&cwd)
        .arg(&input)
        .args(["--lang", "fr-FR"])
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", err(&result));
    assert!(out(&result).contains("Fichier") && out(&result).contains("A verifier"));
    assert!(err(&result).contains("broken.toml"));
    let result = Command::new(&exe)
        .current_dir(&cwd)
        .arg(&input)
        .args(["--lang", "en-US"])
        .output()
        .unwrap();
    assert!(out(&result).contains("Custom file"));
    let result = Command::new(&exe)
        .current_dir(&cwd)
        .arg(&input)
        .args(["--lang", "en-US", "--json"])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(serde_json::from_slice::<serde_json::Value>(&result.stdout).is_ok());
    assert!(err(&result).contains("broken.toml"));
}

#[test]
fn bootstrap_respects_option_values_and_end_of_options() {
    use scan_angle::cli::Cli;
    let args = |a: &[&str]| a.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
    assert_eq!(
        Cli::bootstrap(&args(&["scan-angle", "--", "--lang", "en-US"])),
        (None, None)
    );
    assert_eq!(
        Cli::bootstrap(&args(&["scan-angle", "--output", "--lang", "en-US"])),
        (None, None)
    );
    assert_eq!(
        Cli::bootstrap(&args(&["scan-angle", "--resample", "--lang=en-US"])),
        (None, None)
    );
    assert_eq!(
        Cli::bootstrap(&args(&[
            "scan-angle",
            "--lang-file=translations/翻译 file.toml",
            "--lang=en-US"
        ])),
        (
            Some("en-US".into()),
            Some("translations/翻译 file.toml".into())
        )
    );
    let result = run(&["--lang", "xx-XX", "--help"]);
    assert_eq!(result.status.code(), Some(2));
    assert!(err(&result).contains("xx-XX"));
}

#[test]
fn missing_and_unexpected_values_have_distinct_localized_diagnostics() {
    for flag in ["--output", "--max-angle", "--lang-file"] {
        let result = run(&["--lang", "en-US", flag]);
        assert_eq!(result.status.code(), Some(2));
        assert!(
            err(&result).contains("A value is required"),
            "{}",
            err(&result)
        );
    }
    let result = run(&["--lang", "en-US", "--recursive=yes"]);
    assert_eq!(result.status.code(), Some(2));
    assert!(
        err(&result).contains("Unexpected value"),
        "{}",
        err(&result)
    );
    assert!(!err(&result).contains("A value is required"));
    let result = run(&["--lang", "en-us", "--help"]);
    assert!(result.status.success(), "{}", err(&result));
    assert!(out(&result).contains("Default: 15"));
    let result = run(&["--lang", "ZH-cn", "--help"]);
    assert!(result.status.success(), "{}", err(&result));
    assert!(out(&result).contains("默认值：15"));
    assert!(!out(&result).contains("[default:"));
}
