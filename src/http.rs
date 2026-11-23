//! HTTP 会话层，对应 Python 版 `Fucker._sessionReady` / `_apiQuery`。
//!
//! 关键点：
//! - Cookie 自己管（`CookieJar`），不用 reqwest 的 jar，方便和 `cookies.json` 一一对应；
//! - 5xx 与网络错误重试 5 次（对应 `urllib3.Retry(total=5, status_forcelist=[500,502,503,504])`）；
//! - `proxies` 配了就用配置，没配就跟随系统代理（reqwest 在 Windows 默认读系统代理）。

use crate::cookies::CookieJar;
use anyhow::{bail, Context, Result};
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Method;
use serde_json::Value;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/100.0.4896.127 Safari/537.36";

#[derive(Debug, Clone)]
pub enum Body {
    None,
    Form(Vec<(String, String)>),
    Json(String),
}

/// 一次 API 调用的结果包装。
#[derive(Debug, Clone)]
pub struct ApiBody {
    pub text: String,
    pub status: u16,
    pub url: String,
    /// 3xx 时的 `Location` 头（不跟随重定向的调用才有意义）
    pub location: Option<String>,
}

pub struct Http {
    client: Client,
    pub cookies: CookieJar,
    pub headers: BTreeMap<String, String>,
    /// 记录实际使用的代理，便于日志排查
    pub proxy_desc: String,
}

impl Http {
    pub fn new(proxies: &[(String, String)]) -> Result<Self> {
        let mut builder = Client::builder()
            .connect_timeout(Duration::from_secs(12))
            .timeout(Duration::from_secs(30))
            .user_agent(UA)
            // 重定向自己跟（见 follow_redirects）：自动跟随会丢中间 302 的 Set-Cookie
            .redirect(reqwest::redirect::Policy::none());

        let mut proxy_desc = String::from("系统代理");
        if !proxies.is_empty() {
            builder = builder.no_proxy();
            let mut parts = Vec::new();
            for (scheme, url) in proxies {
                let proxy = reqwest::Proxy::all(url)
                    .with_context(|| format!("代理地址无效: {scheme}={url}"))?;
                builder = builder.proxy(proxy);
                parts.push(format!("{scheme}={url}"));
            }
            proxy_desc = parts.join(", ");
        }

        let client = builder.build().context("构建 HTTP 客户端失败")?;
        Ok(Self {
            client,
            cookies: CookieJar::new(),
            headers: default_headers(),
            proxy_desc,
        })
    }

    pub fn set_header(&mut self, name: &str, value: &str) {
        self.headers.insert(name.to_string(), value.to_string());
    }

    pub fn remove_header(&mut self, name: &str) {
        self.headers.remove(name);
    }

    /// 知到/学习页相关接口需要换 Origin/Referer。
    pub fn set_study_headers(&mut self) {
        self.set_header("Origin", "https://studyh5.zhihuishu.com");
        self.set_header("Referer", "https://studyh5.zhihuishu.com/");
    }

    pub fn reset_headers(&mut self) {
        self.headers = default_headers();
    }

