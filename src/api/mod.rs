//! 接口层：passport（登录）+ studyservice（学习）+ onlineservice（课程清单）。

pub mod passport;
pub mod ai;
pub mod exam;
pub mod zhidao;

use crate::crypto::encrypt;
use crate::http::Http;
use anyhow::anyhow;
use serde_json::{Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// 接口错误：需要单独区分「风控验证码」与普通失败。
#[derive(Debug, Error)]
pub enum ApiError {
    /// 接口 code = -12：账号/会话级风控，需要人工过验证码
    #[error("需要验证码（接口返回 code -12）")]
    Captcha,
    /// 其它业务错误码
    #[error("接口返回 code={code} msg={msg}")]
    Code { code: i64, msg: String },
    /// 网络/解析层错误
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ApiError {
    pub fn is_captcha(&self) -> bool {
        matches!(self, ApiError::Captcha)
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// `zhidaoQuery` 的选项。
pub struct QueryOpts<'a> {
    /// 是否用 `secretStr` 加密
    pub encrypt: bool,
    /// 期望的 `code`，None 表示不校验
    pub ok_code: Option<i64>,
    /// 是否附加 `dateFormate` 毫秒时间戳
    pub timestamp: bool,
    /// "POST" / "GET"
    pub method: &'a str,
    /// AES 密钥
    pub key: &'a [u8; 16],
    /// POST 时用 JSON 而不是 form
    pub json_body: bool,
}

impl<'a> QueryOpts<'a> {
    pub fn post(key: &'a [u8; 16], ok_code: Option<i64>) -> Self {
        Self {
            encrypt: true,
            ok_code,
            timestamp: true,
            method: "POST",
            key,
            json_body: false,
        }
    }

    pub fn get(key: &'a [u8; 16], ok_code: Option<i64>) -> Self {
        Self {
            encrypt: false,
            ok_code,
            timestamp: true,
            method: "GET",
            key,
            json_body: false,
        }
    }
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 对应 Python `Fucker.zhidaoQuery`：
/// 加密请求体 → 发请求 → 校验 `code` → 返回完整 JSON。
pub fn zhidao_query(
    http: &mut Http,
    url: &str,
    mut data: Map<String, Value>,
    opts: QueryOpts<'_>,
) -> ApiResult<Value> {
    let ts = if opts.timestamp {
        // 原注释：他们的时间戳总是以 000 结尾
        let t = now_millis();
        data.insert("dateFormate".into(), Value::from(t));
        Some(t)
    } else {
        None
    };

    let method = opts.method.to_uppercase();
    let result = if method == "POST" {
        let form: Vec<(String, String)> = if opts.encrypt {
            let plain = serde_json::to_string(&Value::Object(data.clone()))
                .map_err(|e| ApiError::Other(anyhow!("序列化请求体失败: {e}")))?;
            let secret = encrypt(opts.key, &plain);
            let mut pairs = vec![("secretStr".to_string(), secret)];
            if let Some(t) = ts {
                pairs.push(("dateFormate".to_string(), t.to_string()));
            }
            pairs
        } else if opts.json_body {
            vec![]
        } else {
            data.iter()
                .map(|(k, v)| {
                    let text = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), text)
                })
                .collect()
        };

        if opts.json_body && !opts.encrypt {
            let body = serde_json::to_string(&Value::Object(data.clone()))
                .map_err(|e| ApiError::Other(anyhow!("序列化请求体失败: {e}")))?;
            http.post_json(url, &body)
                .map_err(ApiError::Other)
                .and_then(|b| Http::as_json(&b).map_err(ApiError::Other))
        } else {
            http.post_form_json(url, &form)
                .map_err(ApiError::Other)
        }
    } else {
        let params: Vec<(String, String)> = data
            .iter()
            .map(|(k, v)| {
                let text = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), text)
            })
            .collect();
        http.get_json(url, &params)
            .map_err(ApiError::Other)
    }?;

    if let Some(expect) = opts.ok_code {
        let code = result
            .get("code")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        if code != expect {
            if code == -12 {
                return Err(ApiError::Captcha);
            }
            let msg = result
                .get("message")
                .or_else(|| result.get("msg"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let msg = if msg.is_empty() {
                crate::http::preview(&result.to_string())
            } else {
                msg
            };
            return Err(ApiError::Code { code, msg });
        }
    }

    Ok(result)
}

/// 从接口返回里取 `data` 字段（没有就返回 Null）。
pub fn data_of(v: &Value) -> &Value {
    v.get("data").unwrap_or(&Value::Null)
}

/// 从接口返回里取 `result` 字段。
pub fn result_of(v: &Value) -> &Value {
    v.get("result").unwrap_or(&Value::Null)
}
