//! 刷课主流程，对应 Python `Fucker.getZhidaoContext` /
//! `fuckZhidaoCourse` / `fuckZhidaoVideo` / `waitCaptcha`。
//!
//! 与 Python 版保持一致的关键行为：
//! - 视频进度按「秒」推进，每秒一 tick，倍速决定推进量；
//! - 每 30 秒（或遇到弹题、快到结尾）上报一次 `saveDatabaseIntervalTimeV2`；
//! - 弹题在 `timeSec` 到达时先假装暂停、延迟几拍再作答；
//! - code -12 视为风控，暂停等人工处理，解开后重刷当前视频，而不是丢掉整门课。

use crate::api::{passport, zhidao, ApiError};
use crate::config::{Config, RunOpts};
use crate::crypto::WatchPoint;
use crate::events::{Control, Ev, Sink};
use crate::http::Http;
use crate::model::{arr_at, f64_at, i64_at, str_at, CourseCtx, CourseKind, CourseRow};
use crate::util::{self, hms};
use anyhow::anyhow;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use rand::Rng;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StudyError {
    #[error("需要验证码（风控 code -12）")]
    Captcha,
    #[error("已超过限时 {0} 分钟")]
    TimeLimit(u64),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<ApiError> for StudyError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Captcha => StudyError::Captcha,
            ApiError::Code { code, msg } => {
                StudyError::Other(anyhow!("接口返回 code={code} msg={msg}"))
            }
            ApiError::Other(e) => StudyError::Other(e),
        }
    }
}

pub type StudyResult<T> = Result<T, StudyError>;

/// 一门课一个实例，负责会话 + 上下文 + 刷课动作。
pub struct Fucker {
    pub http: Http,
    pub opts: RunOpts,
    pub captcha_cfg: crate::config::CaptchaCfg,
    pub tree_view: bool,
    pub log_dir: PathBuf,
    pub uuid: String,
    sink: Sink,
    ctl: Arc<Control>,
    contexts: HashMap<String, CourseCtx>,
    /// 是否顺带上报缓存接口（原版默认关闭）
    pub report_cache: bool,
}

impl Fucker {
    pub fn new(
        cfg: &Config,
        opts: RunOpts,
        sink: Sink,
        ctl: Arc<Control>,
    ) -> anyhow::Result<Self> {
        let http = Http::new(&cfg.proxies())?;
        let mut me = Self {
            http,
            opts,
            captcha_cfg: cfg.captcha(),
            tree_view: cfg.tree_view(),
            log_dir: cfg.dir.clone(),
            uuid: String::new(),
            sink,
            ctl,
            contexts: HashMap::new(),
            report_cache: false,
        };
        me.load_cookies(cfg)?;
        Ok(me)
    }

    // ---------------------------------------------------------------- 日志

    fn log(&self, msg: impl Into<String>) {
        let msg = msg.into();
        util::write_log(&self.log_dir, "INFO", &msg);
        (self.sink)(Ev::Log(msg));
    }

    fn warn(&self, msg: impl Into<String>) {
        let msg = msg.into();
        util::write_log(&self.log_dir, "WARN", &msg);
        (self.sink)(Ev::Log(format!("[警告] {msg}")));
    }

    fn emit(&self, ev: Ev) {
        (self.sink)(ev);
    }

    // ---------------------------------------------------------------- cookie / 登录态

    pub fn load_cookies(&mut self, cfg: &Config) -> anyhow::Result<()> {
        let path = cfg.cookies_path();
        if !path.exists() {
            return Ok(());
        }
        let text = std::fs::read_to_string(&path)?;
        let value: Value = serde_json::from_str(&text)?;
        self.http.cookies = crate::cookies::CookieJar::from_json(&value);
        if self.http.cookies.is_empty() {
            return Ok(());
        }
        match self.http.cookies.normalize_for_session() {
            Ok(uuid) => {
                self.uuid = uuid;
                self.log("已读取本地登录凭证；有效性由课程接口验证。");
                self.emit(Ev::LoginState(true));
            }
            Err(e) => {
                self.warn(format!("本地凭证无法使用：{e}"));
                self.http.cookies = crate::cookies::CookieJar::new();
            }
        }
        Ok(())
    }

    pub fn has_cookies(&self) -> bool {
        !self.http.cookies.is_empty()
    }

    pub fn save_cookies(&self, cfg: &Config) -> anyhow::Result<()> {
        self.http.save_cookies(&cfg.cookies_path())
    }

