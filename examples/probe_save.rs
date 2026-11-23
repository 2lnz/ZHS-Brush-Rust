//! 诊断：对某份试卷的草稿做一次保存探测（读回核验，不交卷）。
//! 用法：probe_save.exe <data_dir> <stuExamId>
use serde_json::Value;
use zhs_brush::api::exam::{ExamClient, HomeworkIdentity};
use zhs_brush::config::Config;
use zhs_brush::cookies::CookieJar;
use zhs_brush::http::Http;
use zhs_brush::util;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let dir = a.next().map(|d| util::normalize_dir(std::path::Path::new(&d))).unwrap_or_else(util::data_dir);
    let stu = a.next().expect("需要 stuExamId");
    let path_ck = dir.join("homework").join(format!("{stu}.json"));
    let ck: Value = serde_json::from_str(&std::fs::read_to_string(&path_ck)?)?;
    let draft = ck.get("drafts").and_then(|d| d.as_array()).and_then(|a| a.first()).cloned()
        .ok_or_else(|| anyhow::anyhow!("草稿为空"))?;
    let id = HomeworkIdentity {
        recruit_id: "425787".into(), stu_exam_id: stu.clone(),
        exam_id: draft.get("examId").and_then(Value::as_str).unwrap_or("").to_string(),
        course_id: "1000154091".into(), school_id: "889".into(), meet_course_type: "0".into(),
    };
    println!("identity: examId={}", id.exam_id);
    let cfg = Config::load(&dir);
    let mut http = Http::new(&cfg.proxies())?;
    ExamClient::prepare_headers(&mut http);
    let p = dir.join("shared_test_cookies.json");
    let cp = if p.exists() { p } else { cfg.cookies_path() };
    http.cookies = CookieJar::from_json(&serde_json::from_str::<Value>(&std::fs::read_to_string(&cp)?)?);
    let mut c = ExamClient::new(id);
    let st = c.open(&mut http)?;
    println!("open OK: state={} 题数={}", st.get("state").map(|v| v.to_string()).unwrap_or_default(),
        st.get("examBase").and_then(|b| b.get("problemNum")).map(|v| v.to_string()).unwrap_or_default());
    match c.save(&mut http, &draft) {
        Ok(()) => println!("[ok] 保存并读回核验通过（未交卷）"),
        Err(e) => println!("[x] 保存失败: {e}"),
    }
    Ok(())
}
