use crate::{
    batch::{self, BatchOptions, Progress, Row},
    cli::Cli,
    detection::{DetectionSettings, RESAMPLES},
};
use crate::{
    i18n::{I18n, LocalizedMessage},
    preferences,
};
use anyhow::Result;
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    time::Duration,
};

const LABELS: [&str; 9] = [
    "settings.label.max_angle",
    "settings.label.min_line_percent",
    "settings.label.min_line_pixels",
    "settings.label.analysis_size",
    "settings.label.merge_angle",
    "settings.label.merge_distance_percent",
    "settings.label.merge_distance_pixels",
    "settings.label.merge_gap_percent",
    "settings.label.merge_gap_pixels",
];
#[derive(Clone)]
pub struct Form {
    pub source: String,
    pub destination: String,
    pub numbers: [String; 9],
    pub resample: String,
    pub recursive: bool,
    pub allow_large: bool,
}
impl Form {
    pub fn new(cli: &Cli, s: &DetectionSettings) -> Self {
        Self {
            source: cli
                .folder
                .as_ref()
                .map_or(String::new(), |p| p.to_string_lossy().into_owned()),
            destination: cli
                .output
                .as_ref()
                .map_or(String::new(), |p| p.to_string_lossy().into_owned()),
            numbers: [
                s.max_angle.to_string(),
                s.min_line_percent.to_string(),
                s.min_line_pixels.to_string(),
                s.analysis_size.to_string(),
                s.merge_angle.to_string(),
                s.merge_distance_percent.to_string(),
                s.merge_distance_pixels.to_string(),
                s.merge_gap_percent.to_string(),
                s.merge_gap_pixels.to_string(),
            ],
            resample: s.resample.clone(),
            recursive: cli.recursive,
            allow_large: cli.allow_large_images,
        }
    }
    pub fn options(&self) -> Result<BatchOptions> {
        let n = |i: usize| -> Result<f64> {
            self.numbers[i].trim().parse().map_err(|_| {
                anyhow::Error::new(
                    LocalizedMessage::new("settings.error.number_required")
                        .with_message_param("label", LocalizedMessage::new(LABELS[i])),
                )
            })
        };
        let settings = DetectionSettings {
            max_angle: n(0)?,
            min_line_percent: n(1)?,
            min_line_pixels: n(2)?,
            analysis_size: self.numbers[3].trim().parse().map_err(|_| {
                anyhow::Error::new(LocalizedMessage::new(
                    "settings.error.analysis_size_integer",
                ))
            })?,
            resample: self.resample.clone(),
            merge_angle: n(4)?,
            merge_distance_percent: n(5)?,
            merge_distance_pixels: n(6)?,
            merge_gap_percent: n(7)?,
            merge_gap_pixels: n(8)?,
        };
        settings.validate().map_err(|error| {
            let Some(message) = error.downcast_ref::<LocalizedMessage>() else {
                return error;
            };
            let label = |name: &str| LocalizedMessage::new(format!("settings.label.{name}"));
            let localized = match message.key() {
                "settings.error.finite_range" => message
                    .parameters()
                    .get("name")
                    .map(|name| message.clone().with_message_param("name", label(name))),
                "settings.error.analysis_size_range" => Some(
                    LocalizedMessage::new("settings.error.label_range")
                        .with_message_param("label", label("analysis_size"))
                        .with_param("min", 256)
                        .with_param("max", 10000),
                ),
                "settings.error.resample_choice" => Some(
                    LocalizedMessage::new("settings.error.label_choice")
                        .with_message_param("label", label("resample"))
                        .with_param("choices", RESAMPLES.join("/")),
                ),
                _ => None,
            };
            localized.map_or(error, anyhow::Error::new)
        })?;
        if !(1. ..=30.).contains(&settings.max_angle) {
            return Err(LocalizedMessage::new("settings.error.max_angle_range").into());
        }
        if self.source.trim().is_empty() || self.destination.trim().is_empty() {
            return Err(LocalizedMessage::new("settings.error.choose_folders").into());
        }
        Ok(BatchOptions {
            source: PathBuf::from(self.source.trim()),
            destination: Some(PathBuf::from(self.destination.trim())),
            recursive: self.recursive,
            settings,
            allow_large_images: self.allow_large,
        })
    }
}
enum Message {
    Progress(Progress),
    Done(Result<Vec<Row>>),
}
struct App {
    form: Form,
    receiver: Option<Receiver<Message>>,
    rows: Vec<Row>,
    completed: usize,
    total: usize,
    status: Option<LocalizedMessage>,
    error: Option<anyhow::Error>,
    language: I18n,
    notices: Vec<LocalizedMessage>,
    preference_path: Option<PathBuf>,
    confirm_close: bool,
    allow_close: bool,
}
impl App {
    fn start(&mut self, ctx: &egui::Context) {
        let options = match self.form.options() {
            Ok(o) => o,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.rows.clear();
        self.completed = 0;
        self.total = 0;
        self.status = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                batch::run_batch(&options, |p| {
                    let _ = tx.send(Message::Progress(p));
                    ctx.request_repaint();
                })
            }))
            .unwrap_or_else(|_| {
                Err(anyhow::Error::new(LocalizedMessage::new(
                    "gui.error.thread_panicked",
                )))
            });
            let _ = tx.send(Message::Done(result));
            ctx.request_repaint();
        });
    }
    fn poll(&mut self) {
        let messages: Vec<_> = self
            .receiver
            .as_ref()
            .map_or(Vec::new(), |r| r.try_iter().collect());
        for m in messages {
            match m {
                Message::Progress(Progress::Log(s)) => self.status = Some(s),
                Message::Progress(Progress::Files(n, t)) => {
                    self.completed = n;
                    self.total = t;
                }
                Message::Done(result) => {
                    self.receiver = None;
                    self.confirm_close = false;
                    match result {
                        Ok(rows) => {
                            let good = rows
                                .iter()
                                .filter(|r| r.status == crate::detection::Status::Trusted)
                                .count();
                            let merged: usize = rows.iter().filter_map(|r| r.merged_count).sum();
                            self.status = Some(
                                LocalizedMessage::new("gui.status.complete")
                                    .with_param("pages", rows.len())
                                    .with_param("trusted", good)
                                    .with_param("merged", merged),
                            );
                            self.rows = rows;
                        }
                        Err(e) => {
                            self.status = Some(LocalizedMessage::new("gui.status.failed"));
                            self.error = Some(e);
                        }
                    }
                }
            }
        }
    }
}
impl App {
    fn render_ui(&mut self, ctx: &egui::Context) {
        self.poll();
        let running = self.receiver.is_some();
        if running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if ctx.input(|i| i.viewport().close_requested()) && running && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(self.language.translate("gui.label.language"));
                let mut selected = self.language.selected_locale().to_owned();
                let languages = self.language.available_languages();
                let name = languages
                    .iter()
                    .find(|l| l.id == selected)
                    .map(|l| l.name.as_str())
                    .unwrap_or(&selected);
                egui::ComboBox::from_id_salt("language")
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        for lang in &languages {
                            ui.selectable_value(&mut selected, lang.id.clone(), &lang.name);
                        }
                    });
                if selected != self.language.selected_locale()
                    && let Ok(language) = self.language.select(&selected)
                {
                    self.language = language;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                        self.language.translate("app.title"),
                    ));
                    if let Some(path) = &self.preference_path {
                        if let Err(warning) = preferences::write(path, &selected) {
                            self.notices.push(warning);
                        }
                    } else {
                        self.notices
                            .push(LocalizedMessage::new("preferences.error.location"));
                    }
                }
            });
            let language = self.language.clone();
            let t = |key: &str| language.translate(key);
            let browse_text_width = ui
                .painter()
                .layout_no_wrap(
                    t("gui.button.browse"),
                    egui::TextStyle::Button.resolve(ui.style()),
                    ui.visuals().text_color(),
                )
                .size()
                .x;
            // Reserve the translated button, its padding, item spacing, and
            // TextEdit's two four-point horizontal margins.
            let browse_reserved = browse_text_width
                + 2. * ui.spacing().button_padding.x
                + ui.spacing().item_spacing.x
                + 8.;
            egui::ScrollArea::vertical()
                .id_salt("main_content")
                .show(ui, |ui| {
                    ui.heading(t("gui.heading"));
                    ui.add_space(8.);
                    ui.add_enabled_ui(!running, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(t("gui.label.input_folder"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.form.source).desired_width(
                                    (ui.available_width() - browse_reserved).max(80.),
                                ),
                            );
                            if ui.button(t("gui.button.browse")).clicked()
                                && let Some(p) = rfd::FileDialog::new()
                                    .set_title(t("gui.dialog.choose_input_folder"))
                                    .pick_folder()
                            {
                                self.form.source = p.to_string_lossy().into_owned();
                                if let Ok(d) = batch::default_output(&p) {
                                    self.form.destination = d.to_string_lossy().into_owned();
                                }
                            }
                        });
                        ui.add_space(4.);
                        ui.horizontal(|ui| {
                            ui.label(t("gui.label.output_folder"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.form.destination)
                                    .desired_width(
                                        (ui.available_width() - browse_reserved).max(80.),
                                    ),
                            );
                            if ui.button(t("gui.button.browse")).clicked()
                                && let Some(p) = rfd::FileDialog::new()
                                    .set_title(t("gui.dialog.choose_output_folder"))
                                    .pick_folder()
                            {
                                self.form.destination = p.to_string_lossy().into_owned();
                            }
                        });
                        ui.add_space(8.);
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.form.recursive, t("gui.checkbox.recursive"));
                            ui.checkbox(&mut self.form.allow_large, t("gui.checkbox.allow_large"));
                        });
                        egui::Grid::new("settings")
                            .num_columns(2)
                            .spacing([12., 5.])
                            .show(ui, |ui| {
                                for (i, label) in LABELS.iter().enumerate() {
                                    ui.add(egui::Label::new(t(label)).wrap());
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.form.numbers[i])
                                            .desired_width(75.),
                                    );
                                    ui.end_row();
                                }
                                ui.label(t("settings.label.resample"));
                                egui::ComboBox::from_id_salt("resample")
                                    .selected_text(&self.form.resample)
                                    .show_ui(ui, |ui| {
                                        for name in RESAMPLES {
                                            ui.selectable_value(
                                                &mut self.form.resample,
                                                name.into(),
                                                name,
                                            );
                                        }
                                    });
                                ui.end_row();
                            });
                    });
                    ui.add_space(8.);
                    ui.label(t("gui.hint.orientation"));
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!running, egui::Button::new(t("gui.button.start")))
                            .clicked()
                        {
                            self.start(ctx);
                        }
                        if ui.button(t("gui.button.open_output_folder")).clicked() {
                            let path = PathBuf::from(self.form.destination.trim());
                            if path.is_dir()
                                && let Err(e) = open::that(path)
                            {
                                self.error = Some(anyhow::Error::new(e));
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        let fraction = self.completed as f32 / self.total.max(1) as f32;
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .desired_width((ui.available_width() - 150.).max(100.)),
                        );
                        ui.label(
                            language.translate_message(
                                &LocalizedMessage::new("gui.progress.files")
                                    .with_param("completed", self.completed)
                                    .with_param("total", self.total),
                            ),
                        );
                    });
                    if let Some(status) = &self.status {
                        ui.label(language.translate_message(status));
                    }
                    ui.separator();
                    egui::ScrollArea::both()
                        .max_height(280.)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Grid::new("results")
                                .striped(true)
                                .min_col_width(60.)
                                .spacing([18., 6.])
                                .show(ui, |ui| {
                                    for label in [
                                        "gui.results.file",
                                        "gui.results.page",
                                        "gui.results.angle_deviation",
                                        "gui.results.counts",
                                        "gui.results.status",
                                    ] {
                                        ui.strong(t(label));
                                    }
                                    ui.end_row();
                                    for r in &self.rows {
                                        ui.label(&r.file);
                                        ui.label(r.page.map_or("—".into(), |p| p.to_string()));
                                        ui.label(
                                            r.angle.map_or("—".into(), |a| format!("{a:+.3}")),
                                        );
                                        ui.label(match (r.fragment_count, r.merged_count) {
                                            (Some(f), Some(m)) => {
                                                format!("{f} / {m} / {}", r.count)
                                            }
                                            _ => "— / — / —".into(),
                                        });
                                        ui.label(r.status.render(&language))
                                            .on_hover_text(r.reason.render(&language));
                                        ui.end_row();
                                    }
                                });
                        });
                });
        });
        let dialog_language = self.language.clone();
        let t = |key: &str| dialog_language.translate(key);
        if let Some(error) = self.error.as_ref().map(|e| self.language.render_anyhow(e)) {
            egui::Window::new(t("gui.window.error"))
                .collapsible(false)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.label(error);
                    if ui.button(t("gui.button.ok")).clicked() {
                        self.error = None;
                    }
                });
        }
        let notices: Vec<_> = self
            .language
            .warning_texts()
            .into_iter()
            .chain(
                self.notices
                    .iter()
                    .map(|m| self.language.translate_message(m)),
            )
            .collect();
        if !notices.is_empty() {
            egui::Window::new(t("gui.window.warnings"))
                .collapsible(true)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(200.)
                        .show(ui, |ui| {
                            for notice in &notices {
                                ui.label(notice);
                            }
                        });
                });
        }
        if self.confirm_close && running {
            egui::Window::new(t("gui.window.running"))
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(t("gui.confirm.close_running"));
                    ui.horizontal(|ui| {
                        if ui.button(t("gui.button.continue")).clicked() {
                            self.confirm_close = false;
                        }
                        if ui.button(t("gui.button.exit")).clicked() {
                            self.allow_close = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.render_ui(ctx);
    }
}
pub fn run(cli: Cli, settings: DetectionSettings) -> Result<()> {
    run_localized(cli, settings, I18n::default())
}
pub fn run_localized(cli: Cli, settings: DetectionSettings, mut language: I18n) -> Result<()> {
    let preference_path = preferences::path();
    let mut notices = Vec::new();
    if cli.lang.is_none()
        && cli.lang_file.is_none()
        && let Some(path) = &preference_path
    {
        match preferences::read(path) {
            Ok(saved) => {
                language = language.select_saved(saved.as_deref())?;
            }
            Err(warning) => notices.push(warning),
        }
    }
    let title = language.translate("app.title");
    let form = Form::new(&cli, &settings);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980., 700.])
            .with_min_inner_size([880., 660.]),
        ..Default::default()
    };
    eframe::run_native(
        &title,
        options,
        Box::new(move |cc| {
            let mut fonts = egui::FontDefinitions::default();
            let windows = std::env::var_os("WINDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
            for path in [
                windows.join("Fonts/msyh.ttc"),
                windows.join("Fonts/simhei.ttf"),
                PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
            ] {
                if let Ok(bytes) = std::fs::read(path) {
                    fonts
                        .font_data
                        .insert("cjk".into(), egui::FontData::from_owned(bytes).into());
                    fonts
                        .families
                        .entry(egui::FontFamily::Proportional)
                        .or_default()
                        .insert(0, "cjk".into());
                    break;
                }
            }
            cc.egui_ctx.set_fonts(fonts);
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            Ok(Box::new(App {
                form,
                receiver: None,
                rows: Vec::new(),
                completed: 0,
                total: 0,
                status: None,
                language,
                notices,
                preference_path,
                error: None,
                confirm_close: false,
                allow_close: false,
            }))
        }),
    )
    .map_err(|e| {
        LocalizedMessage::new("gui.error.open_window")
            .with_param("error", e)
            .into()
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn finishing_a_batch_clears_pending_close_confirmation() {
        for result in [Ok(Vec::new()), Err(anyhow::anyhow!("synthetic failure"))] {
            let cli = Cli::try_parse_from(["scan-angle"]).unwrap();
            let (tx, rx) = mpsc::channel();
            let mut app = App {
                form: Form::new(&cli, &cli.settings().unwrap()),
                receiver: Some(rx),
                rows: Vec::new(),
                completed: 0,
                total: 0,
                status: None,
                language: I18n::default(),
                notices: Vec::new(),
                preference_path: None,
                error: None,
                confirm_close: true,
                allow_close: false,
            };
            tx.send(Message::Done(result)).unwrap();
            app.poll();
            assert!(app.receiver.is_none());
            assert!(!app.confirm_close);
            assert!(!app.allow_close);
        }
    }
    #[test]
    fn gui_initialization_and_validation() {
        let cli = Cli::try_parse_from([
            "scan-angle",
            "--max-angle",
            "20",
            "--recursive",
            "--output",
            "result",
            "--allow-large-images",
        ])
        .unwrap();
        let mut form = Form::new(&cli, &cli.settings().unwrap());
        assert_eq!(form.numbers[0], "20");
        assert!(form.recursive && form.allow_large);
        assert_eq!(form.destination, "result");
        assert!(form.options().is_err());
        form.source = "input".into();
        assert_eq!(form.options().unwrap().settings.max_angle, 20.);
        form.numbers[3] = "400.5".into();
        assert!(form.options().is_err());
        let cli = Cli::try_parse_from(["scan-angle", "input"]).unwrap();
        assert_eq!(Form::new(&cli, &cli.settings().unwrap()).source, "input");
    }
    fn ui_text(app: &mut App, size: egui::Vec2) -> String {
        fn texts(shape: &egui::Shape, result: &mut Vec<String>, width: f32) {
            match shape {
                egui::Shape::Text(t) => {
                    if t.galley.text() == "Browse…" {
                        assert!(
                            t.pos.x + t.galley.size().x <= width,
                            "Browse button clipped: {:?} at {:?}, width {width}",
                            t.galley.size(),
                            t.pos
                        );
                    }
                    result.push(t.galley.text().to_owned());
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        texts(shape, result, width);
                    }
                }
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| app.render_ui(ctx));
        let frame = ctx.run(input(), |ctx| app.render_ui(ctx));
        let mut result = Vec::new();
        for shape in frame.shapes {
            texts(&shape.shape, &mut result, size.x);
        }
        result.join("\n")
    }

    #[test]
    fn completed_rows_summary_and_errors_rerender_after_language_change() {
        use crate::{
            batch::RowReason,
            detection::{DetectionReason, Status},
        };
        let cli = Cli::try_parse_from(["scan-angle"]).unwrap();
        let mut app = App {
            form: Form::new(&cli, &cli.settings().unwrap()),
            receiver: None,
            rows: vec![Row {
                file: "sample.png".into(),
                page: Some(1),
                angle: Some(1.),
                correction: Some(-1.),
                status: Status::Trusted,
                count: 2,
                spread: None,
                reason: RowReason::Detection(DetectionReason::Consistent),
                preview: String::new(),
                fragment_count: Some(3),
                merged_count: Some(2),
            }],
            completed: 1,
            total: 1,
            status: Some(
                LocalizedMessage::new("gui.status.complete")
                    .with_param("pages", 1)
                    .with_param("trusted", 1)
                    .with_param("merged", 2),
            ),
            language: I18n::default(),
            notices: Vec::new(),
            preference_path: None,
            error: Some(
                LocalizedMessage::new("settings.error.number_required")
                    .with_message_param("label", LocalizedMessage::new("settings.label.max_angle"))
                    .into(),
            ),
            confirm_close: false,
            allow_close: false,
        };
        let chinese = ui_text(&mut app, egui::vec2(1000., 1000.));
        assert!(chinese.contains("完成：1 页"), "{chinese}");
        assert!(chinese.contains("可信"), "{chinese}");
        app.language = app.language.select("en-US").unwrap();
        let english = ui_text(&mut app, egui::vec2(1000., 1000.));
        assert!(english.contains("Done: 1 pages"), "{english}");
        assert!(english.contains("Trusted"), "{english}");
        assert!(
            english.contains("Max skew ± (°) must be a number"),
            "{english}"
        );
        assert!(!english.contains("完成："));
        assert_eq!(app.rows[0].status, Status::Trusted);
        // The English form can also be laid out at the supported minimum size.
        let small = ui_text(&mut app, egui::vec2(880., 660.));
        assert!(small.contains("Start detection"), "{small}");
    }
    #[test]
    fn range_errors_use_translatable_gui_labels() {
        let cli = Cli::try_parse_from(["scan-angle"]).unwrap();
        let mut form = Form::new(&cli, &cli.settings().unwrap());
        form.source = "input".into();
        form.destination = "output".into();
        let english = I18n::with_locale("en-US").unwrap();
        form.numbers[1] = "200".into();
        let text = english.render_anyhow(&form.options().unwrap_err());
        assert!(text.contains("Min line"), "{text}");
        assert!(!text.contains("min_line_percent"));
        form.numbers[1] = "10".into();
        form.numbers[3] = "10001".into();
        let text = english.render_anyhow(&form.options().unwrap_err());
        assert!(!text.contains("analysis_size"));
        assert!(text.contains("256") && text.contains("10000"));
    }
}