    pub fn clear_cookies(&mut self, cfg: &Config) -> anyhow::Result<()> {
        self.http.cookies = crate::cookies::CookieJar::new();
        self.uuid.clear();
        let path = cfg.cookies_path();
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        self.emit(Ev::LoginState(false));
        Ok(())
    }

    /// 扫码登录：拿到二维码就回调（写入 qr/ 供界面显示），之后轮询到成功或过期。
    pub fn login_qr(&mut self, cfg: &Config) -> anyhow::Result<()> {
        self.log("=== 开始扫码登录 ===");
        let mut rounds = 0u32;
        loop {
            rounds += 1;
            if self.ctl.should_stop() {
                anyhow::bail!("用户中止登录");
            }
            let (token, png) = passport::qr_image(&mut self.http)?;
            // 落盘一份，方便「打开二维码文件夹」和排查
            self.dump_qr(cfg, &png);
            self.emit(Ev::Qr(png));
            self.log(format!("[二维码 {rounds}] 请用知到 App 扫码：知到 → 我的 → 扫一扫"));

            let deadline = Instant::now() + Duration::from_secs(180);
            let mut scanned_logged = false;
            loop {
                if Instant::now() > deadline {
                    self.warn("二维码 180 秒未确认，自动刷新");
                    break;
                }
                if self.ctl.should_stop() {
                    anyhow::bail!("用户中止登录");
                }
                std::thread::sleep(Duration::from_millis(800));
                match passport::qr_poll(&mut self.http, &token)? {
                    passport::QrState::Waiting => {}
                    passport::QrState::Scanned(msg) => {
                        if !scanned_logged {
                            scanned_logged = true;
                            self.log(format!("手机已扫码{}{}，请在手机上点确认", if msg.is_empty() { "" } else { "：" }, msg));
                        }
                    }
                    passport::QrState::Success(once) => {
                        passport::finish_qr(&mut self.http, &once)?;
                        self.uuid = self.http.cookies.normalize_for_session()?;
                        if cfg.save_cookies() {
                            // 落盘失败不该让整次登录白费：内存里的会话仍然可用
                            if let Err(e) = self.save_cookies(cfg) {
                                self.warn(format!(
                                    "cookies.json 保存失败（本次仍可继续使用）：{e}"
                                ));
                            } else {
                                self.log(format!(
                                    "登录成功，cookies.json 已保存到 {}",
                                    cfg.cookies_path().display()
                                ));
                            }
                        }
                        self.emit(Ev::LoginState(true));
                        return Ok(());
                    }
                    passport::QrState::Expired => {
                        self.warn("二维码已过期，重新获取");
                        break;
                    }
                    passport::QrState::Canceled => {
                        anyhow::bail!("手机端取消了登录");
                    }
                }
            }
            if rounds >= 5 {
                anyhow::bail!("二维码连续刷新 5 次仍未登录成功");
            }
        }
    }

