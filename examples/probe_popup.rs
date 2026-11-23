//! 诊断：验证弹题详情接口 + 答案解析。
//! 用法：probe_popup.exe <data_dir> <RAC_ID> [videoId]
//!   不传 videoId 时：枚举全部视频，找出所有带弹题的视频并逐一验证详情接口。

use zhs_brush::api::zhidao;
use zhs_brush::config::Config;
use zhs_brush::cookies::CookieJar;
use zhs_brush::http::Http;
use serde_json::json;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| ".".into());
    let rac = args.next().expect("需要 RAC_ID");
    let only_video = args.next(); // 可选

    let cfg = Config::load(std::path::Path::new(&dir));
    let mut http = Http::new(&cfg.proxies())?;
    let text = std::fs::read_to_string(cfg.cookies_path())?;
    http.cookies = CookieJar::from_json(&serde_json::from_str(&text)?);
    http.set_study_headers();
    zhidao::gologin(&mut http, &rac).map_err(|e| anyhow::anyhow!("gologin: {e}"))?;

    let course = zhidao::query_course(&mut http, &rac).map_err(|e| anyhow::anyhow!("{e}"))?;
    let recruit_id = zhs_brush::model::i64_at(&course, "recruitId");
    let chapters = zhidao::video_list(&mut http, &rac).map_err(|e| anyhow::anyhow!("{e}"))?;
    let parsed = zhs_brush::model::parse_chapters(&chapters);
    let course_id = if parsed.course_id != 0 {
        parsed.course_id
    } else {
        zhs_brush::model::i64_at(course.get("courseInfo").unwrap_or(&json!(null)), "courseId")
    };
    println!("课程 {rac} recruitId={recruit_id} courseId={course_id} 视频总数 {}", parsed.videos.len());

    let mut tested = 0;
    let mut ok = 0;
    let mut fail = 0;
    for video in parsed.videos.values() {
        if let Some(v) = &only_video {
            if &video.video_id != v {
                continue;
            }
        }
        let pts = match zhidao::load_video_pointer_info(&mut http, course_id, recruit_id, video) {
            Ok(p) => p,
            Err(e) => {
                println!("[{}] 指针信息失败: {e}", video.name);
                continue;
            }
        };
        if pts.is_empty() {
            continue;
        }
        for p in &pts {
            let shape = if p.question_ids.is_array() {
                "数组"
            } else if p.question_ids.is_number() {
                "整数"
            } else {
                "其它"
            };
            println!(
                "\n[{}] timeSec={} questionIds={}（类型 {shape}）",
                video.name, p.time_sec, p.question_ids
            );
            tested += 1;
            match zhidao::lesson_popup_exam(&mut http, video, &p.question_ids) {
                Ok(detail) => {
                    ok += 1;
                    println!(
                        "  详情 OK: {}",
                        serde_json::to_string(&detail)?.chars().take(120).collect::<String>()
                    );
                    if let Some(arr) = detail.get("lessonTestQuestionUseInterfaceDtos") {
                        let n = arr.as_array().map(|a| a.len()).unwrap_or(0);
                        println!("  题目数: {n}");
                        if let Some(q) = arr.get(0).and_then(|x| x.get("testQuestion")) {
                            println!("  题干: {}", zhs_brush::model::str_at(q, "name"));
                            let ans = answer_of(q);
                            println!("  自动答案(选项 id): {ans}");
                        }
                    }
                }
                Err(e) => {
                    fail += 1;
                    println!("  详情失败: {e}");
                }
            }
        }
    }
    println!("\n=== 汇总：验证 {tested} 个弹题点，成功 {ok}，失败 {fail} ===");
    Ok(())
}

/// 与 study::answer_text 同一逻辑
fn answer_of(q: &serde_json::Value) -> String {
    zhs_brush::model::arr_at(q, "questionOptions")
        .iter()
        .filter(|o| zhs_brush::model::str_at(o, "result") == "1")
        .map(|o| zhs_brush::model::i64_at(o, "id").to_string())
        .collect::<Vec<_>>()
        .join(",")
}
