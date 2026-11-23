//! 诊断：dump 开卷返回结构（键名与关键字段），定位题目在哪里。
use serde_json::Value;
use zhs_brush::api::exam::{ExamClient, HomeworkIdentity};
use zhs_brush::config::Config;
use zhs_brush::cookies::CookieJar;
use zhs_brush::http::Http;
use zhs_brush::util;

fn keys(v: &Value) -> Vec<String> {
    v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let dir = a.next().map(|d| util::normalize_dir(std::path::Path::new(&d))).unwrap_or_else(util::data_dir);
    let url = a.next().expect("需要链接");
    let id = HomeworkIdentity::from_url(&url)?;
    let cfg = Config::load(&dir);
    let mut http = Http::new(&cfg.proxies())?;
    ExamClient::prepare_headers(&mut http);
    let p = dir.join("shared_test_cookies.json");
    let path = if p.exists() { p } else { cfg.cookies_path() };
    http.cookies = CookieJar::from_json(&serde_json::from_str::<Value>(&std::fs::read_to_string(&path)?)?);
    let mut c = ExamClient::new(id);
    let st = c.open(&mut http)?;
    println!("顶层键: {:?}", keys(&st));
    let base = st.get("examBase").cloned().unwrap_or(Value::Null);
    println!("examBase 键: {:?}", keys(&base));
    if let Some(parts) = base.get("workExamParts") {
        println!("workExamParts: {}", parts);
        if let Some(arr) = parts.as_array() {
            println!("parts 数 = {}", arr.len());
            for (i, p) in arr.iter().take(2).enumerate() {
                println!("  part[{i}] 键: {:?}", keys(p));
                if let Some(qs) = p.get("questionDtos").and_then(Value::as_array) {
                    println!("  part[{i}] 题数 = {}", qs.len());
                    if let Some(q0) = qs.first() {
                        println!("  第一题键: {:?}", keys(q0));
                        println!("  eid={:?} type={:?} name={}", q0.get("eid"), q0.get("questionType"),
                            q0.get("name").and_then(Value::as_str).unwrap_or("").chars().take(60).collect::<String>());
                    }
                }
            }
        }
    }
    Ok(())
}