    fn header_map(&self) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in &self.headers {
            if let (Ok(name), Ok(val)) = (
                HeaderName::from_str(k),
                HeaderValue::from_str(v),
            ) {
                map.insert(name, val);
            }
        }
        map
    }

    fn make_request(
        &self,
        method: &Method,
        url: &str,
        params: &[(String, String)],
        body: &Body,
    ) -> RequestBuilder {
        let mut rb = self.client.request(method.clone(), url);
        rb = rb.headers(self.header_map());
        if !params.is_empty() {
            rb = rb.query(params);
        }
        if !self.cookies.is_empty() {
            rb = rb.header("Cookie", self.cookies.header());
        }
        match body {
            Body::None => rb,
            Body::Form(form) => rb.form(form),
            Body::Json(text) => rb
                .header("Content-Type", "application/json;charset=UTF-8")
                .body(text.clone()),
        }
    }

    /// 发请求并把 `Set-Cookie` 写回 jar，失败/5xx 自动重试。
    ///
    /// 重定向必须自己跟：reqwest 自动跟随时会丢掉中间 302 上的 `Set-Cookie`，
    /// 而知到登录的 `CASLOGC` 恰好是链路中间下的，丢了就会出现
    /// 「已扫码成功但 Cookies invalid」。
    fn send(
        &mut self,
        method: &Method,
        url: &str,
        params: &[(String, String)],
        body: &Body,
    ) -> Result<ApiBody> {
        let mut last_err: Option<String> = None;
        for attempt in 0..5u32 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(150 * attempt as u64));
            }
            match self.follow_redirects(method, url, params, body) {
                Ok((status, text, final_url)) => {
                    if matches!(status, 500 | 502 | 503 | 504) {
                        last_err = Some(format!("HTTP {status}"));
                        continue;
                    }
                    if status >= 400 {
                        bail!("HTTP {status} {url}\n{}", preview(&text));
                    }
                    return Ok(ApiBody {
                        text,
                        status,
                        url: final_url,
                        location: None,
                    });
                }
                Err(e) => {
                    last_err = Some(e.to_string());
                    continue;
                }
            }
        }
        bail!(
            "请求失败（已重试 5 次）: {url} -> {}",
            last_err.unwrap_or_else(|| "未知错误".into())
        )
    }

    /// 手动跟随 3xx，每一步都吸收 `Set-Cookie`；跨跳后按浏览器习惯降级成 GET。
    fn follow_redirects(
        &mut self,
        method: &Method,
        url: &str,
        params: &[(String, String)],
        body: &Body,
    ) -> Result<(u16, String, String)> {
        let mut current = url.to_string();
        let mut first = true;
        for _hop in 0..10 {
            let m = if first { method.clone() } else { Method::GET };
            let p: &[(String, String)] = if first { params } else { &[] };
            let b: &Body = if first { body } else { &Body::None };
            let req = self.make_request(&m, &current, p, b);
            let resp = req.send()?;
            let status = resp.status().as_u16();
            self.absorb_cookies(&resp);
            if (300..400).contains(&status) {
                if let Some(loc) = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
                {
                    let base = reqwest::Url::parse(&current)
                        .with_context(|| format!("URL 解析失败: {current}"))?;
                    let next = base
                        .join(&loc)
                        .with_context(|| format!("跳转地址解析失败: {loc}"))?;
                    current = next.to_string();
                    first = false;
                    continue;
                }
            }
            let final_url = resp.url().to_string();
            let text = resp.text().unwrap_or_default();
            return Ok((status, text, final_url));
        }
        bail!("重定向次数过多（>10）: {url}")
    }

    fn absorb_cookies(&mut self, resp: &Response) {
        for value in resp.headers().get_all("set-cookie").iter() {
            if let Ok(raw) = value.to_str() {
                // 少数服务端会把多条 cookie 塞进一个 header，用 ", " 分隔；
                // 这里只在逗号后面紧跟 "name=" 时才切开，避免把
                // "Expires=Wed, 21 Oct 2015 ..." 这种日期里的逗号当分隔符。
                for part in split_combined_cookie(raw) {
                    self.cookies.apply_set_cookie(&part);
                }
            }
        }
    }

    /// 单次请求：**不**跟随重定向，吸收 `Set-Cookie` 后原样返回。
    /// 给需要自己处理 3xx 的服务用（考试服务的 SSO 跳转链、401/403 判定）。
    pub fn send_once(
        &mut self,
        method: &Method,
        url: &str,
        params: &[(String, String)],
        body: &Body,
    ) -> Result<ApiBody> {
        let req = self.make_request(method, url, params, body);
        let resp = req.send()?;
        let status = resp.status().as_u16();
        self.absorb_cookies(&resp);
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let final_url = resp.url().to_string();
        let text = resp.text().unwrap_or_default();
        Ok(ApiBody {
            text,
            status,
            url: final_url,
            location,
        })
    }

    pub fn get_once(&mut self, url: &str, params: &[(String, String)]) -> Result<ApiBody> {
        self.send_once(&Method::GET, url, params, &Body::None)
    }

    pub fn post_form_once(&mut self, url: &str, form: &[(String, String)]) -> Result<ApiBody> {
        self.send_once(&Method::POST, url, &[], &Body::Form(form.to_vec()))
    }

    pub fn post_form(&mut self, url: &str, form: &[(String, String)]) -> Result<ApiBody> {
        self.send(&Method::POST, url, &[], &Body::Form(form.to_vec()))
    }

    pub fn post_json(&mut self, url: &str, json_text: &str) -> Result<ApiBody> {
        self.send(&Method::POST, url, &[], &Body::Json(json_text.to_string()))
    }

    pub fn get(&mut self, url: &str, params: &[(String, String)]) -> Result<ApiBody> {
        self.send(&Method::GET, url, params, &Body::None)
    }

    /// 解析成 JSON；非 JSON 时把响应体前 300 字带进错误里，方便定位风控页。
    pub fn as_json(body: &ApiBody) -> Result<Value> {
        serde_json::from_str::<Value>(&body.text).with_context(|| {
            format!(
                "响应不是 JSON（HTTP {} {}）：{}",
                body.status,
                body.url,
                preview(&body.text)
            )
        })
    }

    /// POST form 并直接解析 JSON。
    pub fn post_form_json(&mut self, url: &str, form: &[(String, String)]) -> Result<Value> {
        let body = self.post_form(url, form)?;
        Self::as_json(&body)
    }

    pub fn get_json(&mut self, url: &str, params: &[(String, String)]) -> Result<Value> {
        let body = self.get(url, params)?;
        Self::as_json(&body)
    }

    pub fn save_cookies(&self, path: &std::path::Path) -> Result<()> {
        let text = serde_json::to_string_pretty(&self.cookies.to_json())?;
        std::fs::write(path, text)?;
        Ok(())
    }
}

