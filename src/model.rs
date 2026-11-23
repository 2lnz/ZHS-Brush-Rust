//! 领域模型：课程行、课程上下文、视频节点。
//!
//! 服务器返回的 JSON 形状很松（字段可能缺、类型可能变），所以接口层用
//! `serde_json::Value` 直接读，只在这里把它收敛成强类型，避免后续代码到处写
//! `.get("x").and_then(...)`。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// 课程类型：共享课（RAC_id）/ 翻转课（courseId）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CourseKind {
    /// 共享学分课，`secret` 形如 32/64 位十六进制
    Zhidao,
    /// hike 平台课程，`courseId` 是纯数字
    Hike,
}

impl CourseKind {
    pub fn label(&self) -> &'static str {
        match self {
            CourseKind::Zhidao => "共享课",
            CourseKind::Hike => "翻转课",
        }
    }
}

/// 课程清单里的一行（对应 execution.json 的 `{name,id}`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CourseRow {
    pub name: String,
    pub id: String,
    #[serde(default = "default_kind")]
    pub kind: CourseKind,
}

fn default_kind() -> CourseKind {
    CourseKind::Zhidao
}

/// 一节课里的视频（`videoSmallLessons` 中的一项）
#[derive(Debug, Clone, Default)]
pub struct VideoNode {
    /// `videoId`（URL 里不可见，但接口要用）
    pub video_id: String,
    /// `videoSmallLessons[].id`，单视频 lesson 时为 0
    pub small_id: i64,
    /// 所属 lesson 的 id
    pub lesson_id: i64,
    /// 所属 chapter 的 id
    pub chapter_id: i64,
    pub name: String,
    pub video_sec: f64,
    pub watch_state: i64,
    pub study_total_time: f64,
}

/// 一个 lesson（小节）
#[derive(Debug, Clone, Default)]
pub struct LessonNode {
    pub id: i64,
    pub name: String,
    pub videos: Vec<VideoNode>,
}

/// 一个 chapter（章）
#[derive(Debug, Clone, Default)]
pub struct ChapterNode {
    pub id: i64,
    pub name: String,
    pub lessons: Vec<LessonNode>,
}

/// 课程上下文，对应 Python 的 `ctx`（course/chapters/videos/fucked_time）。
#[derive(Debug, Clone, Default)]
pub struct CourseCtx {
    pub rac_id: String,
    pub name: String,
    pub course_id: i64,
    pub recruit_id: i64,
    pub chapters: Vec<ChapterNode>,
    /// videoId → 视频节点
    pub videos: HashMap<String, VideoNode>,
    /// 已经消耗的刷课时长（秒），用于限时
    pub fucked_time: u64,
}

impl CourseCtx {
    pub fn video_count(&self) -> usize {
        self.videos.len()
    }
}

// ---------------------------------------------------------------- Value 取值助手

pub fn as_str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

pub fn str_at(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

pub fn i64_at(v: &Value, key: &str) -> i64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

pub fn f64_at(v: &Value, key: &str) -> f64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub fn bool_at(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    }
}

/// 数组字段，缺失时返回空切片。
pub fn arr_at<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

/// 节/视频解析结果。
#[derive(Debug, Clone, Default)]
pub struct ParsedChapters {
    pub course_id: i64,
    pub chapters: Vec<ChapterNode>,
    /// videoId → 视频节点
    pub videos: HashMap<String, VideoNode>,
    /// 所有 lesson id（查询学习进度要用）
    pub lesson_ids: Vec<i64>,
}

