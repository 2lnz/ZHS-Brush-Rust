//! 诊断：现有 cookies 对视频服务 / 测试服务是否仍然有效。
//! 用法：probe_session.exe <data_dir>

use serde_json::Value;
use zhs_brush::api::{exam, zhidao};
use zhs_brush::config::Config;
use zhs_brush::cookies::CookieJar;
use zhs_brush::http::Http;
use zhs_brush::util;

fn main() -> anyhow::Result<()> {
    let dir = std::env::args()
        .nth(1)
        .map(|d| util::normalize_dir(std::path::Path::new(&d)))
        .unwrap_or_else(util::data_dir);
    let cfg = Config::load(&dir);
    let mut http = Http::new(&cfg.proxies())?;
    let text = std::fs::read_to_string(cfg.cookies_path())?;
    http.cookies = CookieJar::from_json(&serde_json::from_str::<Value>(&text)?);
    println!("cookies 条数: {}", http.cookies.len());

    // 1) 视频服务登录态（gologin）
    let rac = std::fs::read_to_string(cfg.execution_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get(0).cloned())
        .and_then(|c| c.get("id").and_then(Value::as_str).map(String::from));
    match rac {
        Some(rac) => match zhidao::gologin(&mut http, &rac) {
            Ok(()) => println!("[ok] 视频服务登录态有效（gologin 通过）"),
            Err(e) => println!("[x] 视频服务登录态失效: {e}"),
        },
        None => println!("[!] execution.json 里没有课程 id，跳过视频服务检测"),
    }

    // 2) 测试服务：密钥交换 + 开卷探测
    println!("--- 测试服务 ---");
    let mut probe = exam::ExamClient::new(exam::HomeworkIdentity {
        recruit_id: "0".into(),
        stu_exam_id: "0".into(),
        exam_id: "0".into(),
        course_id: "0".into(),
        school_id: "0".into(),
        meet_course_type: "0".into(),
    });
    exam::ExamClient::prepare_headers(&mut http);
    match probe.initialize(&mut http) {
        Ok(()) => println!("[ok] RSA 密钥交换通过"),
        Err(e) => println!("[x] 密钥交换失败: {e}"),
    }
    let before = http.cookies.len();
    match exam::sso_login(&mut http) {
        Ok(()) => println!("[ok] SSO 跳转链完成，cookie {before} → {}", http.cookies.len()),
        Err(e) => println!("[x] SSO: {e}（cookie {before} → {}）", http.cookies.len()),
    }
    // 3) 用全 0 的假身份直接开卷：能拿到 JSON 业务错误说明登录态没问题；
    //    被 302 到登录页则说明测试服务确实没登录。
    println!("--- 开卷探测（假身份 0）---");
    match probe.open(&mut http) {
        Ok(v) => println!("[?] 竟然开卷成功: {}", v.to_string().chars().take(80).collect::<String>()),
        Err(exam::ExamError::LoginRequired(m)) => println!("[x] 未登录（被重定向/拒绝）: {m}"),
        Err(exam::ExamError::Failed(m)) => println!("[ok] 登录态有效，失败只是业务错误: {m}"),
    }
    Ok(())
}
