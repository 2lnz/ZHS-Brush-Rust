//! egui 界面，对应原项目 `gui.py` 的 tkinter 版。
//!
//! 布局：顶部状态栏 / 左侧登录与参数 / 中间课程清单 / 底部日志。
//! 二维码用弹窗显示（不再依赖系统看图软件），验证码风控用模态提示。

use crate::config::{Config, RunOpts};
use crate::events::{Control, Ev, Sink};
use crate::model::CourseRow;
use crate::study::Fucker;
use crate::util;
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

/// 启动 GUI。
pub fn run(dir: PathBuf) -> Result<(), eframe::Error> {
    enable_dpi_awareness();
    let cfg = Config::load(&dir);
    let (win_w, win_h) = cfg.window_size();
    // eframe/winit 把这里的数值当**物理像素**，而 egui 按 DPI 缩放排版，
    // 1.25 缩放下 900 物理像素只够放 720 逻辑点，右侧会被裁掉。
    // 这里按系统 DPI 折算，再按屏幕高度收一下，避免超出工作区。
    let borderless = cfg.borderless();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([win_w, win_h])
        .with_min_inner_size([700.0, 480.0])
        // 无边框时自己画标题栏（拖动、最小化、最大化、关闭）
        .with_decorations(!borderless)
        // 无边框时开透明：窗口四角由我们自己裁出圆角
        .with_transparent(borderless && cfg.transparent() && cfg.corner_radius() > 0.5)
        .with_title("ZHS-Brush-RUST");
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "ZHS-Brush-RUST",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, dir, cfg)))),
    )
}

/// 窗口/任务栏图标：直接吃打包进来的 PNG。
fn app_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../../assets/icon.png");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some(egui::IconData {
        rgba: img.as_raw().clone(),
        width: img.width(),
        height: img.height(),
    })
}

struct CaptchaPrompt {
    url: String,
    video: String,
    timeout_sec: u64,
    started: Instant,
}

pub struct App {
    dir: PathBuf,
    cfg: Config,
    opts: RunOpts,
    tx: Sender<Ev>,
    rx: Receiver<Ev>,
    ctl: Arc<Control>,

    log_lines: Vec<String>,
    status: String,
    login_ok: bool,
    progress: Option<(String, f32, String)>,
    busy: bool,
    task_name: String,

    courses: Vec<CourseRow>,
    selected: Vec<bool>,

    qr_texture: Option<egui::TextureHandle>,
    /// 界面线程拿不到 egui 上下文，先把 PNG 存这里，下一帧再解码成纹理
    pending_qr: Option<Vec<u8>>,
    qr_open: bool,
    captcha: Option<CaptchaPrompt>,

    speed_text: String,
    limit_text: String,
    end_text: String,
    /// 平时测试作答页链接
    hw_url: String,
    /// 批量处理本课程全部未提交测试
    hw_batch: bool,
    /// 交卷二次确认：记录已确认过的链接（换了链接要重新确认）
    hw_armed_for: Option<String>,
    /// AI 配置弹窗是否打开
    ai_open: bool,
    ai_base: String,
    ai_key: String,
    ai_model: String,
    /// true = 深色（默认），false = 白色背景的浅色主题
    dark: bool,
    /// 无边框（自绘标题栏）；false 时用系统标题栏
    borderless: bool,
    /// 自绘标题栏里的最大化状态
    maximized: bool,
    /// 帧计数：周期性重申无边框样式（winit 显示窗口时会把 WS_CAPTION 加回来）
    frames: u32,
    /// 窗口圆角半径（config.json 的 ui.corner_radius）
    corner: f32,
    /// 是否用透明窗口做圆角（false 时改用 SetWindowRgn 硬裁剪）
    transparent: bool,
    /// 最终是否启用圆角：无边框 + 半径>0 + 透明窗口
    rounded: bool,
}

const LOG_MAX: usize = 4000;