/// 从 `videolist` 的 `data` 里解析章/节/视频。
///
/// 关键细节（原版 `getZhidaoContext` 的行为，抄错就会被服务端判 421）：
/// - lesson 自己带 `videoId` 时，这个 lesson 只有一个视频，伪造出的
///   `videoSmallLessons[0].id` 必须是 **0**，`lessonId` 才是 lesson.id；
/// - 其余情况 small_id 取 `videoSmallLessons[].id`。
pub fn parse_chapters(root: &Value) -> ParsedChapters {
    let mut out = ParsedChapters {
        course_id: i64_at(root, "courseId"),
        ..Default::default()
    };

    for ch in arr_at(root, "videoChapterDtos") {
        let chapter_id = i64_at(ch, "id");
        let mut chapter = ChapterNode {
            id: chapter_id,
            name: str_at(ch, "name"),
            lessons: Vec::new(),
        };
        for ls in arr_at(ch, "videoLessons") {
            let lesson_id = i64_at(ls, "id");
            out.lesson_ids.push(lesson_id);
            let mut lesson = LessonNode {
                id: lesson_id,
                name: str_at(ls, "name"),
                videos: Vec::new(),
            };
            let single = ls.get("videoId").is_some();
            let sources: Vec<Value> = if single {
                vec![ls.clone()]
            } else {
                arr_at(ls, "videoSmallLessons").to_vec()
            };
            for vs in sources {
                let video_id = str_at(&vs, "videoId");
                if video_id.is_empty() {
                    continue;
                }
                let node = VideoNode {
                    video_id: video_id.clone(),
                    small_id: if single { 0 } else { i64_at(&vs, "id") },
                    lesson_id,
                    chapter_id,
                    name: {
                        let a = str_at(&vs, "name");
                        if a.is_empty() {
                            lesson.name.clone()
                        } else {
                            a
                        }
                    },
                    video_sec: f64_at(&vs, "videoSec"),
                    watch_state: 0,
                    study_total_time: 0.0,
                };
                lesson.videos.push(node.clone());
                out.videos.insert(video_id, node);
            }
            chapter.lessons.push(lesson);
        }
        out.chapters.push(chapter);
    }
    out
}

/// 有的接口把 id 写成数字，有的写成字符串，统一成 i64。
pub fn parse_id(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        Value::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn single_video_lesson_uses_zero_small_id() {
        // lesson 自带 videoId → 单视频 lesson，lessonVideoId 必须是 0
        let root = json!({
            "courseId": 1000154091,
            "videoChapterDtos": [{
                "id": 1001339965,
                "name": "大学生创业",
                "videoLessons": [{
                    "id": 1002335721,
                    "name": "从生活痛点发现创业机会",
                    "videoId": 89662055,
                    "videoSec": 477
                }]
            }]
        });
        let parsed = parse_chapters(&root);
        assert_eq!(parsed.course_id, 1000154091);
        let v = parsed.videos.get("89662055").expect("视频应被收录");
        assert_eq!(v.small_id, 0, "单视频 lesson 的 lessonVideoId 必须是 0");
        assert_eq!(v.lesson_id, 1002335721);
        assert_eq!(v.chapter_id, 1001339965);
        assert_eq!(v.video_sec, 477.0);
        assert_eq!(parsed.lesson_ids, vec![1002335721]);
    }

    #[test]
    fn multi_video_lesson_keeps_small_ids() {
        let root = json!({
            "courseId": 42,
            "videoChapterDtos": [{
                "id": 7,
                "name": "章一",
                "videoLessons": [{
                    "id": 99,
                    "name": "节一",
                    "videoSmallLessons": [
                        {"id": 1001, "videoId": 555, "name": "视频A", "videoSec": 60},
                        {"id": 1002, "videoId": 556, "name": "视频B", "videoSec": 90}
                    ]
                }]
            }]
        });
        let parsed = parse_chapters(&root);
        assert_eq!(parsed.videos.len(), 2);
        assert_eq!(parsed.videos["555"].small_id, 1001);
        assert_eq!(parsed.videos["556"].small_id, 1002);
        assert_eq!(parsed.videos["555"].lesson_id, 99);
        assert_eq!(parsed.videos["556"].chapter_id, 7);
    }
}
