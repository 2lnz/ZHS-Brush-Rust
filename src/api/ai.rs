//! AI 求解器：OpenAI 兼容接口（chat/completions，非流式）。
//!
//! 复用 config.json 的 `ai.openai` 配置（api_base / api_key / model_name），
//! 与视频弹题的 AI 设置同一份。题目提示词对齐原版 `_baseTemplate`：
//! 要求 AI 在 ```answer``` 代码块里返回 `[{"id": 选项id}, ...]` 格式。

use crate::api::exam::{text_content, ExamError, ExamResult};
use crate::http::Http;
use serde_json::{json, Value};

/// 校验配置并构造求解器配置项（Python `configured_solver` 同样的门槛）。
pub fn solver_from_config(cfg: &crate::config::Config) -> ExamResult<AiSolver> {
    let ai = cfg.raw.get("ai").cloned().unwrap_or(Value::Null);
    let enabled = ai.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let use_zhidao = ai
        .get("use_zhidao_ai")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let openai = ai.get("openai").cloned().unwrap_or(Value::Null);
    let api_base = openai
        .get("api_base")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .trim_end_matches('/')
        .to_string();
    let api_key = openai
        .get("api_key")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let model = openai
        .get("model_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if !enabled || use_zhidao {
        return Err(ExamError::Failed(
            "共享课测试需要已启用的外部 AI 接口：config.json 里设 ai.enabled=true 且 ai.use_zhidao_ai=false"
                .into(),
        ));
    }
    if api_base.is_empty() || api_key.is_empty() || model.is_empty() {
        return Err(ExamError::Failed(
            "外部 AI 接口配置不完整：ai.openai 需要 api_base / api_key / model_name".into(),
        ));
    }
    Ok(AiSolver {
        api_base,
        api_key,
        model,
    })
}

pub struct AiSolver {
    api_base: String,
    api_key: String,
    model: String,
}

impl AiSolver {
    /// 对一道选择题/判断题生成答案（返回选项 id 字符串列表）。
    pub fn solve(&self, http: &mut Http, question: &Value) -> ExamResult<Vec<String>> {
        let kind = crate::api::exam::question_type(question).unwrap_or(0);
        let type_name = match kind {
            1 => "单选题",
            2 => "多选题",
            14 => "判断题",
            _ => return Err(ExamError::Failed("题型未知，无法生成提示词".into())),
        };
        let question_text = text_content(question.get("name"))?;
        let choices: Vec<Value> = question
            .get("questionOptions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|o| {
                json!({
                    "id": o.get("id").cloned().unwrap_or(Value::Null),
                    "content": text_content(o.get("content").or_else(|| o.get("name"))).unwrap_or_default(),
                })
            })
            .collect();
        let prompt = build_prompt(type_name, &question_text, &choices);
        // AI 偶发返回空响应/非 JSON：整轮重试 3 次
        let mut last = String::new();
        for attempt in 0..3u32 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_secs(2u64.pow(attempt)));
            }
            match self.chat(http, &prompt).and_then(|c| parse_answer_ids(&c)) {
                Ok(ids) => {
                    // 严格串行：每题独立请求，答完休息 3 秒再问下一题（避免限流与并发）
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    return Ok(ids);
                }
                Err(e) => last = e.to_string(),
            }
        }
        Err(ExamError::Failed(format!(
            "AI 连续 3 次未能给出有效答案（{last}），已停止本份试卷"
        )))
    }

    fn chat(&self, http: &mut Http, prompt: &str) -> ExamResult<String> {
        let url = format!("{}/chat/completions", self.api_base);
        let body = json!({
            "model": self.model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.2,
            "stream": false,
        });
        http.set_header("Authorization", &format!("Bearer {}", self.api_key));
        let resp = http
            .post_json(&url, &body.to_string())
            .map_err(|e| ExamError::Failed(format!("AI 接口请求失败: {e}")))?;
        http.remove_header("Authorization");
        if resp.status >= 400 {
            return Err(ExamError::Failed(format!(
                "AI 接口返回 HTTP {}：{}",
                resp.status,
                resp.text.chars().take(200).collect::<String>()
            )));
        }
        if resp.text.trim().is_empty() {
            return Err(ExamError::Failed(format!(
                "AI 接口返回空响应（HTTP {}）",
                resp.status
            )));
        }
        let value: Value = serde_json::from_str(&resp.text).map_err(|e| {
            ExamError::Failed(format!(
                "AI 响应不是 JSON: {e}（前 80 字: {}）",
                resp.text.chars().take(80).collect::<String>()
            ))
        })?;
        if let Some(err) = value.get("error") {
            return Err(ExamError::Failed(format!(
                "AI 接口报错：{}",
                err.get("message").and_then(Value::as_str).unwrap_or("未知")
            )));
        }
        let content = value
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if content.trim().is_empty() {
            return Err(ExamError::Failed("AI 返回空内容".into()));
        }
        Ok(content.to_string())
    }
}

/// 提示词：与原版 `_baseTemplate` 同构（去掉课程上下文，作业场景拿不到）。
fn build_prompt(type_name: &str, question: &str, choices: &[Value]) -> String {
    let choose = if type_name == "单选题" {
        "最合适的答案"
    } else {
        "所有正确的答案"
    };
    let choices_json = serde_json::to_string_pretty(choices).unwrap_or_default();
    format!(
        "假设你是一名学生，正在完成一道在线课程的选择题。需要严格按照考试要求作答。\n\
本题为{type_name}，请从选项中选择{choose}，回答放到markdown代码块中，例如：\n\n\
```answer\n\
[{{\"id\": 652308395, \"content\": \"选项内容\"}}]\n\
```\n\
答案必须为满足格式的json字符串(列表，单选也要是列表)，否则视为无效答案，不能得分。\n\
在这个markdown代码块（answer）外，你需要解释为什么你认为这个答案是正确的，\
并且标注出你选择这个答案的依据。\n\n\
现在，请听题：\n\n{question}\n\n\
选项如下：```choices\n{choices_json}\n```"
    )
}

/// 从 AI 回复里提取选项 id 列表。
fn parse_answer_ids(content: &str) -> ExamResult<Vec<String>> {
    let answer_str = extract_answer_block(content);
    let value: Value = serde_json::from_str(answer_str)
        .map_err(|e| ExamError::Failed(format!("AI 答案不是有效 JSON: {e}")))?;
    let arr = value
        .as_array()
        .ok_or_else(|| ExamError::Failed("AI 答案不是 JSON 列表".into()))?;
    let mut ids = Vec::new();
    for item in arr {
        let id = item
            .get("id")
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => v.to_string(),
            })
            .ok_or_else(|| ExamError::Failed("AI 答案条目缺少 id".into()))?;
        ids.push(id);
    }
    if ids.is_empty() {
        return Err(ExamError::Failed("AI 答案列表为空".into()));
    }
    Ok(ids)
}

/// 提取 ```answer``` 或 ```json``` 代码块；没有则整段当作裸 JSON。
fn extract_answer_block(content: &str) -> &str {
    let lower = content.to_ascii_lowercase();
    for marker in ["```answer", "```json"] {
        if let Some(start) = lower.find(marker) {
            let body_start = start + marker.len();
            if let Some(end) = lower[body_start..].find("```") {
                return content[body_start..body_start + end].trim();
            }
        }
    }
    content.trim()
}
