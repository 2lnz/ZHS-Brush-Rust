//! 共享课平时测试（作业）：界面与命令行共用的业务流程。
//!
//! 协议细节见 [`crate::api::exam`]；这里只做编排：
//! 载入配置/凭证 → 开卷 → 校验题型 → 生成答案 → 保存读回 → 交卷，
//! 所有进展通过 `report` 回调上报（GUI 里就是日志行）。

use crate::api::ai::{solver_from_config, AiSolver};
use crate::api::exam::{
    id_string, is_submitted, paper_fingerprint, paper_info, prepare_questions, question_type,
    validate_choice, ExamClient, ExamError, ExamResult, HomeworkIdentity,
};
use crate::config::Config;
use crate::cookies::CookieJar;
use crate::http::Http;
use crate::util;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub type Report<'a> = &'a mut dyn FnMut(String);

/// 载入凭证并创建会话（优先 shared_test_cookies.json，其次 cookies.json）。
pub fn open_session(dir: &Path) -> ExamResult<(Config, Http)> {
    let cfg = Config::load(dir);
    let mut http = Http::new(&cfg.proxies())
        .map_err(|e| ExamError::Failed(format!("HTTP 客户端初始化失败: {e}")))?;
    ExamClient::prepare_headers(&mut http);

    let preferred = dir.join("shared_test_cookies.json");
    let cookie_path = if preferred.exists() {
        preferred
    } else {
        cfg.cookies_path()
    };
    let text = std::fs::read_to_string(&cookie_path).map_err(|_| {
        ExamError::Failed(format!(
            "找不到 cookies 文件（{}）。先扫码登录一次再跑平时测试",
            cookie_path.display()
        ))
    })?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| ExamError::Failed(format!("cookies 文件无效: {e}")))?;
    http.cookies = CookieJar::from_json(&value);
    Ok((cfg, http))
}

/// 入口：`execute=false` 只读取检查；`batch=true` 处理同课程全部未提交测试。
pub fn run_all(
    dir: &Path,
    identity: HomeworkIdentity,
    execute: bool,
    batch: bool,
    report: Report,
) -> ExamResult<()> {
    // 单实例锁：避免多个本地进程同时交卷
    let lock_path = dir.join(".homework.lock");
    let _lock = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(f) => f,
        Err(_) => {
            return Err(ExamError::Failed(format!(
                "已有平时测试任务在运行（{} 存在）。确认没有任务后删除该文件再试",
                lock_path.display()
            )))
        }
    };
    let result = run_inner(dir, identity, execute, batch, report);
    let _ = std::fs::remove_file(&lock_path);
    result
}

fn run_inner(
    dir: &Path,
    identity: HomeworkIdentity,
    execute: bool,
    batch: bool,
    report: Report,
) -> ExamResult<()> {
    let (cfg, mut http) = open_session(dir)?;
    let state_dir = dir.join("homework");
    let _ = std::fs::create_dir_all(&state_dir);

    // 求解器只在真正要答题时才需要
    let solver = if execute {
        Some(solver_from_config(&cfg)?)
    } else {
        None
    };

    report(format!("凭证已载入（{} 条 cookie）", http.cookies.len()));
    if batch {
        run_batch(&mut http, identity, solver.as_ref(), &state_dir, execute, report)
    } else {
        let mut client = ExamClient::new(identity);
        run_one(&mut http, &mut client, solver.as_ref(), &state_dir, execute, report)
    }
}

