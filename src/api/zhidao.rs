//! 对应 Python `Fucker` 里 studyservice-api / onlineservice-api 那组方法。

use super::{data_of, result_of, zhidao_query, ApiResult, QueryOpts};
use crate::crypto::{get_ev, HOME_KEY, VIDEO_KEY};
use crate::http::Http;
use crate::model::{arr_at, i64_at, str_at, CourseRow, CourseKind, VideoNode};
use crate::util::hms;
use rand::Rng;
use serde_json::{json, Map, Value};

pub const LOGIN_PAGE: &str =
    "https://passport.zhihuishu.com/login?service=https://onlineservice-api.zhihuishu.com/login/gologin";
const STUDYSERVICE: &str = "https://studyservice-api.zhihuishu.com/gateway/t/v1";

/// 播放页地址（风控验证码一般在这个页面上弹）。
pub fn study_url(rac_id: &str) -> String {
    format!(
        "https://studyh5.zhihuishu.com/videoStudy.html#/studyVideo?recruitAndCourseId={rac_id}"
    )
}

/// 跨站登录，给学习接口换一套 cookie。
pub fn gologin(http: &mut Http, rac_id: &str) -> ApiResult<()> {
    let url = "https://studyservice-api.zhihuishu.com/login/gologin";
    let fromurl = format!("https://studyh5.zhihuishu.com/videoStudy.html#/studyVideo?recruitAndCourseId={rac_id}");
    http.get(url, &[("fromurl".to_string(), fromurl)])
        .map_err(super::ApiError::Other)?;
    Ok(())
}

/// 共享课清单（分页拉完）。对应 `getZhidaoList`。
pub fn get_zhidao_list(http: &mut Http) -> ApiResult<Vec<CourseRow>> {
    let url = "https://onlineservice-api.zhihuishu.com/gateway/t/v1/student/course/share/queryShareCourseInfo";
    let mut out: Vec<CourseRow> = Vec::new();
    let page_size = 5i64;
    let mut page = 1i64;

    loop {
        let mut data = Map::new();
        data.insert("status".into(), json!(0));
        data.insert("pageNo".into(), json!(page));
        data.insert("pageSize".into(), json!(page_size));
        let resp = zhidao_query(
            http,
            url,
            data,
            QueryOpts::post(HOME_KEY, Some(200)),
        )?;
        let r = result_of(&resp);
        for item in arr_at(r, "courseOpenDtos") {
            let name = {
                let a = str_at(item, "courseName");
                if a.is_empty() {
                    str_at(item, "name")
                } else {
                    a
                }
            };
            let secret = str_at(item, "secret");
            if !secret.is_empty() {
                out.push(CourseRow {
                    name,
                    id: secret,
                    kind: CourseKind::Zhidao,
                });
            }
        }
        let total = i64_at(r, "totalCount");
        let pages = if total > 0 {
            (total + page_size - 1) / page_size
        } else {
            1
        };
        if page >= pages || arr_at(r, "courseOpenDtos").is_empty() {
            break;
        }
        page += 1;
    }
    Ok(out)
}

/// 课程基本信息（含 recruitId）。对应 `queryCourse`。
pub fn query_course(http: &mut Http, rac_id: &str) -> ApiResult<Value> {
    let url = format!("{STUDYSERVICE}/learning/queryCourse");
    let mut data = Map::new();
    data.insert("recruitAndCourseId".into(), json!(rac_id));
    let resp = zhidao_query(http, &url, data, QueryOpts::post(VIDEO_KEY, Some(0)))?;
    Ok(data_of(&resp).clone())
}

/// 章/节/视频列表。对应 `videoList`。
pub fn video_list(http: &mut Http, rac_id: &str) -> ApiResult<Value> {
    let url = format!("{STUDYSERVICE}/learning/videolist");
    let mut data = Map::new();
    data.insert("recruitAndCourseId".into(), json!(rac_id));
    let resp = zhidao_query(http, &url, data, QueryOpts::post(VIDEO_KEY, Some(0)))?;
    Ok(data_of(&resp).clone())
}

/// 学习进度（watchState / studyTotalTime）。对应 `queryStudyInfo`。
pub fn query_study_info(
    http: &mut Http,
    lesson_ids: &[i64],
    video_small_ids: &[i64],
    recruit_id: i64,
) -> ApiResult<Value> {
    // 注意接口名是 queryStuyInfo（原作者拼写错误，服务端就这样）
    let url = format!("{STUDYSERVICE}/learning/queryStuyInfo");
    let mut data = Map::new();
    data.insert("lessonIds".into(), json!(lesson_ids));
    data.insert("lessonVideoIds".into(), json!(video_small_ids));
    data.insert("recruitId".into(), json!(recruit_id));
    let resp = zhidao_query(http, &url, data, QueryOpts::post(VIDEO_KEY, Some(0)))?;
    Ok(data_of(&resp).clone())
}

