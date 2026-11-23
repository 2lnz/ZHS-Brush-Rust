//! 诊断：考试服务协议层真实验证（只读，不交卷）。
//!
//! 用法：probe_exam.exe <data_dir>
//!   1. RSA 密钥交换（initialize，不需要登录态）
//!   2. （有 cookies 时）SSO 建立与开卷读取——需要 dohomework 链接则用 homework.exe

use zhs_brush::api::exam::{ExamClient, HomeworkIdentity};
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
    ExamClient::prepare_headers(&mut http);

    // ---- 1) 密钥交换（无登录要求）----
    println!("=== 1. RSA 密钥交换 ===");
    let mut client = ExamClient::new(HomeworkIdentity {
        recruit_id: "0".into(),
        stu_exam_id: "0".into(),
        exam_id: "0".into(),
        course_id: "0".into(),
        school_id: "0".into(),
        meet_course_type: "0".into(),
    });
    client.initialize(&mut http)?;
    println!("[ok] initialize 成功（RSA PKCS#1 v1.5 + cKey 解出）");

    // ---- 2) cookies 加载 ----
    println!("=== 2. 会话 ===");
    let cookie_path = cfg.cookies_path();
    let text = std::fs::read_to_string(&cookie_path)?;
    let value: serde_json::Value = serde_json::from_str(&text)?;
    let jar = CookieJar::from_json(&value);
    println!("[i] cookies 条数: {}", jar.len());
    http.cookies = jar;

    // ---- 3) SSO 跳转链 ----
    println!("=== 3. SSO 跳转链 ===");
    match client.establish_sso(&mut http) {
        Ok(()) => println!("[ok] SSO 建立（或无需跳转）"),
        Err(e) => println!("[!] SSO: {e}（未登录测试服务属正常，跑 homework.exe 时会自动处理）"),
    }
    println!("\n=== 完成：协议层可用。实际作答请用 zhs-brush-homework.exe ===");
    Ok(())
}