/// 单份测试：读取检查 / AI 答题交卷。
pub fn run_one(
    http: &mut Http,
    client: &mut ExamClient,
    solver: Option<&AiSolver>,
    state_dir: &Path,
    execute: bool,
    report: Report,
) -> ExamResult<()> {
    report("正在读取测试试卷…".into());
    let state = client.open(http)?;
    let (name, base, _count) = paper_info(&state);
    if is_submitted(&state) {
        let score = state.get("score").cloned().unwrap_or(json!("未知"));
        report(format!("「{name}」已经提交过（平台成绩: {score}），跳过"));
        return Ok(());
    }
    let questions = prepare_questions(&base)?;
    report(format!("已读取「{name}」，共 {} 题（单选/多选/判断，全部可处理）", questions.len()));

    let fingerprint = paper_fingerprint(&questions);
    if !execute {
        let inspection = json!({
            "name": name,
            "totalScore": base.get("totalScore").cloned().unwrap_or(Value::Null),
            "questionCount": questions.len(),
            "fingerprint": fingerprint,
            "status": "inspected",
        });
        let path = state_dir.join(format!("{}.inspection.json", client.identity().stu_exam_id));
        atomic_json(&path, &inspection);
        report(format!("读取检查完成（未写服务器任何数据）：{}", path.display()));
        return Ok(());
    }

    let solver = solver.ok_or_else(|| ExamError::Failed("未提供 AI 求解器".into()))?;
    let checkpoint_path = state_dir.join(format!("{}.json", client.identity().stu_exam_id));
    let mut progress = load_checkpoint(&checkpoint_path, &fingerprint);
    let mut drafts: Vec<Value> = progress
        .get("drafts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // 读回服务器已有答案（复用已保存的，选项顺序变化仍按实际 ID 对应）
    let eids: Vec<String> = questions
        .iter()
        .map(|q| id_string(q.get("eid").unwrap()))
        .collect();
    let server_answers = client.answers(http, &eids)?;

    // 1) 全卷先生成完毕，再开始保存
    for (index, q) in questions.iter().enumerate() {
        let eid = id_string(q.get("eid").unwrap());
        if drafts
            .iter()
            .any(|d| d.get("eid").map(id_string) == Some(eid.clone()))
        {
            report(format!("第 {}/{} 题：草稿已存在，跳过生成", index + 1, questions.len()));
            continue;
        }
        let saved_text = match server_answers.get(&eid).and_then(|e| e.get("answer")) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(v) => v.to_string(),
        };
        let answer = if !saved_text.is_empty() {
            match validate_choice(q, &saved_text.split(',').map(String::from).collect::<Vec<_>>()) {
                Ok(v) => {
                    report(format!("第 {}/{} 题：复用服务器已保存答案", index + 1, questions.len()));
                    v
                }
                Err(_) => {
                    report(format!("第 {}/{} 题：AI 正在生成答案…", index + 1, questions.len()));
                    validate_choice(q, &solver.solve(http, q)?)?
                }
            }
        } else {
            report(format!("第 {}/{} 题：AI 正在生成答案…", index + 1, questions.len()));
            validate_choice(q, &solver.solve(http, q)?)?
        };
        let mut record = client.identity().params();
        record.remove("courseId");
        record.insert("eid".into(), json!(eid));
        record.insert("questionType".into(), json!(question_type(q).unwrap_or(0)));
        record.insert("answer".into(), json!(answer));
        record.insert("dataIds".into(), json!(""));
        record.insert("deviceId".into(), json!(""));
        record.insert("examType".into(), json!(""));
        record.insert("fromType".into(), json!(3));
        drafts.push(Value::Object(record));
        progress.insert("drafts".into(), json!(drafts));
        progress.insert("status".into(), json!("drafting"));
        atomic_json(&checkpoint_path, &Value::Object(progress.clone()));
    }

    // 2) 逐题保存 + 读回核验（风控时暂停等人工验证）
    let mut saved_count = 0usize;
    for record in &drafts {
        if !client.matches(record, &server_answers) {
            report(format!("保存并读回核验 {}/{} …", saved_count + 1, drafts.len()));
            save_with_verify_retry(http, client, record, report)?;
            // 连续保存太快容易被判机器行为，按真人节奏留出间隔
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        saved_count += 1;
        progress.insert("status".into(), json!("saving"));
        progress.insert("saved".into(), json!(saved_count));
        atomic_json(&checkpoint_path, &Value::Object(progress.clone()));
    }

    // 3) 交卷
    report("全部答案已读回核验，正在交卷…".into());
    progress.insert("status".into(), json!("submitting"));
    atomic_json(&checkpoint_path, &Value::Object(progress.clone()));
    let submission = client.submit(http, &drafts, questions.len())?;
    progress.insert("status".into(), json!("submitted"));
    progress.insert("submission".into(), submission.clone());
    atomic_json(&checkpoint_path, &Value::Object(progress.clone()));
    let score = submission.get("score").cloned().unwrap_or(json!("以平台为准"));
    let via = submission
        .get("verifiedBy")
        .and_then(Value::as_str)
        .unwrap_or("");
    report(format!("已交卷：{name}，服务器成绩 {score}（确认方式: {via}）"));
    report("成绩以智慧树平台为准；已提交的试卷不会自动重做".into());
    Ok(())
}

/// 批量：先显式开卷认证，再列出同课程全部未提交测试逐一处理。
pub fn run_batch(
    http: &mut Http,
    identity: HomeworkIdentity,
    solver: Option<&AiSolver>,
    state_dir: &Path,
    execute: bool,
    report: Report,
) -> ExamResult<()> {
    let mut client = ExamClient::new(identity);
    client.open(http)?;
    let items = client.list_homeworks(http)?;
    report(format!("发现 {} 份未提交的普通共享课平时测试", items.len()));
    if items.is_empty() {
        return Ok(());
    }
    let recruit = client.identity().recruit_id.clone();
    let course = client.identity().course_id.clone();
    let school = client.identity().school_id.clone();

    for (index, item) in items.iter().enumerate() {
        let name = item
            .get("examName")
            .and_then(Value::as_str)
            .unwrap_or("未命名测试")
            .to_string();
        let sub = HomeworkIdentity {
            recruit_id: recruit.clone(),
            stu_exam_id: id_string(item.get("id").unwrap()),
            exam_id: id_string(item.get("examId").unwrap()),
            course_id: course.clone(),
            school_id: school.clone(),
            meet_course_type: "0".into(),
        };
        report(format!("===== 第 {}/{} 份：{name} =====", index + 1, items.len()));
        let mut target = ExamClient::new(sub);
        target.reuse_key(&client); // 同课程批量复用已交换的 AES 密钥
        run_one(http, &mut target, solver, state_dir, execute, report)?;
    }
    Ok(())
}

/// 保存一题；遇平台风控（滑块验证）时自动打开测试页并暂停 60 秒重试。
fn save_with_verify_retry(
    http: &mut Http,
    client: &mut ExamClient,
    record: &Value,
    report: Report,
) -> ExamResult<()> {
    let mut tries = 0u32;
    loop {
        match client.save(http, record) {
            Ok(()) => return Ok(()),
            Err(ExamError::NeedVerify { msg, url }) => {
                if tries >= 3 {
                    return Err(ExamError::Failed(format!(
                        "{msg}（已自动等待 3 分钟仍被拦截：请稍后重试，或重新扫码登录换一个新会话）"
                    )));
                }
                tries += 1;
                report(format!(
                    "[风控] {msg}；已自动打开测试页，暂停 60 秒等待你完成滑块（第 {tries}/3 次）…"
                ));
                let _ = crate::util::open_path(Path::new(&url));
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
            Err(ExamError::LoginRequired(m)) => {
                return Err(ExamError::LoginRequired(m));
            }
            Err(e) => return Err(e),
        }
    }
}

fn load_checkpoint(path: &Path, fingerprint: &Value) -> Map<String, Value> {
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(Value::Object(prev)) = serde_json::from_str::<Value>(&text) {
            // 指纹一致才复用旧草稿（题干/选项变化不套用旧答案）
            if prev.get("fingerprint") == Some(fingerprint) {
                return prev;
            }
        }
    }
    let mut m = Map::new();
    m.insert("drafts".into(), json!([]));
    m.insert("fingerprint".into(), fingerprint.clone());
    m
}

fn atomic_json(path: &Path, value: &Value) {
    let tmp = path.with_extension("json.tmp");
    if let Ok(text) = serde_json::to_string_pretty(value) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}