    fn dump_qr(&self, cfg: &Config, png: &[u8]) {
        let dir = cfg.qr_dir();
        if util::ensure_dir(&dir).is_err() {
            return;
        }
        let name = format!("qr_{}.png", stamp());
        let path = dir.join(name);
        if std::fs::write(&path, png).is_ok() {
            // 只保留最近 10 张
            if let Ok(rd) = std::fs::read_dir(&dir) {
                let mut files: Vec<PathBuf> = rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().map(|e| e == "png").unwrap_or(false))
                    .collect();
                files.sort();
                while files.len() > 10 {
                    if let Some(old) = files.first().cloned() {
                        let _ = std::fs::remove_file(&old);
                        files.remove(0);
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- 课程清单

    /// 共享课清单（翻转课在 hike 接口，二期再补）。
    pub fn fetch_courses(&mut self) -> anyhow::Result<Vec<CourseRow>> {
        if !self.has_cookies() {
            anyhow::bail!("还没有登录，先扫码登录");
        }
        let mut rows = zhidao::get_zhidao_list(&mut self.http)
            .map_err(|e| anyhow!("拉取共享课清单失败: {e}"))?;
        self.log(format!("共享课 {} 门", rows.len()));
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(rows)
    }

    /// 拉课程上下文（章/节/视频/进度）。
    pub fn get_context(&mut self, rac_id: &str) -> StudyResult<CourseCtx> {
        if let Some(ctx) = self.contexts.get(rac_id) {
            return Ok(ctx.clone());
        }
        if !self.has_cookies() {
            return Err(StudyError::Other(anyhow!("还没有登录，先扫码登录")));
        }
        self.log(format!("读取课程上下文 {rac_id}"));
        self.http.set_study_headers();
        zhidao::gologin(&mut self.http, rac_id)?;

        let course = zhidao::query_course(&mut self.http, rac_id)?;
        let recruit_id = i64_at(&course, "recruitId");
        let chapters_json = zhidao::video_list(&mut self.http, rac_id)?;
        let mut parsed = crate::model::parse_chapters(&chapters_json);
        let mut course_id = parsed.course_id;
        if course_id == 0 {
            course_id = i64_at(
                course.get("courseInfo").unwrap_or(&Value::Null),
                "courseId",
            );
        }
        parsed.course_id = course_id;
        let name = {
            let info = course.get("courseInfo").unwrap_or(&Value::Null);
            let a = str_at(info, "name");
            if a.is_empty() {
                str_at(info, "enName")
            } else {
                a
            }
        };

        let chapters = parsed.chapters;
        let lesson_ids = parsed.lesson_ids;
        let mut videos = parsed.videos;

        // 进度
        let small_ids: Vec<i64> = videos.values().filter(|v| v.small_id != 0).map(|v| v.small_id).collect();
        if !lesson_ids.is_empty() {
            match zhidao::query_study_info(&mut self.http, &lesson_ids, &small_ids, recruit_id) {
                Ok(states) => {
                    let lv = states.get("lv").cloned().unwrap_or(Value::Null);
                    let lesson_states = states.get("lesson").cloned().unwrap_or(Value::Null);
                    for video in videos.values_mut() {
                        let key = video.small_id.to_string();
                        let mut st = lv.get(&key).cloned().unwrap_or(Value::Null);
                        if st.is_null() || st.get("watchState").is_none() {
                            st = lesson_states
                                .get(video.lesson_id.to_string())
                                .cloned()
                                .unwrap_or(Value::Null);
                        }
                        video.watch_state = i64_at(&st, "watchState");
                        video.study_total_time = f64_at(&st, "studyTotalTime");
                        if video.study_total_time <= 0.0 {
                            // 少数返回里字段叫 totalStudyTime
                            video.study_total_time = f64_at(&st, "totalStudyTime");
                        }
                    }
                }
                Err(ApiError::Captcha) => {
                    self.warn("查询学习进度时被风控拦下（code -12）");
                }
                Err(e) => self.warn(format!("查询学习进度失败: {e}")),
            }
        }
        // 原版还会请求「最近观看视频」，对结果没影响，失败也忽略
        let _ = zhidao::query_last_video(&mut self.http, recruit_id);

        let ctx = CourseCtx {
            rac_id: rac_id.to_string(),
            name,
            course_id,
            recruit_id,
            chapters,
            videos,
            fucked_time: 0,
        };
        self.log(format!(
            "课程「{}」：{} 章 / {} 视频",
            ctx.name,
            ctx.chapters.len(),
            ctx.video_count()
        ));
        self.contexts.insert(rac_id.to_string(), ctx.clone());
        Ok(ctx)
    }

    // ---------------------------------------------------------------- 刷课

    /// 按清单顺序刷，返回每条的结果描述。
    pub fn run_courses(&mut self, ids: &[String]) -> Vec<(String, anyhow::Result<()>)> {
        let mut out = Vec::new();
        for id in ids {
            if self.ctl.should_stop() {
                self.warn("收到停止指令，结束");
                break;
            }
            let kind = if id.chars().any(|c| c.is_ascii_alphabetic()) {
                CourseKind::Zhidao
            } else {
                CourseKind::Hike
            };
            let res = match kind {
                CourseKind::Zhidao => self.fuck_zhidao_course(id),
                CourseKind::Hike => Err(anyhow!(
                    "翻转课（hike）接口本期未移植，课程 {id} 已跳过"
                )),
            };
            match &res {
                Ok(()) => self.log(format!("课程 {id} 处理完成")),
                Err(e) => self.warn(format!("课程 {id} 失败：{e}")),
            }
            out.push((id.clone(), res));
        }
        out
    }

    pub fn fuck_zhidao_course(&mut self, rac_id: &str) -> anyhow::Result<()> {
        self.log(format!("开始刷共享课 {rac_id}"));
        let ctx = self.get_context(rac_id).map_err(|e| anyhow!("{e}"))?;
        let begin = Instant::now();
        self.log(format!(
            "课程：{}（recruitId={}）",
            if ctx.name.is_empty() { rac_id } else { &ctx.name },
            ctx.recruit_id
        ));

        for chapter in ctx.chapters.clone() {
            if self.ctl.should_stop() {
                return Ok(());
            }
            if self.tree_view {
                self.log(format!("  |__ 章：{}", chapter.name));
            }
            for lesson in chapter.lessons.clone() {
                if self.tree_view {
                    self.log(format!("  |  |__ 节：{}", lesson.name));
                }
                for video in lesson.videos.clone() {
                    if self.ctl.should_stop() {
                        return Ok(());
                    }
                    if self.tree_view {
                        self.log(format!("  |  |  |__ 视频：{}", video.name));
                    }
                    let mut captcha_rounds = 0u32;
                    loop {
                        match self.fuck_zhidao_video(rac_id, &video.video_id) {
                            Ok(()) => break,
                            Err(StudyError::TimeLimit(min)) => {
                                self.log(format!("已达到限时 {min} 分钟，停止本课程"));
                                return Ok(());
                            }
                            Err(StudyError::Captcha) => {
                                captcha_rounds += 1;
                                if captcha_rounds <= self.captcha_cfg.max_retry
                                    && self.wait_captcha(rac_id, &video.name)
                                {
                                    self.log(format!(
                                        "验证码已解除，重刷视频「{}」（第 {captcha_rounds} 轮）",
                                        video.name
                                    ));
                                    continue;
                                }
                                self.warn("验证码未解决，本课程停止");
                                return Ok(());
                            }
                            Err(StudyError::Other(e)) => {
                                self.warn(format!("视频「{}」失败：{e}", video.name));
                                break;
                            }
                        }
                    }
                }
            }
        }
        self.log(format!(
            "课程「{}」处理完毕，耗时 {:.1} 秒",
            if ctx.name.is_empty() { rac_id } else { &ctx.name },
            begin.elapsed().as_secs_f64()
        ));
        Ok(())
    }

    pub fn fuck_zhidao_video(&mut self, rac_id: &str, video_id: &str) -> StudyResult<()> {
        let ctx = self.get_context(rac_id)?;
        self.check_time_limit(&ctx)?;

        let Some(video) = ctx.videos.get(video_id).cloned() else {
            return Err(StudyError::Other(anyhow!("视频 {video_id} 不在课程上下文里")));
        };

        if video.watch_state == 1 && self.opts.end_thre <= 1.0 {
            self.log(format!("视频「{}」已看完，跳过", video.name));
            return Ok(());
        }

        // token：所有上报都要带
        let token = zhidao::prelearning_note(&mut self.http, ctx.course_id, ctx.recruit_id, &video)?;
        let token_b64 = B64.encode(token.to_string());

        // 弹题
        let mut questions = zhidao::load_video_pointer_info(
            &mut self.http,
            ctx.course_id,
            ctx.recruit_id,
            &video,
        )?;
        let mut played_time = video.study_total_time.max(0.0);
        while questions
            .last()
            .map(|q| q.time_sec <= played_time)
            .unwrap_or(false)
        {
            questions.pop();
        }

        let mut end_time = (video.video_sec * self.opts.end_thre).max(1.0);
        if let Some(first) = questions.first() {
            end_time = end_time.max(first.time_sec);
        }

        // 模拟播放器初始化
        if let Err(e) = zhidao::watch_video(&mut self.http, &video.video_id) {
            self.warn(format!("initVideo 失败（不影响刷课）：{e}"));
        }
        let _ = zhidao::three_dimensional_course_ware(&mut self.http, &video.video_id);

        let speed = self.opts.speed.unwrap_or(1.5).max(0.1);
        let mut last_submit = played_time;
        let mut elapsed: u64 = 0;
        let db_interval: u64 = 30;
        // 弹题延迟提交：拿到题后先「假装思考」两拍，再提交（对齐原版 answer=2 的节拍）
        let mut answer_counter: Option<i32> = None;
        let mut pending_answer: Option<(i64, String)> = None;
        let mut report = false;
        let mut pause: i32 = 0;
        let mut wp = WatchPoint::new(played_time as i64);
        let mut rng = rand::thread_rng();

        while played_time < end_time {
            if self.ctl.should_stop() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(1));
            elapsed += 1;
            // 限时按「真实秒」累计
            if let Some(ctx_mut) = self.contexts.get_mut(rac_id) {
                ctx_mut.fucked_time += 1;
            }
            played_time = (played_time + speed).min(end_time);

            // 0.25% 概率假装暂停一分钟，贴近真实观看
            if rng.gen::<f64>() < 0.0025 {
                pause = 60;
                report = true;
            }
            if pause > 0 {
                pause -= 1;
                played_time = last_submit;
            }
            if elapsed % 2 == 0 {
                wp.add(played_time, None);
            }

            // 到点触发弹题：先取题、缓存答案，两拍后再提交
            if let Some(q) = questions.last() {
                if played_time >= q.time_sec {
                    let q = questions.pop();
                    if let Some(q) = q {
                        // 服务端偶尔回 code=-1（系统不可预期错误），重试几次再放弃，
                        // 否则整道弹题就白白漏答了
                        let mut attempt = 0u32;
                        let detail = loop {
                            attempt += 1;
                            match zhidao::lesson_popup_exam(&mut self.http, &video, &q.question_ids) {
                                Ok(d) => break Some(d),
                                Err(ApiError::Captcha) => return Err(StudyError::Captcha),
                                Err(e) => {
                                    if attempt >= 3 {
                                        self.warn(format!(
                                            "取弹题详情失败（已重试 {attempt} 次）：{e}"
                                        ));
                                        break None;
                                    }
                                    self.log(format!(
                                        "取弹题详情失败，1 秒后第 {attempt} 次重试：{e}"
                                    ));
                                    std::thread::sleep(Duration::from_millis(1000));
                                }
                            }
                        };
                        if let Some(detail) = detail {
                            if let Some(question) = extract_question(&detail) {
                                let ans = answer_text(&question);
                                let qid = i64_at(&question, "questionId");
                                pending_answer = Some((qid, ans));
                                answer_counter = Some(2);
                            } else {
                                self.warn("弹题详情里没有 testQuestion，跳过这道题");
                            }
                        }
                        report = true;
                    }
                }
            }

            // 答案延迟两拍再落库（模拟原版「思考」节拍）
            if let Some(counter) = answer_counter {
                if counter == 0 {
                    if let Some((qid, ans)) = pending_answer.take() {
                        match zhidao::save_popup_answer(
                            &mut self.http,
                            ctx.course_id,
                            ctx.recruit_id,
                            &video,
                            qid,
                            &ans,
                        ) {
                            Ok(()) => self.log(format!(
                                "弹题已作答 questionId={qid} answer={ans}"
                            )),
                            Err(ApiError::Captcha) => return Err(StudyError::Captcha),
                            Err(e) => self.warn(format!("保存弹题答案失败：{e}")),
                        }
                    }
                    answer_counter = None;
                    report = true;
                } else {
                    pause = pause.max(1);
                    answer_counter = Some(counter - 1);
                }
            }

            // 上报数据库
            if elapsed % db_interval == 0 || played_time >= end_time || report {
                report = false;
                wp.add(played_time, None);
                if let Err(e) = zhidao::save_database_interval_time_v2(
                    &mut self.http,
                    ctx.course_id,
                    ctx.recruit_id,
                    &video,
                    &self.uuid,
                    played_time,
                    last_submit,
                    &wp.get(),
                    &token_b64,
                ) {
                    match e {
                        ApiError::Captcha => return Err(StudyError::Captcha),
                        other => self.warn(format!("上报进度失败：{other}")),
                    }
                }
                last_submit = played_time;
                wp.reset(played_time as i64);
            }

            if self.report_cache && elapsed % 18 == 0 {
                let _ = zhidao::save_cache_interval_time(
                    &mut self.http,
                    ctx.course_id,
                    ctx.recruit_id,
                    &video,
                    played_time,
                    last_submit,
                    &wp.get(),
                    &token_b64,
                );
            }

            self.emit(Ev::Progress {
                action: if pause > 0 {
                    "暂停中".to_string()
                } else if answer_counter.is_some() {
                    "答题中".to_string()
                } else {
                    format!("刷课 {}", video.name)
                },
                cur: played_time,
                end: end_time,
            });
        }

        self.log(format!(
            "视频「{}」完成：{} / {}",
            video.name,
            hms(played_time),
            hms(end_time)
        ));
        // 原版在结尾还要睡 random()+1 秒
        std::thread::sleep(Duration::from_millis(rng.gen_range(1000..2000)));
        Ok(())
    }

    fn check_time_limit(&self, ctx: &CourseCtx) -> StudyResult<()> {
        if self.opts.limit_min > 0 && ctx.fucked_time >= self.opts.limit_min * 60 {
            return Err(StudyError::TimeLimit(self.opts.limit_min));
        }
        Ok(())
    }

    // ---------------------------------------------------------------- 验证码

    /// 用只读接口试探风控是否解除（不产生学习记录）。
    pub fn captcha_cleared(&mut self, rac_id: &str) -> bool {
        self.http.set_study_headers();
        match zhidao::query_course(&mut self.http, rac_id) {
            Ok(_) => true,
            Err(ApiError::Captcha) => false,
            Err(_) => false,
        }
    }

    /// 暂停刷课等待人工过验证码。返回 true 表示已解除。
    pub fn wait_captcha(&mut self, rac_id: &str, video_name: &str) -> bool {
        if !self.captcha_cfg.wait {
            return false;
        }
        let cfg = self.captcha_cfg;
        let url = zhidao::study_url(rac_id);
        self.warn(format!(
            "智慧树要求验证码，刷课已暂停（视频：{}）",
            if video_name.is_empty() { "未知" } else { video_name }
        ));
        self.log(format!("1. 浏览器打开（必须同一个账号）：{url}"));
        self.log("2. 完成滑块/图形验证码，然后保持页面别关");
        self.log(format!(
            "3. 每 {} 秒自动探测一次；最多等 {} 分 {} 秒",
            cfg.interval,
            cfg.timeout / 60,
            cfg.timeout % 60
        ));
        self.emit(Ev::Captcha {
            url: url.clone(),
            video: video_name.to_string(),
            timeout_sec: cfg.timeout,
        });

        let deadline = Instant::now() + Duration::from_secs(cfg.timeout);
        while Instant::now() < deadline {
            if self.ctl.should_stop() {
                return false;
            }
            // 分段睡眠，界面点了按钮能立刻响应
            let mut waited = 0u64;
            while waited < cfg.interval {
                if self.ctl.should_stop() || self.ctl.take_poke() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
                waited += 1;
                // 半秒粒度
                if waited * 500 >= cfg.interval * 1000 {
                    break;
                }
            }
            if self.ctl.skip.load(std::sync::atomic::Ordering::SeqCst) {
                self.log("收到「放弃」指令，停止等待验证码");
                self.emit(Ev::CaptchaTimeout);
                return false;
            }
            if self.captcha_cleared(rac_id) {
                self.log("验证码已解除，继续刷课");
                self.emit(Ev::CaptchaOk);
                self.ctl.reset();
                return true;
            }
            let left = deadline
                .saturating_duration_since(Instant::now())
                .as_secs();
            self.log(format!("仍需要验证码，继续等待（剩余 {left} 秒）"));
        }
        self.warn("等待验证码超时");
        self.emit(Ev::CaptchaTimeout);
        false
    }
}

// ---------------------------------------------------------------- 弹题解析

/// 从 `lessonPopupExam` 的返回里挖出 `testQuestion`。
fn extract_question(detail: &Value) -> Option<Value> {
    let arr = arr_at(detail, "lessonTestQuestionUseInterfaceDtos");
    let first = arr.first()?;
    let q = first.get("testQuestion")?.clone();
    if q.is_null() {
        None
    } else {
        Some(q)
    }
}

/// 与原版 `answerZhidao` 一致：挑 `result == '1'` 的选项 id，逗号拼接。
pub fn answer_text(question: &Value) -> String {
    arr_at(question, "questionOptions")
        .iter()
        .filter(|opt| {
            let r = str_at(opt, "result");
            r == "1" || r == "true"
        })
        .map(|opt| i64_at(opt, "id").to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = util::civil_from_unix(now);
    format!("{y:04}{mo:02}{d:02}_{h:02}{mi:02}{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn answer_picks_correct_options() {
        let q = json!({
            "questionId": 11,
            "questionOptions": [
                {"id": 101, "result": "0"},
                {"id": 102, "result": "1"},
                {"id": 103, "result": "1"}
            ]
        });
        assert_eq!(answer_text(&q), "102,103");
    }

    #[test]
    fn extract_question_shape() {
        let detail = json!({
            "lessonTestQuestionUseInterfaceDtos": [
                {"testQuestion": {"questionId": 7, "questionOptions": []}}
            ]
        });
        let q = extract_question(&detail).unwrap();
        assert_eq!(i64_at(&q, "questionId"), 7);
    }
}