/// 应用主题：深色 / 浅色（白色背景）。
fn apply_theme(ctx: &egui::Context, dark: bool, transparent_panels: bool) {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    // 面板底色稍微调一下，浅色模式纯白会刺眼
    visuals.panel_fill = if dark {
        egui::Color32::from_rgb(27, 29, 32)
    } else {
        egui::Color32::from_rgb(250, 250, 252)
    };
    if transparent_panels {
        // 透明窗口：面板不再铺底色，统一由每帧画的圆角底板提供背景
        visuals.panel_fill = egui::Color32::TRANSPARENT;
        visuals.window_fill = if dark {
            egui::Color32::from_rgb(33, 35, 39)
        } else {
            egui::Color32::WHITE
        };
    }
    ctx.set_visuals(visuals);
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, dir: PathBuf, cfg: Config) -> Self {
        install_cjk_font(&cc.egui_ctx);
        let (tx, rx) = channel();
        let dir = if dir.is_absolute() {
            dir
        } else {
            std::env::current_dir()
                .map(|c| c.join(&dir))
                .unwrap_or(dir)
        };
        // join(".") 会留下尾部的 "\."，规范成真实路径；顺手去掉 Windows 的 \\?\ 前缀
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        let dir = match dir.to_string_lossy().strip_prefix(r"\\?\") {
            Some(rest) => std::path::PathBuf::from(rest),
            None => dir,
        };
        let login_ok = cfg.cookies_path().exists();
        let dark = cfg.theme() != "light";
        let borderless = cfg.borderless();
        let corner = cfg.corner_radius();
        let transparent = cfg.transparent();
        let rounded = borderless && corner > 0.5 && transparent;
        let mut app = Self {
            dir,
            opts: RunOpts {
                speed: Some(1.5),
                limit_min: cfg.limit_min(),
                end_thre: 0.91,
            },
            cfg,
            tx,
            rx,
            ctl: Control::new(),
            log_lines: Vec::new(),
            status: if login_ok {
                "登录状态：已保存".into()
            } else {
                "登录状态：未登录".into()
            },
            login_ok,
            progress: None,
            busy: false,
            task_name: String::new(),
            courses: Vec::new(),
            selected: Vec::new(),
            qr_texture: None,
            pending_qr: None,
            qr_open: false,
            captcha: None,
            speed_text: "1.5".into(),
            limit_text: "0".into(),
            end_text: "0.91".into(),
            hw_url: String::new(),
            hw_batch: true,
            hw_armed_for: None,
            ai_open: false,
            ai_base: String::new(),
            ai_key: String::new(),
            ai_model: String::new(),
            dark,
            borderless,
            maximized: false,
            frames: 0,
            corner,
            transparent,
            rounded,
        };
        apply_theme(&cc.egui_ctx, app.dark, app.rounded);
        app.push_log(format!("数据目录：{}", app.dir.display()));
        app.push_log(format!("代理：跟随系统（可在 config.json 的 proxies 覆盖）"));
        app.push_log(format!(
            "界面主题：{}（左侧「浅色主题/深色主题」按钮切换，会写回 config.json）",
            if app.dark { "深色" } else { "浅色" }
        ));
        app
    }

    fn push_log(&mut self, line: impl Into<String>) {
        self.log_lines.push(line.into());
        if self.log_lines.len() > LOG_MAX {
            let drop = self.log_lines.len() - LOG_MAX;
            self.log_lines.drain(0..drop);
        }
    }

    fn sink(&self, ctx: &egui::Context) -> Sink {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        Arc::new(move |ev| {
            let _ = tx.send(ev);
            ctx.request_repaint();
        })
    }

    fn reload_config(&mut self) {
        self.cfg = Config::load(&self.dir);
        self.opts.limit_min = self.cfg.limit_min();
    }

    // ---------------------------------------------------------------- 动作

    fn do_login(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.task_name = "扫码登录".into();
        self.ctl.reset();
        self.reload_config();
        let sink = self.sink(ctx);
        let ctl = self.ctl.clone();
        let dir = self.dir.clone();
        let s2 = sink.clone();
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<String> {
                let cfg = Config::load(&dir);
                let mut fucker = Fucker::new(&cfg, RunOpts::default(), sink.clone(), ctl)?;
                fucker.login_qr(&cfg)?;
                // 视频服务登录后，主动走一遍测试服务的 SSO 链：
                // 把服务票据 cookie 一起落进 shared_test_cookies.json，避免首次开卷才发现未登录
                let sso = crate::api::exam::sso_login(&mut fucker.http);
                let jar = fucker.http.cookies.to_json();
                let text = serde_json::to_string_pretty(&jar).unwrap_or_default();
                let test_path = dir.join("shared_test_cookies.json");
                let _ = std::fs::write(&test_path, text);
                match sso {
                    Ok(()) => Ok("登录成功（测试服务会话已建立）".to_string()),
                    Err(e) => Ok(format!("登录成功，但测试服务会话未建立: {e}（可先刷课，稍后重试）")),
                }
            })();
            match res {
                Ok(msg) => s2(Ev::Done(Ok(msg))),
                Err(e) => s2(Ev::Done(Err(format!("{e:#}")))),
            }
        });
    }

    fn do_fetch(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.task_name = "拉取课程清单".into();
        self.ctl.reset();
        self.reload_config();
        let sink = self.sink(ctx);
        let ctl = self.ctl.clone();
        let dir = self.dir.clone();
        let s2 = sink.clone();
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<String> {
                let cfg = Config::load(&dir);
                let mut fucker = Fucker::new(&cfg, RunOpts::default(), sink.clone(), ctl)?;
                let rows = fucker.fetch_courses()?;
                std::fs::write(cfg.execution_path(), serde_json::to_string_pretty(&rows)?)?;
                sink(Ev::Courses(rows.clone()));
                Ok(format!("清单已保存：{} 门课", rows.len()))
            })();
            match res {
                Ok(msg) => s2(Ev::Done(Ok(msg))),
                Err(e) => s2(Ev::Done(Err(format!("{e:#}")))),
            }
        });
    }

    /// 平时测试：读取检查 / AI 答题交卷（后台线程 + 日志上报）。
    fn do_homework(&mut self, ctx: &egui::Context, execute: bool) {
        if self.busy {
            return;
        }
        let url = self.hw_url.trim().to_string();
        let identity = match crate::api::exam::HomeworkIdentity::from_url(&url) {
            Ok(v) => v,
            Err(e) => {
                self.push_log(format!("!! 链接无效: {e}"));
                return;
            }
        };
        if execute && self.hw_armed_for.as_deref() != Some(url.as_str()) {
            self.hw_armed_for = Some(url);
            self.push_log("!! 再点一次「确认交卷」才会开始：AI 生成答案 → 保存读回 → 交卷");
            return;
        }
        self.hw_armed_for = None;
        self.busy = true;
        self.task_name = if execute { "平时测试交卷" } else { "平时测试检查" }.into();
        let batch = self.hw_batch;
        let sink = self.sink(ctx);
        let dir = self.dir.clone();
        std::thread::spawn(move || {
            let mut report = |line: String| sink(Ev::Log(line));
            let res = crate::homework::run_all(&dir, identity, execute, batch, &mut report);
            match res {
                Ok(()) => {
                    let msg = if execute {
                        "平时测试：已完成"
                    } else {
                        "平时测试：读取检查完成（未写服务器）"
                    };
                    sink(Ev::Done(Ok(msg.into())));
                }
                Err(crate::api::exam::ExamError::LoginRequired(msg)) => {
                    sink(Ev::Done(Err(format!("需要重新登录: {msg}"))))
                }
                Err(crate::api::exam::ExamError::NeedVerify { msg, url }) => {
                    // 自动把验证页拉起来，用户滑完再点一次交卷即可
                    match crate::util::open_path(std::path::Path::new(&url)) {
                        Ok(()) => sink(Ev::Log(
                            "[风控] 已自动打开测试页，请完成滑块验证后重新点「AI 答题交卷」".into(),
                        )),
                        Err(e) => sink(Ev::Log(format!(
                            "[风控] 请手动在浏览器打开：{url}（自动打开失败: {e}）"
                        ))),
                    }
                    sink(Ev::Done(Err(msg)))
                }
                Err(crate::api::exam::ExamError::Failed(msg)) => sink(Ev::Done(Err(msg))),
            }
        });
    }

    fn do_run(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        let ids: Vec<String> = self
            .courses
            .iter()
            .zip(self.selected.iter())
            .filter(|(_, s)| **s)
            .map(|(c, _)| c.id.clone())
            .collect();
        if ids.is_empty() {
            self.push_log("[提示] 先在右侧勾选要刷的课程");
            return;
        }
        self.busy = true;
        self.task_name = format!("刷 {} 门课", ids.len());
        self.ctl.reset();
        self.reload_config();
        let opts = self.run_opts();
        let sink = self.sink(ctx);
        let ctl = self.ctl.clone();
        let dir = self.dir.clone();
        let s2 = sink.clone();
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<String> {
                let cfg = Config::load(&dir);
                let mut fucker = Fucker::new(&cfg, opts.clone(), sink.clone(), ctl.clone())?;
                if !fucker.has_cookies() {
                    anyhow::bail!("还没有登录，先扫码登录");
                }
                let results = fucker.run_courses(&ids);
                let failed = results.iter().filter(|(_, r)| r.is_err()).count();
                Ok(format!(
                    "任务结束：{} 门成功，{} 门失败",
                    results.len() - failed,
                    failed
                ))
            })();
            match res {
                Ok(msg) => s2(Ev::Done(Ok(msg))),
                Err(e) => s2(Ev::Done(Err(format!("{e:#}")))),
            }
        });
    }

    fn run_opts(&self) -> RunOpts {
        let speed = self.speed_text.trim().parse::<f64>().ok();
        let limit = self.limit_text.trim().parse::<u64>().unwrap_or(0);
        let end = self
            .end_text
            .trim()
            .parse::<f64>()
            .unwrap_or(0.91)
            .clamp(0.1, 1.0);
        RunOpts {
            speed,
            limit_min: limit,
            end_thre: end,
        }
    }

    fn drain_events(&mut self) {
        let mut n = 0;
        while let Ok(ev) = self.rx.try_recv() {
            n += 1;
            if n > 500 {
                break;
            }
            match ev {
                Ev::Log(line) => self.push_log(line),
                Ev::Status(s) => self.status = s,
                Ev::Progress { action, cur, end } => {
                    let frac = if end > 0.0 {
                        (cur / end).clamp(0.0, 1.0) as f32
                    } else {
                        0.0
                    };
                    let text = format!(
                        "{}  {} / {}",
                        action,
                        util::hms(cur),
                        util::hms(end)
                    );
                    self.progress = Some((action, frac, text));
                }
                Ev::Qr(bytes) => {
                    self.pending_qr = Some(bytes);
                    self.qr_open = true;
                    self.push_log("[二维码] 已显示，请用知到 App 扫码");
                }
                Ev::Captcha {
                    url,
                    video,
                    timeout_sec,
                } => {
                    self.push_log("[风控] 需要验证码，刷课已暂停");
                    // 自动把课程页拉起来，用户只需拖滑块/点图形码；探测解除后会自动继续
                    match util::open_path(std::path::Path::new(&url)) {
                        Ok(()) => self.push_log("[风控] 已自动打开浏览器课程页，完成验证码后无需操作"),
                        Err(e) => self.push_log(format!("[风控] 自动打开浏览器失败（{e}），请手动打开：{url}")),
                    }
                    self.captcha = Some(CaptchaPrompt {
                        url,
                        video,
                        timeout_sec,
                        started: Instant::now(),
                    });
                }
                Ev::CaptchaOk => {
                    self.captcha = None;
                    self.push_log("[风控] 已解除，继续刷课");
                }
                Ev::CaptchaTimeout => {
                    self.captcha = None;
                    self.push_log("[风控] 等待结束");
                }
                Ev::Courses(rows) => {
                    self.selected = vec![true; rows.len()];
                    self.courses = rows;
                }
                Ev::LoginState(ok) => {
                    self.login_ok = ok;
                    self.status = if ok {
                        "登录状态：已保存".into()
                    } else {
                        "登录状态：未登录".into()
                    };
                    if ok {
                        // 登录成功就把二维码窗口收掉，别让用户以为还要再扫
                        self.qr_open = false;
                        self.qr_texture = None;
                    }
                }
                Ev::Done(res) => {
                    self.busy = false;
                    self.task_name.clear();
                    self.progress = None;
                    match res {
                        Ok(msg) => self.push_log(format!("== {msg}")),
                        Err(msg) => self.push_log(format!("!! {msg}")),
                    }
                }
            }
        }
    }

    fn load_qr_texture(&mut self, ctx: &egui::Context, png: &[u8]) {
        match image::load_from_memory(png) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                self.qr_texture =
                    Some(ctx.load_texture("qr", color, egui::TextureOptions::NEAREST));
            }
            Err(e) => self.push_log(format!("[二维码解码失败] {e}")),
        }
    }

    // ---------------------------------------------------------------- 绘制

    /// 无边框模式下的自绘标题栏：拖动 / 最小化 / 最大化 / 关闭 / 切回系统边框。
    fn draw_titlebar(&mut self, ctx: &egui::Context) {
        let fill = if self.dark {
            egui::Color32::from_rgb(23, 25, 28)
        } else {
            egui::Color32::from_rgb(240, 241, 244)
        };
        let (mut close, mut min, mut max) = (false, false, false);

        egui::TopBottomPanel::top("titlebar")
            .exact_height(34.0)
            .frame(
                egui::Frame::none()
                    .fill(if self.rounded {
                        egui::Color32::TRANSPARENT
                    } else {
                        fill
                    })
                    .inner_margin(egui::Margin::symmetric(10.0, 0.0)),
            )
            .show(ctx, |ui| {
                // 整条标题栏都是拖动区；后添加的按钮会盖在上面，点击优先给按钮
                let bar = ui.max_rect();
                let drag = ui.interact(
                    bar,
                    egui::Id::new("titlebar_drag"),
                    egui::Sense::click_and_drag(),
                );
                if drag.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if drag.double_clicked() {
                    max = true;
                }

                ui.horizontal_centered(|ui| {
                    ui.label(egui::RichText::new("ZHS-Brush-RUST").strong());
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("v{}", crate::APP_VERSION))
                            .weak()
                            .small(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("×")
                            .on_hover_text("关闭")
                            .clicked()
                        {
                            close = true;
                        }
                        if ui
                            .button(if self.maximized { "❐" } else { "□" })
                            .on_hover_text("最大化 / 还原")
                            .clicked()
                        {
                            max = true;
                        }
                        if ui
                            .button("—")
                            .on_hover_text("最小化")
                            .clicked()
                        {
                            min = true;
                        }

                    });
                });
            });

        if min {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        if max {
            self.maximized = !self.maximized;
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.maximized));
        }
        if close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn draw_top(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if !self.borderless {
                    ui.label(
                        egui::RichText::new("ZHS-Brush-RUST")
                            .size(20.0)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(format!("v{}", crate::APP_VERSION))
                            .weak()
                            .small(),
                    );
                    ui.separator();
                }
                // 浅色模式下把绿/红调深一点，白底上才看得清
                let color = if self.login_ok {
                    if self.dark {
                        egui::Color32::from_rgb(70, 190, 110)
                    } else {
                        egui::Color32::from_rgb(20, 120, 60)
                    }
                } else if self.dark {
                    egui::Color32::from_rgb(220, 95, 85)
                } else {
                    egui::Color32::from_rgb(170, 45, 35)
                };
                ui.colored_label(color, &self.status);
                ui.separator();
                if self.busy {
                    ui.spinner();
                    ui.label(format!("正在{}", self.task_name));
                }
            });
            ui.add_space(4.0);
        });
        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.add_space(2.0);
            match &self.progress {
                Some((_, frac, text)) => {
                    ui.add(egui::ProgressBar::new(*frac).text(text.clone()));
                }
                None => {
                    ui.add(egui::ProgressBar::new(0.0).text("未在刷课"));
                }
            }
            ui.add_space(2.0);
        });
    }

    fn draw_left(&mut self, ctx: &egui::Context) {
        let busy = self.busy;
        egui::SidePanel::left("left")
            .resizable(false)
            .exact_width(300.0)
            .show(ctx, |ui| {
                let scroll_h = (ui.available_height() - 78.0).max(120.0);
                egui::ScrollArea::vertical()
                    .id_salt("left_scroll")
                    .max_height(scroll_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                ui.add_space(6.0);
                let mut want_toggle = false;
                ui.horizontal(|ui| {
                    if ui
                        .button(if self.dark { "浅色主题" } else { "深色主题" })
                        .on_hover_text("白底 / 深色可自由切换，选择会写回 config.json 的 ui.theme")
                        .clicked()
                    {
                        want_toggle = true;
                    }
                    if ui.button("数据目录").clicked() {
                        let _ = util::open_path(&self.dir);
                    }
                });
                if want_toggle {
                    self.dark = !self.dark;
                    apply_theme(ctx, self.dark, self.rounded);
                    self.cfg.set_theme(if self.dark { "dark" } else { "light" });
                    self.push_log(format!(
                        "已切换到{}主题（config.json 的 ui.theme 已更新）",
                        if self.dark { "深色" } else { "浅色" }
                    ));
                }
                ui.add_space(4.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(egui::RichText::new("1. 登录").strong());
                    ui.label("仅支持知到 App 扫码（账号密码登录已基本停用）");
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("扫码登录"))
                            .clicked()
                        {
                            self.do_login(ctx);
                        }
                        if ui.add_enabled(!busy, egui::Button::new("重新扫码")).clicked() {
                            let path = self.cfg.cookies_path();
                            if path.exists() {
                                let _ = std::fs::remove_file(&path);
                            }
                            self.login_ok = false;
                            self.push_log("[提示] 已清除 cookies.json，将强制生成新二维码");
                            self.do_login(ctx);
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("打开二维码文件夹").clicked() {
                            let dir = self.cfg.qr_dir();
                            let _ = util::ensure_dir(&dir);
                            let _ = util::open_path(&dir);
                        }
                        if ui.button("清除登录状态").clicked() {
                            let path = self.cfg.cookies_path();
                            if path.exists() {
                                let _ = std::fs::remove_file(&path);
                            }
                            self.login_ok = false;
                            self.status = "登录状态：未登录".into();
                            self.push_log("[提示] 已清除登录状态");
                        }
                    });
                });

                ui.add_space(8.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(egui::RichText::new("2. 课程清单").strong());
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("拉取智慧树清单"))
                            .clicked()
                        {
                            self.do_fetch(ctx);
                        }
                        if ui.button("打开 execution.json").clicked() {
                            let p = self.cfg.execution_path();
                            let _ = util::open_path(&p);
                        }
                    });
                    ui.label(
                        egui::RichText::new("清单会写入 execution.json，可直接手改")
                            .weak()
                            .small(),
                    );
                });

                ui.add_space(8.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(egui::RichText::new("3. 刷课参数").strong());
                    egui::Grid::new("opts").num_columns(2).show(ui, |ui| {
                        ui.label("倍速");
                        ui.text_edit_singleline(&mut self.speed_text);
                        ui.end_row();
                        ui.label("限时(分钟,0=不限)");
                        ui.text_edit_singleline(&mut self.limit_text);
                        ui.end_row();
                        ui.label("结束阈值");
                        ui.text_edit_singleline(&mut self.end_text);
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        for v in ["1.5", "2", "3", "5"] {
                            if ui.small_button(v).clicked() {
                                self.speed_text = v.to_string();
                            }
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !busy && self.selected.iter().any(|s| *s),
                                egui::Button::new(egui::RichText::new("开始刷课").strong()),
                            )
                            .clicked()
                        {
                            self.do_run(ctx);
                        }
                        if ui.add_enabled(busy, egui::Button::new("停止")).clicked() {
                            self.ctl.request_stop();
                            self.push_log("[提示] 已发送停止指令");
                        }
                    });
                });

                ui.add_space(10.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(egui::RichText::new("4. 平时测试（作业）").strong());
                    ui.label(
                        egui::RichText::new("粘贴作答页链接：onlineexamh5...\n#/webExamList/dohomework/...")
                            .weak()
                            .small(),
                    );
                    ui.add(
                        egui::TextEdit::multiline(&mut self.hw_url)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .hint_text("https://onlineexamh5new.zhihuishu.com/..."),
                    );
                    ui.checkbox(&mut self.hw_batch, "处理本课程全部未提交测试");
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy && !self.hw_url.trim().is_empty(), egui::Button::new("读取检查"))
                            .on_hover_text("只读：开卷 + 校验题型，不写服务器任何数据")
                            .clicked()
                        {
                            self.do_homework(ctx, false);
                        }
                        let label = if self.hw_armed_for.as_deref() == Some(self.hw_url.trim()) {
                            egui::RichText::new("确认交卷？").strong()
                        } else {
                            egui::RichText::new("AI 答题交卷")
                        };
                        if ui
                            .add_enabled(!busy && !self.hw_url.trim().is_empty(), egui::Button::new(label))
                            .on_hover_text("AI 生成答案 → 逐题保存并读回核验 → 交卷（需点两次确认）")
                            .clicked()
                        {
                            self.do_homework(ctx, true);
                        }
                        if ui
                            .button("DS API")
                            .on_hover_text("填写 OpenAI 兼容接口（DeepSeek）：api_base / api_key / 模型")
                            .clicked()
                        {
                            let (b, k, m) = self.cfg.ai_settings();
                            self.ai_base = b;
                            self.ai_key = k;
                            self.ai_model = if m.is_empty() { "deepseek-chat".into() } else { m };
                            self.ai_open = true;
                        }
                    });
                    ui.label(
                        egui::RichText::new("只支持单选/多选/判断；遇到其它题型整卷停止")
                            .weak()
                            .small(),
                    );
                });

                    });
                // 底部固定：选中计数
                ui.add_space(6.0);
                let sel = self.selected.iter().filter(|s| **s).count();
                ui.label(
                    egui::RichText::new(format!("已选 {}/{} 门", sel, self.courses.len())).weak(),
                );
            });
    }

    fn draw_center(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("课程（勾选后开始刷课）").strong());
                if ui.small_button("全选").clicked() {
                    for s in self.selected.iter_mut() {
                        *s = true;
                    }
                }
                if ui.small_button("全不选").clicked() {
                    for s in self.selected.iter_mut() {
                        *s = false;
                    }
                }
            });
            ui.separator();

            let list_height = (ui.available_height() * 0.42).max(120.0);
            egui::ScrollArea::vertical()
                .id_salt("courses")
                .max_height(list_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if self.courses.is_empty() {
                        ui.label(egui::RichText::new("还没有课程清单，点左侧「拉取智慧树清单」").weak());
                    }
                    for (i, course) in self.courses.iter().enumerate() {
                        if i >= self.selected.len() {
                            break;
                        }
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.selected[i], "");
                            ui.label(egui::RichText::new(format!("[{}]", course.kind.label())).weak());
                            ui.label(&course.name);
                            ui.label(egui::RichText::new(&course.id).weak().small());
                        });
                    }
                });

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("日志").strong());
                if ui.small_button("清空").clicked() {
                    self.log_lines.clear();
                }
            });
            let text = self.log_lines.join("\n");
            egui::ScrollArea::vertical()
                .id_salt("log")
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).monospace().size(12.0))
                            .wrap(),
                    );
                });
        });
    }

    fn draw_qr_window(&mut self, ctx: &egui::Context) {
        if !self.qr_open {
            return;
        }
        let mut open = true;
        egui::Window::new("扫码登录")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("知到 App → 底部「我的」→ 右上角「扫一扫」").strong(),
                );
                let warn = if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(220, 95, 85)
                } else {
                    egui::Color32::from_rgb(170, 45, 35)
                };
                ui.label(egui::RichText::new("必须用知到 App，微信扫无效").color(warn));
                ui.add_space(6.0);
                match &self.qr_texture {
                    Some(tex) => {
                        let s = egui::load::SizedTexture::new(tex.id(), egui::vec2(320.0, 320.0));
                        ui.add(egui::Image::new(s));
                    }
                    None => {
                        ui.add_space(120.0);
                        ui.label("二维码还在生成…");
                        ui.add_space(120.0);
                    }
                }
                ui.add_space(6.0);
                ui.label(egui::RichText::new("过期会自动刷新").weak().small());
                if ui.button("关闭").clicked() {
                    self.qr_open = false;
                }
            });
        if !open {
            self.qr_open = false;
        }
    }

    /// AI 接口配置弹窗：api_base / api_key / 模型，保存写回 config.json
    fn draw_ai_window(&mut self, ctx: &egui::Context) {
        if !self.ai_open {
            return;
        }
        let mut open = true;
        let mut save = false;
        egui::Window::new("AI 接口设置（OpenAI 兼容）")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("平时测试的 AI 答案走这个接口（DeepSeek 默认已填好）").weak(),
                );
                ui.add_space(4.0);
                egui::Grid::new("ai_grid").num_columns(2).show(ui, |ui| {
                    ui.label("API 地址");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.ai_base)
                            .desired_width(320.0)
                            .hint_text("https://api.deepseek.com"),
                    );
                    ui.end_row();
                    ui.label("API Key");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.ai_key)
                            .desired_width(320.0)
                            .password(true)
                            .hint_text("sk-..."),
                    );
                    ui.end_row();
                    ui.label("模型");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.ai_model)
                            .desired_width(320.0)
                            .hint_text("deepseek-chat"),
                    );
                    ui.end_row();
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("保存后会自动把 ai.enabled 设为 true、use_zhidao_ai 设为 false")
                        .weak()
                        .small(),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("保存").clicked() {
                        save = true;
                    }
                    if ui.button("取消").clicked() {
                        self.ai_open = false;
                    }
                });
            });
        if save {
            let (b, k, m) = (
                self.ai_base.trim().to_string(),
                self.ai_key.trim().to_string(),
                self.ai_model.trim().to_string(),
            );
            if b.is_empty() || k.is_empty() || m.is_empty() {
                self.push_log("!! AI 配置不完整：API 地址 / Key / 模型都不能为空");
            } else {
                self.cfg.set_ai(&b, &k, &m);
                self.ai_open = false;
                self.push_log(format!(
                    "AI 接口已保存：{}（模型 {m}），config.json 的 ai 段已更新",
                    b
                ));
            }
        }
        if !open {
            self.ai_open = false;
        }
    }

    fn draw_captcha_window(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &self.captcha else {
            return;
        };
        let url = prompt.url.clone();
        let video = prompt.video.clone();
        let left = prompt
            .timeout_sec
            .saturating_sub(prompt.started.elapsed().as_secs());
        let mut close = false;
        let mut resume = false;
        let mut skip = false;
        egui::Window::new("需要验证码")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 40.0])
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("智慧树要求验证码，刷课已暂停").strong().color(
                        if ui.visuals().dark_mode {
                            egui::Color32::from_rgb(235, 165, 70)
                        } else {
                            egui::Color32::from_rgb(150, 85, 10)
                        },
                    ),
                );
                if !video.is_empty() {
                    ui.label(format!("卡在视频：{video}"));
                }
                ui.label("1. 用浏览器打开下面地址（必须同一个账号）");
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&url).monospace().small()).wrap());
                });
                if ui.button("复制地址").clicked() {
                    ui.output_mut(|o| o.copied_text = url.clone());
                }
                if ui.button("用浏览器打开").clicked() {
                    let _ = util::open_path(std::path::Path::new(&url));
                }
                ui.label("2. 完成滑块/图形验证码，保持页面别关");
                ui.label(format!("3. 剩余等待 {left} 秒，解完点下面按钮立刻重试"));
                ui.horizontal(|ui| {
                    if ui.button("我已解完，继续刷课").clicked() {
                        resume = true;
                    }
                    if ui.button("放弃这门课").clicked() {
                        skip = true;
                    }
                    if ui.button("关闭提示").clicked() {
                        close = true;
                    }
                });
            });
        if resume {
            self.ctl.request_resume();
            self.push_log("[风控] 已通知脚本立刻重试");
            ctx.request_repaint();
        }
        if skip {
            self.ctl.request_skip();
            self.captcha = None;
        }
        if close {
            self.captcha = None;
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        // 界面线程里解码二维码 PNG（之前漏了这一步，导致弹窗里一直是空的）
        if let Some(png) = self.pending_qr.take() {
            self.load_qr_texture(ctx, &png);
        }
        self.frames = self.frames.wrapping_add(1);
        if self.rounded {
            // 透明窗口：自己画一张带圆角的底板，四角以外保持透明
            let bg = if self.dark {
                egui::Color32::from_rgb(27, 29, 32)
            } else {
                egui::Color32::from_rgb(250, 250, 252)
            };
            let rect = ctx.screen_rect();
            ctx.layer_painter(egui::LayerId::background())
                .rect_filled(rect, egui::Rounding::same(self.corner), bg);
        }
        // winit 在「显示窗口」时会把 WS_CAPTION 加回来，而 egui 只在需要时才重绘，
        // 所以前几帧连续重申几次，并主动请求重绘，确保样式真正落地。
        if self.frames <= 10 || self.frames % 120 == 0 {
            apply_borderless_native(self.borderless);
            if self.borderless && self.corner > 0.5 && !self.transparent {
                // 没有透明窗口能力时的降级方案：用圆角区域硬裁剪（边缘会有锯齿）
                apply_round_region(self.corner);
            }
            if self.frames <= 10 {
                ctx.request_repaint();
            }
        }
        if self.borderless {
            self.draw_titlebar(ctx);
        }
        self.draw_top(ctx);
        self.draw_left(ctx);
        self.draw_center(ctx);
        self.draw_qr_window(ctx);
        self.draw_ai_window(ctx);
        self.draw_captcha_window(ctx);

        if self.captcha.is_some() || self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

/// 装一个中文可用的字体，否则界面全是方框。
fn install_cjk_font(ctx: &egui::Context) {
    let candidates = [
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/deng.ttf",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simsun.ttc",
    ];
    for path in candidates {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        let data = egui::FontData::from_owned(bytes);
        fonts.font_data.insert("cjk".to_owned(), data);
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "cjk".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("cjk".to_owned());
        ctx.set_fonts(fonts);
        return;
    }
}

/// 系统 DPI 缩放（125% → 1.25）。取不到时按 1.0 处理。
fn system_scale() -> f32 {
    #[cfg(windows)]
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn GetDpiForSystem() -> u32;
        }
        let dpi = GetDpiForSystem();
        if dpi >= 96 {
            return dpi as f32 / 96.0;
        }
    }
    1.0
}

