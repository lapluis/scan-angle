use crate::{
    batch::{self, BatchOptions, Progress, Row},
    cli::Cli,
    detection::{DetectionSettings, RESAMPLES},
};
use anyhow::{Result, bail};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    time::Duration,
};

const LABELS: [&str; 9] = [
    "最大偏角 ±（°）",
    "最短直线 ≥ 对应方向边长（%）",
    "最短直线 ≥ 分析像素",
    "分析图最长边（像素，仅缩小）",
    "合并角度差 ≤（°）",
    "垂直距离容差：短边（%）",
    "垂直距离容差：至少（像素）",
    "间隙容差：短边（%）",
    "间隙容差：至少（像素）",
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
            source: String::new(),
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
            self.numbers[i]
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("{}必须是数值", LABELS[i]))
        };
        let settings = DetectionSettings {
            max_angle: n(0)?,
            min_line_percent: n(1)?,
            min_line_pixels: n(2)?,
            analysis_size: self.numbers[3]
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("分析图最长边必须是整数"))?,
            resample: self.resample.clone(),
            merge_angle: n(4)?,
            merge_distance_percent: n(5)?,
            merge_distance_pixels: n(6)?,
            merge_gap_percent: n(7)?,
            merge_gap_pixels: n(8)?,
        };
        settings.validate()?;
        if !(1. ..=30.).contains(&settings.max_angle) {
            bail!("最大偏角必须在 1～30 之间");
        }
        if self.source.trim().is_empty() || self.destination.trim().is_empty() {
            bail!("请选择输入和输出文件夹");
        }
        Ok(BatchOptions {
            source: PathBuf::from(self.source.trim()),
            destination: PathBuf::from(self.destination.trim()),
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
    status: String,
    error: Option<String>,
    confirm_close: bool,
    allow_close: bool,
}
impl App {
    fn start(&mut self, ctx: &egui::Context) {
        let options = match self.form.options() {
            Ok(o) => o,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.rows.clear();
        self.completed = 0;
        self.total = 0;
        self.status.clear();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                batch::run_batch(&options, |p| {
                    let _ = tx.send(Message::Progress(p));
                    ctx.request_repaint();
                })
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("检测线程异常退出")));
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
                Message::Progress(Progress::Log(s)) => self.status = s,
                Message::Progress(Progress::Files(n, t)) => {
                    self.completed = n;
                    self.total = t;
                }
                Message::Done(result) => {
                    self.receiver = None;
                    match result {
                        Ok(rows) => {
                            let good = rows.iter().filter(|r| r.status == "可信").count();
                            let merged: usize = rows.iter().filter_map(|r| r.merged_count).sum();
                            self.status = format!(
                                "完成：{} 页；可信 {good} 页；合并直线共 {merged} 条；其余请核对。已保存 angles.csv 和预览图。",
                                rows.len()
                            );
                            self.rows = rows;
                        }
                        Err(e) => {
                            self.status = "检测失败".into();
                            self.error = Some(format!("{e:#}"));
                        }
                    }
                }
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
            ui.heading("扫图偏角检测");
            ui.add_space(8.);
            ui.add_enabled_ui(!running, |ui| {
                egui::Grid::new("paths")
                    .num_columns(3)
                    .spacing([10., 8.])
                    .show(ui, |ui| {
                        ui.label("输入文件夹");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.form.source)
                                .desired_width((ui.available_width() - 80.).max(200.)),
                        );
                        if ui.button("浏览…").clicked()
                            && let Some(p) = rfd::FileDialog::new()
                                .set_title("选择扫描图片文件夹")
                                .pick_folder()
                        {
                            self.form.source = p.to_string_lossy().into_owned();
                            if let Ok(d) = batch::default_output(&p) {
                                self.form.destination = d.to_string_lossy().into_owned();
                            }
                        }
                        ui.end_row();
                        ui.label("输出文件夹");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.form.destination)
                                .desired_width((ui.available_width() - 80.).max(200.)),
                        );
                        if ui.button("浏览…").clicked()
                            && let Some(p) = rfd::FileDialog::new()
                                .set_title("选择结果保存文件夹")
                                .pick_folder()
                        {
                            self.form.destination = p.to_string_lossy().into_owned();
                        }
                        ui.end_row();
                    });
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.form.recursive, "包含子文件夹");
                    ui.checkbox(&mut self.form.allow_large, "允许超大图片");
                });
                egui::Grid::new("settings")
                    .num_columns(4)
                    .spacing([12., 5.])
                    .show(ui, |ui| {
                        for (i, label) in LABELS.iter().enumerate() {
                            ui.label(*label);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.form.numbers[i])
                                    .desired_width(75.),
                            );
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                        ui.label("缩放算法");
                        egui::ComboBox::from_id_salt("resample")
                            .selected_text(&self.form.resample)
                            .show_ui(ui, |ui| {
                                for name in RESAMPLES {
                                    ui.selectable_value(&mut self.form.resample, name.into(), name);
                                }
                            });
                        ui.end_row();
                    });
            });
            ui.add_space(8.);
            ui.label("正数＝顺时针；负数＝逆时针。蓝线＝合并直线，红线＝入选证据。");
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!running, egui::Button::new("开始检测"))
                    .clicked()
                {
                    self.start(ctx);
                }
                if ui.button("打开结果文件夹").clicked() {
                    let path = PathBuf::from(self.form.destination.trim());
                    if path.is_dir()
                        && let Err(e) = open::that(path)
                    {
                        self.error = Some(e.to_string());
                    }
                }
            });
            ui.horizontal(|ui| {
                let fraction = self.completed as f32 / self.total.max(1) as f32;
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width((ui.available_width() - 150.).max(100.)),
                );
                ui.label(format!("{} / {} 文件", self.completed, self.total));
            });
            if !self.status.is_empty() {
                ui.label(&self.status);
            }
            ui.separator();
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("results")
                        .striped(true)
                        .min_col_width(60.)
                        .spacing([18., 6.])
                        .show(ui, |ui| {
                            for label in
                                ["文件", "页", "偏差角度（°）", "检测 / 合并 / 入选", "状态"]
                            {
                                ui.strong(label);
                            }
                            ui.end_row();
                            for r in &self.rows {
                                ui.label(&r.file);
                                ui.label(r.page.map_or("—".into(), |p| p.to_string()));
                                ui.label(r.angle.map_or("—".into(), |a| format!("{a:+.3}")));
                                ui.label(match (r.fragment_count, r.merged_count) {
                                    (Some(f), Some(m)) => format!("{f} / {m} / {}", r.count),
                                    _ => "— / — / —".into(),
                                });
                                ui.label(&r.status).on_hover_text(&r.reason);
                                ui.end_row();
                            }
                        });
                });
        });
        if let Some(error) = self.error.clone() {
            egui::Window::new("输入或处理错误")
                .collapsible(false)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.label(error);
                    if ui.button("确定").clicked() {
                        self.error = None;
                    }
                });
        }
        if self.confirm_close {
            egui::Window::new("正在检测")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("退出会中断检测，当前批次可能没有完整 CSV。确定退出？");
                    ui.horizontal(|ui| {
                        if ui.button("继续检测").clicked() {
                            self.confirm_close = false;
                        }
                        if ui.button("退出").clicked() {
                            self.allow_close = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
        }
    }
}
pub fn run(cli: Cli, settings: DetectionSettings) -> Result<()> {
    let form = Form::new(&cli, &settings);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980., 700.])
            .with_min_inner_size([880., 660.]),
        ..Default::default()
    };
    eframe::run_native(
        "扫图偏角检测",
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
                status: String::new(),
                error: None,
                confirm_close: false,
                allow_close: false,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("无法打开窗口：{e}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
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
    }
}
