//! 诊断：用真实链接跑「开卷 + 未提交清单」，验证分页与登录态。
//! 用法：probe_list.exe <data_dir> <dohomework链接>
use serde_json::Value;
use zhs_brush::api::exam::{ExamClient, HomeworkIdentity};
use zhs_brush::config::Config;
use zhs_brush::cookies::CookieJar;
use zhs_brush::http::Http;
use zhs_brush::util;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let dir = a.next().map(|d| util::normalize_dir(std::path::Path::new(&d))).unwrap_or_else(util::data_dir);
    let url = a.next().expect("需要 dohomework 链接");
    let id = HomeworkIdentity::from_url(&url)?;

    let cfg = Config::load(&dir);
    let mut http = Http::new(&cfg.proxies())?;
    ExamClient::prepare_headers(&mut http);
    let preferred = dir.join("shared_test_cookies.json");
    let path = if preferred.exists() { preferred } else { cfg.cookies_path() };
    http.cookies = CookieJar::from_json(&serde_json::from_str::<Value>(&std::fs::read_to_string(&path)?)?);
    println!("凭证: {}（{} 条）", path.display(), http.cookies.len());

    let mut c = ExamClient::new(id);
    let state = c.open(&mut http)?;
    let base = state.get("examBase").cloned().unwrap_or(Value::Null);
    println!("开卷 OK: state={} 试卷={} 题数={}",
        state.get("state").map(|v| v.to_string()).unwrap_or_default(),
        base.get("name").and_then(Value::as_str).unwrap_or("?"),
        base.get("problemNum").map(|v| v.to_string()).unwrap_or_default());
    match c.list_homeworks(&mut http) {
        Ok(items) => {
            println!("未提交清单: {} 份", items.len());
            for it in items.iter().take(10) {
                println!("  - {}", it.get("examName").and_then(Value::as_str).unwrap_or("?"));
            }
        }
        Err(e) => println!("[x] 清单失败: {e}"),
    }
    Ok(())
}