/// 最近观看的视频（原版注释说不确定有没有用，但保留请求以贴近真实流量）。
pub fn query_last_video(http: &mut Http, recruit_id: i64) -> ApiResult<()> {
    let url = format!("{STUDYSERVICE}/learning/queryUserRecruitIdLastVideoId");
    let mut data = Map::new();
    data.insert("recruitId".into(), json!(recruit_id));
    zhidao_query(http, &url, data, QueryOpts::post(VIDEO_KEY, Some(0)))?;
    Ok(())
}

/// 预学习记录，返回 `studiedLessonDto.id`（后续所有上报都要带它的 base64）。
pub fn prelearning_note(
    http: &mut Http,
    course_id: i64,
    recruit_id: i64,
    video: &VideoNode,
) -> ApiResult<i64> {
    let url = format!("{STUDYSERVICE}/learning/prelearningNote");
    let mut data = Map::new();
    data.insert("ccCourseId".into(), json!(course_id));
    data.insert("chapterId".into(), json!(video.chapter_id));
    data.insert("isApply".into(), json!(1));
    data.insert("lessonId".into(), json!(video.lesson_id));
    data.insert("lessonVideoId".into(), json!(video.small_id));
    data.insert("recruitId".into(), json!(recruit_id));
    data.insert("videoId".into(), json!(video.video_id));
    let resp = zhidao_query(http, &url, data, QueryOpts::post(VIDEO_KEY, Some(0)))?;
    Ok(i64_at(
        data_of(&resp).get("studiedLessonDto").unwrap_or(&Value::Null),
        "id",
    ))
}