fn default_headers() -> BTreeMap<String, String> {
    let mut h = BTreeMap::new();
    h.insert("Accept".into(), "*/*".into());
    h.insert(
        "sec-ch-ua".into(),
        "\" Not A;Brand\";v=\"99\", \"Chromium\";v=\"101\", \"Google Chrome\";v=\"101\"".into(),
    );
    h.insert("sec-ch-ua-mobile".into(), "?0".into());
    h.insert("sec-ch-ua-platform".into(), "\"macOS\"".into());
    h.insert("Accept-Language".into(), "en-GB,en;q=0.9".into());
    h.insert("User-Agent".into(), UA.into());
    h
}

pub fn preview(text: &str) -> String {
    let cleaned = text.replace('\n', " ").replace('\r', " ");
    if cleaned.chars().count() > 300 {
        cleaned.chars().take(300).collect::<String>() + "…"
    } else {
        cleaned
    }
}

/// 把「一个 header 里塞了多条 cookie」的情况拆开。
///
/// 只在 `,` 后面紧跟 `name=`（token 字符 + 等号，且前面不是空白的日期）时切分，
/// 因此 `Expires=Wed, 21 Oct 2015 07:28:00 GMT` 不会被误切。
fn split_combined_cookie(raw: &str) -> Vec<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ',' {
            let rest: String = chars[i + 1..].iter().take(64).collect();
            if looks_like_cookie_start(rest.trim_start()) {
                parts.push(std::mem::take(&mut cur));
                i += 1;
                continue;
            }
        }
        cur.push(c);
        i += 1;
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

fn looks_like_cookie_start(s: &str) -> bool {
    let mut saw_eq = false;
    for ch in s.chars() {
        if ch == '=' {
            saw_eq = true;
            break;
        }
        if !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')) {
            return false;
        }
    }
    saw_eq
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_single_cookie() {
        let parts = split_combined_cookie("CASLOGC=abc; Path=/; Domain=.zhihuishu.com");
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn split_combined_cookies() {
        let parts = split_combined_cookie("A=1; Path=/, B=2; Path=/; Expires=Wed, 21 Oct 2015 07:28:00 GMT, C=3");
        assert_eq!(parts.len(), 3);
        assert!(parts[1].trim_start().starts_with("B=2"));
        assert!(parts[1].contains("21 Oct 2015"));
        assert!(parts[2].trim_start().starts_with("C=3"));
    }
}
