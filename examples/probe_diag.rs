//! 诊断：某份试卷的状态/限时/已存答案（只读）。
//! 用法：probe_diag.exe <data_dir> <stuExamId>
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
    // 用课程级 identity + 指定 stuExamId/examId（examId 从批量清单里对应：第六章需另给）
    let exam = a.next().unwrap_or_default();
    let id = HomeworkIdentity {
        recruit_id: "425787".into(), stu_exam_id: stu, exam_id: exam,
        course_id: "1000154091".into(), school_id: "889".into(), meet_course_type: "0".into(),
    };
    let cfg = Config::load(&dir);
    let mut http = Http::new(&cfg.proxies())?;
    ExamClient::prepare_headers(&mut http);
    let p = dir.join("shared_test_cookies.json");
    let path = if p.exists() { p } else { cfg.cookies_path() };
    http.cookies = CookieJar::from_json(&serde_json::from_str::<Value>(&std::fs::read_to_string(&path)?)?);
    let mut c = ExamClient::new(id);
    let st = c.open(&mut http)?;
    println!("state={} score={} limitTime={} endTime={} startTime={}",
        st.get("state").map(|v| v.to_string()).unwrap_or_default(),
        st.get("score").map(|v| v.to_string()).unwrap_or_default(),
        st.get("examBase").and_then(|b| b.get("limitTime")).map(|v| v.to_string()).unwrap_or_default(),
        st.get("examEndTime").map(|v| v.to_string()).unwrap_or_default(),
        st.get("startTime").map(|v| v.to_string()).unwrap_or_default());
    println!("顶层键: {:?}", st.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default());
    // 读回已保存答案
    let eids = vec!["none".to_string()];
    match c.answers(&mut http, &eids) {
        Ok(m) => println!("读回接口 OK，返回 {} 条", m.len()),
        Err(e) => println!("[x] 读回失败: {e}"),
    }
    Ok(())
}
