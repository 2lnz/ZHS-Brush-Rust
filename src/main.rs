//! 命令行入口：`zhs-brush <gui|login|fetch|run>`。

use anyhow::Result;
use clap::{Parser, Subcommand};
use zhs_brush::config::{Config, RunOpts};
use zhs_brush::events::{Control, Ev, Sink};
use zhs_brush::model::CourseRow;
use zhs_brush::study::Fucker;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "zhs-brush",
    version,
    about = "智慧树/知到 课程管理与刷课（Rust 重写版）",
    long_about = None
)]
struct Cli {
    /// 数据目录（放 config.json / cookies.json / qr / logs），默认 exe 同目录
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// 打开图形界面（默认）
    Gui,
    /// 扫码登录并保存 cookies.json
    Login,
    /// 拉取课程清单，写入 execution.json
    Fetch,
    /// 按清单或指定 ID 刷课
    Run {
        /// 指定课程 ID（共享课 secret，可多个）；省略则读 execution.json
        #[arg(short = 'c', long = "course", value_name = "ID")]
        courses: Vec<String>,
        /// 每门课限时（分钟），0 表示不限
        #[arg(short = 'l', long = "limit", value_name = "MIN")]
        limit: Option<u64>,
        /// 播放倍速，默认 1.5
        #[arg(long, value_name = "SPEED")]
        speed: Option<f64>,
        /// 结束阈值，默认 0.91
        #[arg(long = "end", value_name = "RATIO")]
        end_thre: Option<f64>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dir = match cli.data_dir {
        Some(d) => {
            // 批处理里 "%~dp0" 的结尾反斜杠会把后面那个引号转义掉，
            // 目录名里混进 `"` 后所有文件操作都会报 os error 123
            let d = zhs_brush::util::normalize_dir(&d);
            std::fs::create_dir_all(&d).ok();
            d
        }
        None => zhs_brush::util::data_dir(),
    };
    std::env::set_var("ZHS_HOME", &dir);

    match cli.cmd.unwrap_or(Cmd::Gui) {
        Cmd::Gui => {
            // eframe::Error 不是 Send+Sync，anyhow 收不了，手动转一层
            zhs_brush::gui::run(dir).map_err(|e| anyhow::anyhow!("界面启动失败: {e}"))?;
            Ok(())
        }
        Cmd::Login => cmd_login(dir),
        Cmd::Fetch => cmd_fetch(dir),
        Cmd::Run {
            courses,
            limit,
            speed,
            end_thre,
        } => cmd_run(dir, courses, limit, speed, end_thre),
    }
}

/// 终端用的事件接收器：日志直接打印，进度条原地刷新。
fn cli_sink() -> Sink {
    Arc::new(|ev: Ev| match ev {
        Ev::Log(line) => println!("{line}"),
        Ev::Status(s) => println!(":: {s}"),
        Ev::Qr(_) => {
            println!("二维码已生成，见 qr/ 目录（本终端不渲染图片）");
        }
        Ev::Progress { action, cur, end } => {
            let pct = if end > 0.0 { cur / end * 100.0 } else { 0.0 };
            print!(
                "\r  {}  {:>5.1}%  {} / {}        ",
                action,
                pct,
                zhs_brush::util::hms(cur),
                zhs_brush::util::hms(end)
            );
            let _ = std::io::stdout().flush();
        }
        Ev::Captcha { url, video, .. } => {
            println!("\n## 智慧树要求验证码，已暂停");
            if !video.is_empty() {
                println!("## 卡在视频：{video}");
            }
            println!("## 浏览器打开（同一个账号）：{url}");
            println!("## 终端里输入 go 回车 = 我已解完；输入 skip = 放弃");
        }
        Ev::CaptchaOk => println!("## 验证码已解除，继续"),
        Ev::CaptchaTimeout => println!("## 验证码等待结束"),
        Ev::Courses(rows) => println!(":: 共 {} 门课", rows.len()),
        Ev::LoginState(ok) => println!(":: 登录状态：{}", if ok { "已保存" } else { "未登录" }),
        Ev::Done(res) => match res {
            Ok(msg) => println!("\n== {msg}"),
            Err(msg) => println!("\n!! {msg}"),
        },
    })
}

/// 让终端可以喊 `go` / `skip` 的 stdin 监听线程。
fn spawn_stdin_watcher(ctl: Arc<Control>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            if stdin.read_line(&mut line).is_err() {
                return;
            }
            match line.trim().to_lowercase().as_str() {
                "go" => ctl.request_resume(),
                "skip" => ctl.request_skip(),
                "stop" | "q" => ctl.request_stop(),
                _ => {}
            }
        }
    });
}

fn cmd_login(dir: PathBuf) -> Result<()> {
    let cfg = Config::load(&dir);
    let ctl = Control::new();
    spawn_stdin_watcher(ctl.clone());
    let mut fucker = Fucker::new(&cfg, RunOpts::default(), cli_sink(), ctl)?;
    fucker.login_qr(&cfg)?;
    println!("登录成功，cookies.json 在 {}", cfg.cookies_path().display());
    Ok(())
}

fn cmd_fetch(dir: PathBuf) -> Result<()> {
    let cfg = Config::load(&dir);
    let ctl = Control::new();
    let mut fucker = Fucker::new(&cfg, RunOpts::default(), cli_sink(), ctl)?;
    let rows = fucker.fetch_courses()?;
    let list: Vec<CourseRow> = rows.clone();
    let path = cfg.execution_path();
    std::fs::write(&path, serde_json::to_string_pretty(&list)?)?;
    for row in &rows {
        println!("  [{}] {}  {}", row.kind.label(), row.name, row.id);
    }
    println!("已写入 {}", path.display());
    Ok(())
}

fn cmd_run(
    dir: PathBuf,
    courses: Vec<String>,
    limit: Option<u64>,
    speed: Option<f64>,
    end_thre: Option<f64>,
) -> Result<()> {
    let cfg = Config::load(&dir);
    let mut opts = RunOpts {
        speed,
        limit_min: limit.unwrap_or_else(|| cfg.limit_min()),
        end_thre: end_thre.unwrap_or(0.91),
    };
    if opts.end_thre <= 0.0 {
        opts.end_thre = 0.91;
    }

    let ids: Vec<String> = if courses.is_empty() {
        let path = cfg.execution_path();
        if !path.exists() {
            anyhow::bail!(
                "没有指定课程，且 {} 不存在；先跑 `zhs-brush fetch`",
                path.display()
            );
        }
        let text = std::fs::read_to_string(&path)?;
        let rows: Vec<CourseRow> = serde_json::from_str(&text)?;
        rows.into_iter().map(|r| r.id).collect()
    } else {
        courses
    };

    let ctl = Control::new();
    spawn_stdin_watcher(ctl.clone());
    let mut fucker = Fucker::new(&cfg, opts.clone(), cli_sink(), ctl)?;
    if !fucker.has_cookies() {
        anyhow::bail!("还没有登录，先跑 `zhs-brush login`");
    }
    println!("准备刷 {} 门课，倍速 {}", ids.len(), opts.speed.unwrap_or(1.5));
    let results = fucker.run_courses(&ids);
    let failed = results.iter().filter(|(_, r)| r.is_err()).count();
    println!("\n完成：{} 门成功，{} 门失败", results.len() - failed, failed);
    Ok(())
}