/// 当前屏幕可用区域的物理像素宽高。
fn screen_px() -> (f32, f32) {
    #[cfg(windows)]
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn GetSystemMetrics(index: i32) -> i32;
        }
        // SM_CXSCREEN = 0, SM_CYSCREEN = 1
        let w = GetSystemMetrics(0) as f32;
        let h = GetSystemMetrics(1) as f32;
        if w > 0.0 && h > 0.0 {
            return (w, h);
        }
    }
    (1280.0, 800.0)
}

/// 在创建窗口前声明 per-monitor v2 DPI 感知。
///
/// 不这么做的话，winit 建窗时拿到的缩放是 1.0，`with_inner_size(900)` 会被当成
/// 900 物理像素；窗口建好后 DPI 变成 1.25，egui 按点排版 → 内容比窗口宽，右侧被裁。
fn enable_dpi_awareness() {
    #[cfg(windows)]
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn SetProcessDpiAwarenessContext(value: isize) -> i32;
        }
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4
        let _ = SetProcessDpiAwarenessContext(-4);
    }
}

/// 直接改窗口样式去掉系统标题栏。
///
/// `with_decorations(false)` / `ViewportCommand::Decorations(false)` 在本机
/// winit 0.30 上没有实际效果（GW L_STYLE 里 WS_CAPTION 仍在），所以改用原生调用：
/// 去掉 WS_CAPTION / WS_THICKFRAME，再 SWP_FRAMECHANGED 让系统重算非客户区。
fn apply_borderless_native(borderless: bool) {
    #[cfg(windows)]
    unsafe {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "user32")]
        extern "system" {
            fn FindWindowW(class: *const u16, title: *const u16) -> isize;
            fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
            fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
            fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
        }

        const GWL_STYLE: i32 = -16;
        const WS_CAPTION: isize = 0x00C0_0000;
        const WS_THICKFRAME: isize = 0x0004_0000;
        const WS_SYSMENU: isize = 0x0008_0000;
        const WS_MINIMIZEBOX: isize = 0x0002_0000;
        const WS_MAXIMIZEBOX: isize = 0x0001_0000;
        const SWP_NOMOVE: u32 = 0x0002;
        const SWP_NOSIZE: u32 = 0x0001;
        const SWP_NOZORDER: u32 = 0x0004;
        const SWP_FRAMECHANGED: u32 = 0x0020;

        let title: Vec<u16> = OsStr::new("ZHS-Brush-RUST")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd == 0 {
            return;
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let new_style = if borderless {
            // 无边框：去掉标题栏与可拖拽边框，但保留系统菜单/最小化最大化的行为位
            (style & !(WS_CAPTION | WS_THICKFRAME)) | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX
        } else {
            style | WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX
        };
        if new_style != style {
            SetWindowLongPtrW(hwnd, GWL_STYLE, new_style);
            SetWindowPos(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
    }
}

/// 没有透明窗口时的圆角做法：`SetWindowRgn` + `CreateRoundRectRgn` 硬裁剪。
/// 边缘会有锯齿，但兼容性最好（老驱动/远程桌面）。
fn apply_round_region(radius: f32) {
    #[cfg(windows)]
    unsafe {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "user32")]
        extern "system" {
            fn FindWindowW(class: *const u16, title: *const u16) -> isize;
            fn GetWindowRect(hwnd: isize, rect: *mut [i32; 4]) -> i32;
            fn SetWindowRgn(hwnd: isize, rgn: isize, redraw: i32) -> i32;
        }
        #[link(name = "gdi32")]
        extern "system" {
            fn CreateRoundRectRgn(x1: i32, y1: i32, x2: i32, y2: i32, w: i32, h: i32) -> isize;
        }

        let title: Vec<u16> = OsStr::new("ZHS-Brush-RUST")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd == 0 {
            return;
        }
        let mut rc = [0i32; 4];
        if GetWindowRect(hwnd, &mut rc) == 0 {
            return;
        }
        let (w, h) = (rc[2] - rc[0], rc[3] - rc[1]);
        // 半径按 125% 缩放折算成物理像素
        let scale = 1.25_f32;
        let r = (radius * scale) as i32 * 2;
        let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, r, r);
        if rgn != 0 {
            SetWindowRgn(hwnd, rgn, 1);
        }
    }
}