/// 弹题（页面上的随堂小测）触发点。
pub fn load_video_pointer_info(
    http: &mut Http,
    course_id: i64,
    recruit_id: i64,
    video: &VideoNode,
) -> ApiResult<Vec<PopupQuestion>> {
    let url = format!("{STUDYSERVICE}/popupAnswer/loadVideoPointerInfo");
    let mut data = Map::new();
    data.insert("lessonId".into(), json!(video.lesson_id));
    data.insert("lessonVideoId".into(), json!(video.small_id));
    data.insert("recruitId".into(), json!(recruit_id));
    data.insert("courseId".into(), json!(course_id));
    let resp = zhidao_query(http, &url, data, QueryOpts::post(VIDEO_KEY, Some(0)))?;
    let mut out = Vec::new();
    for item in arr_at(data_of(&resp), "questionPoint") {
        // 注意：`questionIds` 的**类型不固定**——这门课返回的是单个整数
        // （1098102151），别处可能是数组。原版 Python 是把这个值原样透传，
        // 所以这里也保留原始 JSON，取详情时原样回传，否则传 [] 会得到 code=-1。
        out.push(PopupQuestion {
            time_sec: crate::model::f64_at(item, "timeSec"),
            question_ids: item.get("questionIds").cloned().unwrap_or(Value::Null),
        });
    }
    out.sort_by(|a, b| b.time_sec.partial_cmp(&a.time_sec).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct PopupQuestion {
    pub time_sec: f64,
    /// 原样保留：可能是 int，也可能是 int 数组（见上面注释）
    pub question_ids: Value,
}

/// 取弹题详情。
pub fn lesson_popup_exam(
    http: &mut Http,
    video: &VideoNode,
    question_ids: &Value,
) -> ApiResult<Value> {
    let url = format!("{STUDYSERVICE}/popupAnswer/lessonPopupExam");
    let mut data = Map::new();
    data.insert("lessonId".into(), json!(video.lesson_id));
    data.insert("lessonVideoId".into(), json!(video.small_id));
    // 原样回传：服务端认的就是它给出来的那个形状（int 或数组）
    data.insert("questionIds".into(), question_ids.clone());
    let resp = zhidao_query(http, &url, data, QueryOpts::post(VIDEO_KEY, Some(0)))?;
    Ok(data_of(&resp).clone())
}

/// 提交弹题答案。
pub fn save_popup_answer(
    http: &mut Http,
    course_id: i64,
    recruit_id: i64,
    video: &VideoNode,
    question_id: i64,
    answer: &str,
) -> ApiResult<()> {
    let url = format!("{STUDYSERVICE}/popupAnswer/saveLessonPopupExamSaveAnswer");
    let mut data = Map::new();
    data.insert("courseId".into(), json!(course_id));
    data.insert("recruitId".into(), json!(recruit_id));
    data.insert("testQuestionId".into(), json!(question_id));
    data.insert("isCurrent".into(), json!("1"));
    data.insert("lessonId".into(), json!(video.lesson_id));
    data.insert("lessonVideoId".into(), json!(video.small_id));
    data.insert("answer".into(), json!(answer));
    data.insert("testType".into(), json!(0));
    zhidao_query(http, &url, data, QueryOpts::post(VIDEO_KEY, Some(0)))?;
    Ok(())
}

/// 上报学习进度（V2，主用）。
pub fn save_database_interval_time_v2(
    http: &mut Http,
    course_id: i64,
    recruit_id: i64,
    video: &VideoNode,
    uuid: &str,
    played_time: f64,
    last_submit: f64,
    watch_point: &str,
    token_id: &str,
) -> ApiResult<()> {
    let url = format!("{STUDYSERVICE}/learning/saveDatabaseIntervalTimeV2");
    let raw_ev: Vec<String> = vec![
        recruit_id.to_string(),
        video.lesson_id.to_string(),
        video.small_id.to_string(),
        video.video_id.to_string(),
        video.chapter_id.to_string(),
        "0".to_string(),
        ((played_time - last_submit) as i64).to_string(),
        (played_time as i64).to_string(),
        hms(video.video_sec.min(played_time as i64 as f64)),
        format!("{uuid}zhs"),
    ];
    let mut data = Map::new();
    data.insert("ewssw".into(), json!(watch_point));
    data.insert("sdsew".into(), json!(get_ev(&raw_ev, "zzpttjd")));
    data.insert("zwsds".into(), json!(token_id));
    data.insert("courseId".into(), json!(course_id));
    zhidao_query(http, &url, data, QueryOpts::post(VIDEO_KEY, Some(0)))?;
    Ok(())
}

/// 上报缓存进度（原版默认关掉，这里保留接口以便开关）。
pub fn save_cache_interval_time(
    http: &mut Http,
    course_id: i64,
    recruit_id: i64,
    video: &VideoNode,
    played_time: f64,
    last_submit: f64,
    watch_point: &str,
    token_id: &str,
) -> ApiResult<()> {
    let url = format!("{STUDYSERVICE}/learning/saveCacheIntervalTime");
    let jitter: f64 = rand::thread_rng().gen_range(10..=20) as f64;
    let raw_ev: Vec<String> = vec![
        recruit_id.to_string(),
        video.chapter_id.to_string(),
        course_id.to_string(),
        video.lesson_id.to_string(),
        hms(video.video_sec.min(played_time + jitter)),
        (played_time as i64).to_string(),
        video.video_id.to_string(),
        video.small_id.to_string(),
        ((played_time - last_submit) as i64).to_string(),
    ];
    let mut data = Map::new();
    data.insert("watchPoint".into(), json!(watch_point));
    data.insert("ev".into(), json!(get_ev(&raw_ev, "zzpttjd")));
    data.insert("learningTokenId".into(), json!(token_id));
    zhidao_query(http, &url, data, QueryOpts::post(VIDEO_KEY, Some(0)))?;
    Ok(())
}

/// 三维课件接口（原版注释：不确定作用，保留以对齐真实流量）。
pub fn three_dimensional_course_ware(http: &mut Http, video_id: &str) -> ApiResult<()> {
    let url = format!("{STUDYSERVICE}/course/threeDimensionalCourseWare");
    let params = vec![("videoId".to_string(), video_id.to_string())];
    http.get(&url, &params).map_err(super::ApiError::Other)?;
    Ok(())
}

/// 播放器初始化：拿到真实视频地址并访问一次（原版 `watchVideo`）。
pub fn watch_video(http: &mut Http, video_id: &str) -> ApiResult<()> {
    let parse_url = "https://newbase.zhihuishu.com/video/initVideo";
    let ts = super::now_millis().to_string();
    let params = vec![
        ("jsonpCallBack".to_string(), "result".to_string()),
        ("videoID".to_string(), video_id.to_string()),
        ("_".to_string(), ts),
    ];
    let body = http.get(parse_url, &params).map_err(super::ApiError::Other)?;
    // 返回是 `result({...})` 的 jsonp
    let text = body.text.trim();
    let inner = text
        .strip_prefix("result(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(text);
    let json: Value = serde_json::from_str(inner)
        .map_err(|e| super::ApiError::Other(anyhow::anyhow!("initVideo 返回无法解析: {e}")))?;
    let url = json
        .get("result")
        .and_then(|r| r.get("lines"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|l| l.get("lineUrl"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if url.is_empty() {
        return Ok(());
    }
    // 访问一次真实视频地址；失败不影响刷课
    let _ = http.get(url, &[]);
    Ok(())
}
