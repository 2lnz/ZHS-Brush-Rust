//! 共享课平时测试（作业）服务协议层。
//!
//! 逐条对齐原版 `shared_tests.py` 的公开网页协议：
//! - API 入口   `https://studentexam-api.zhihuishu.com/studentExam/gateway/t/v1/`
//! - 密钥交换   RSA PKCS#1 v1.5：`{"module":3}` 加密成 uid → `app-commserv-user/c/has`
//!              → 服务器回签名信封 → 裸 `pow(e,n)` 解出 AES 密钥 cKey（16/24/32 字节）
//! - 请求体     AES-CBC（IV 固定），compact JSON → PKCS7 → base64 → `secretStr`
//! - 安全边界   未知题型/媒体题停止；保存后逐题读回核验；交卷前全量读回；
//!              交卷超时用 state 回读兜底，绝不重复提交。
//!
//! 登录态与视频服务**不通用**：cookies.json 里若没有测试服务的会话，
//! 会走 `establish_sso` 的跳转链；仍失败时按 LoginRequired 报错，
//! 提示先用 `zhs-brush login` 重新登录。

use crate::http::Http;
use anyhow::{anyhow, bail, Context, Result};
use num_bigint::BigUint;
use serde_json::{json, Map, Value};
use std::time::Duration;

/// 考试服务网关
pub const EXAM_API: &str = "https://studentexam-api.zhihuishu.com/studentExam/gateway/t/v1/";
/// AES-CBC 固定 IV（原版网页 JS 同款）
const EXAM_IV: &[u8; 16] = b"1g3qqdh4jvbskb9x";
/// 密钥交换用的服务端公钥（1024-bit，e = 65537）
const RSA_N_HEX: &str = "a07d99a92e93c4a441512817be6635895233defbbaa0ee91aa71f3778f67a027c1f69ab2b0684a991273e7a62d8ede137c1d1c9a168cc33a9f20c198987e22198d6cb426bfc53408a22988796ead06b049fe89e326472260ffe8b5aea8b386f097bc576b7468454f0e440704e9b067707b5bad765aef2095001568fd621e2a6d";
const RSA_E: u32 = 65537;
/// 密钥交换端点
const USER_HAS_URL: &str = "https://appcomm-user.zhihuishu.com/app-commserv-user/c/has";
/// SSO 起跳点
const SSO_START: &str = "https://studentexam-api.zhihuishu.com/studentExam/gateway/f/v1/gologin/login?fromurl=https%3A%2F%2Fonlineexamh5new.zhihuishu.com%2FstuExamWeb.html";

/// 一份平时测试的身份参数（来自 dohomework 页面链接，或批量清单里的一个条目）。
#[derive(Debug, Clone)]
pub struct HomeworkIdentity {
    pub recruit_id: String,
    pub stu_exam_id: String,
    pub exam_id: String,
    pub course_id: String,
    pub school_id: String,
    /// 链接第 6 段，普通共享课固定 "0"
    pub meet_course_type: String,
}

impl HomeworkIdentity {
    /// 从 `https://onlineexamh5new.zhihuishu.com/stuExamWeb.html#/webExamList/dohomework/a/b/c/d/e/0`
    /// 这样的作答页链接解析身份。
    pub fn from_url(url: &str) -> Result<Self> {
        // 粘贴时常带换行/全角空格，先全部剔除
        let cleaned: String = url.chars().filter(|c| !c.is_whitespace()).collect();
        let url = cleaned.as_str();
        let (scheme, host, frag) = split_url(url)?;
        if scheme != "https" || host != "onlineexamh5new.zhihuishu.com" {
            bail!("请输入智慧树共享课 dohomework 页面完整 HTTPS 链接");
        }
        let parts: Vec<&str> = frag.split('/').collect();
        if parts.len() != 9 || parts[0] != "" || parts[1] != "webExamList" || parts[2] != "dohomework" {
            bail!(
                "链接不是共享课平时测试 dohomework 页面（识别到 {} 段: {}）。\
                 正确格式: ...#/webExamList/dohomework/recruitId/stuExamId/examId/courseId/schoolId/0",
                parts.len(),
                parts.iter().take(4).cloned().collect::<Vec<_>>().join(" / ")
            );
        }
        let values: Vec<String> = parts[3..]
            .iter()
            .map(|v| percent_decode(v))
            .collect::<Result<_>>()?;
        for v in &values {
            if v.is_empty() || !v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '=' | '-')) {
                bail!("测试链接参数格式无效：{v}");
            }
        }
        if values[5] != "0" {
            bail!("当前仅支持普通共享课，暂不支持见面课测试");
        }
        Ok(Self {
            recruit_id: values[0].clone(),
            stu_exam_id: values[1].clone(),
            exam_id: values[2].clone(),
            course_id: values[3].clone(),
            school_id: values[4].clone(),
            meet_course_type: values[5].clone(),
        })
    }

    /// 该份测试的作答页链接（风控时用来打开浏览器）
    pub fn page_url(&self) -> String {
        format!(
            "https://onlineexamh5new.zhihuishu.com/stuExamWeb.html#/webExamList/dohomework/{}/{}/{}/{}/{}/0",
            self.recruit_id, self.stu_exam_id, self.exam_id, self.course_id, self.school_id
        )
    }

    /// 接口通用的查询参数（不含 meetCourseType）。
    pub fn params(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("recruitId".into(), json!(self.recruit_id));
        m.insert("stuExamId".into(), json!(self.stu_exam_id));
        m.insert("examId".into(), json!(self.exam_id));
        m.insert("courseId".into(), json!(self.course_id));
        m.insert("schoolId".into(), json!(self.school_id));
        m
    }

    /// 开卷参数：stuExamId 换名为 studentExamId。
    pub fn open_params(&self) -> Map<String, Value> {
        let mut m = self.params();
        let v = m.remove("stuExamId").unwrap();
        m.insert("studentExamId".into(), v);
        m
    }
}

/// 拆 https://host/path#fragment
fn split_url(url: &str) -> Result<(String, String, String)> {
    let rest = url.strip_prefix("https://").ok_or_else(|| anyhow!("只支持 HTTPS 链接"))?;
    let (host_part, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host_part.split('/').next().unwrap_or(host_part).to_string();
    let frag = if url.contains('#') {
        url.split_once('#').unwrap().1.to_string()
    } else {
        String::new()
    };
    let _ = tail;
    Ok(("https".into(), host, frag))
}

/// 最小 percent-decode（+ 空格还原）
fn percent_decode(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3])?;
                let byte = u8::from_str_radix(hex, 16).map_err(|_| anyhow!("链接百分号编码无效"))?;
                out.push(byte);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).context("链接参数不是合法 UTF-8")
}

fn hex_to_bytes(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        bail!("hex 长度无效");
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(Into::into))
        .collect()
}

/// 裸 RSA（PKCS#1 v1.5 信封手工实现，不引入 rsa crate）：
/// 服务器的密钥响应就是 `pow(sig, e, n)` 的明文包，验证方只需要公钥运算。
fn rsa_public_op(block: &[u8]) -> Result<Vec<u8>> {
    let n = BigUint::from_bytes_be(&hex_to_bytes(RSA_N_HEX)?);
    let e = BigUint::from_bytes_be(&RSA_E.to_be_bytes());
    let m = BigUint::from_bytes_be(block);
    if m >= n {
        bail!("密钥响应块超出模数");
    }
    let out = m.modpow(&e, &n);
    Ok(out.to_bytes_be())
}

/// 解出服务器签名的 AES 密钥（cKey）。
fn decode_signed_key(encrypted_b64: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    let block = base64::engine::general_purpose::STANDARD
        .decode(encrypted_b64)
        .context("密钥响应 base64 无效")?;
    if block.len() != 128 {
        bail!("共享课密钥响应长度无效");
    }
    let raw = rsa_public_op(&block)?;
    if raw.len() != 128 {
        let mut padded = vec![0u8; 128 - raw.len()];
        padded.extend_from_slice(&raw);
        return decode_envelope(&padded);
    }
    decode_envelope(&raw)
}

fn decode_envelope(raw: &[u8]) -> Result<Vec<u8>> {
    if raw.len() < 11 || raw[0] != 0x00 || raw[1] != 0x01 {
        bail!("共享课密钥响应封装无效");
    }
    let sep = raw[2..]
        .iter()
        .position(|&b| b == 0x00)
        .map(|p| p + 2)
        .ok_or_else(|| anyhow!("共享课密钥响应缺少分隔符"))?;
    if sep < 10 || raw[2..sep].iter().any(|&b| b != 0xff) {
        bail!("共享课密钥响应填充无效");
    }
    let payload: Value = serde_json::from_slice(&raw[sep + 1..]).context("密钥载荷 JSON 无效")?;
    let key = payload
        .get("cKey")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("密钥载荷缺少 cKey"))?
        .as_bytes()
        .to_vec();
    if !matches!(key.len(), 16 | 24 | 32) {
        bail!("共享课密钥长度无效: {}", key.len());
    }
    Ok(key)
}

/// PKCS#1 v1.5 加密：0x00 0x02 + ≥8 个非零随机字节 + 0x00 + 消息
fn rsa_encrypt_pkcs1(message: &[u8]) -> Result<Vec<u8>> {
    use rand::RngCore;
    if message.len() > 128 - 11 {
        bail!("RSA 待加密消息过长");
    }
    let mut rng = rand::thread_rng();
    let mut block = vec![0u8; 128];
    block[0] = 0x00;
    block[1] = 0x02;
    let pad_len = 128 - 3 - message.len();
    for b in &mut block[2..2 + pad_len] {
        loop {
            rng.fill_bytes(std::slice::from_mut(b));
            if *b != 0 {
                break;
            }
        }
    }
    block[2 + pad_len] = 0x00;
    block[3 + pad_len..].copy_from_slice(message);
    // pow 结果高位可能不足 128 字节，补齐
    let out = rsa_public_op(&block)?;
    if out.len() == 128 {
        Ok(out)
    } else {
        let mut full = vec![0u8; 128 - out.len()];
        full.extend_from_slice(&out);
        Ok(full)
    }
}

/// AES-CBC + PKCS7 + base64，密钥长度 16/24/32 通吃。
fn aes_encrypt_b64(key: &[u8], plaintext: &str) -> Result<String> {
    use base64::Engine;
    use cbc::cipher::{BlockEncryptMut, KeyIvInit};
    macro_rules! enc {
        ($t:ty) => {
            cbc::Encryptor::<$t>::new(key.into(), EXAM_IV.into())
                .encrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(plaintext.as_bytes())
        };
    }
    let ct = match key.len() {
        16 => enc!(aes::Aes128),
        24 => enc!(aes::Aes192),
        32 => enc!(aes::Aes256),
        n => bail!("AES 密钥长度无效: {n}"),
    };
    Ok(base64::engine::general_purpose::STANDARD.encode(ct))
}

/// compact JSON：无空格分隔（与 Python `json.dumps(separators=(",", ":"))` 对齐）。
fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// 考试服务错误分类。
#[derive(Debug, thiserror::Error)]
pub enum ExamError {
    /// 需要重新登录测试服务
    #[error("{0}")]
    LoginRequired(String),
    /// 平台风控：需要人工完成滑块/图形验证，附带需要打开的测试页
    #[error("{msg}")]
    NeedVerify { msg: String, url: String },
    /// 其它业务/网络错误
    #[error("{0}")]
    Failed(String),
}

pub type ExamResult<T> = std::result::Result<T, ExamError>;

fn map_err(e: anyhow::Error) -> ExamError {
    ExamError::Failed(e.to_string())
}

/// 走一遍测试服务的 SSO 跳转链，把服务票据 cookie 落进会话。
///
/// 扫码登录后主动调一次，`shared_test_cookies.json` 里就同时有视频与测试服务的凭证，
/// 不必等首次开卷时才发现未登录。
pub fn sso_login(http: &mut Http) -> ExamResult<()> {
    prepare_exam_headers(http);
    let mut url = SSO_START.to_string();
    for _ in 0..8 {
        let resp = http.get_once(&url, &[]).map_err(map_err)?;
        match resp.location {
            None => return Ok(()),
            Some(loc) => {
                let next = reqwest::Url::parse(&url)
                    .and_then(|base| base.join(&loc))
                    .map_err(|e| ExamError::Failed(format!("SSO 跳转解析失败: {e}")))?;
                let host = next.host_str().unwrap_or_default().to_string();
                if next.scheme() != "https"
                    || !(host == "zhihuishu.com" || host.ends_with(".zhihuishu.com"))
                {
                    return Err(ExamError::LoginRequired(
                        "测试登录跳转到非智慧树地址，已停止；请重新登录".into(),
                    ));
                }
                url = next.to_string();
            }
        }
    }
    Err(ExamError::LoginRequired(
        "测试服务登录跳转次数过多，请重新扫码登录".into(),
    ))
}

/// 覆盖考试服务专用的 Origin/Referer（与视频服务不同）。
pub fn prepare_exam_headers(http: &mut Http) {
    http.set_header("Origin", "https://onlineexamh5new.zhihuishu.com".into());
    http.set_header("Referer", "https://onlineexamh5new.zhihuishu.com/".into());
}

/// 考试服务客户端。**不**持有 Http——方法显式传参，
/// 这样调用方能在「读试卷」与「调 AI」之间交替使用同一个会话。
pub struct ExamClient {
    identity: HomeworkIdentity,
    key: Option<Vec<u8>>,
}

impl ExamClient {
    /// 覆盖考试服务专用的 Origin/Referer（与视频服务不同）。
    pub fn prepare_headers(http: &mut Http) {
        http.set_header("Origin", "https://onlineexamh5new.zhihuishu.com".into());
        http.set_header("Referer", "https://onlineexamh5new.zhihuishu.com/".into());
    }

    pub fn new(identity: HomeworkIdentity) -> Self {
        Self {
            identity,
            key: None,
        }
    }

    /// 批量同课程时复用已交换的 AES 密钥。
    pub fn reuse_key(&mut self, other: &ExamClient) {
        self.key = other.key.clone();
    }

    pub fn identity(&self) -> &HomeworkIdentity {
        &self.identity
    }

    /// RSA 密钥交换（懒执行，key 可跨同课程的多份测试复用）。
    pub fn initialize(&mut self, http: &mut Http) -> ExamResult<()> {
        use base64::Engine;
        let uid = base64::engine::general_purpose::STANDARD
            .encode(rsa_encrypt_pkcs1(br#"{"module":3}"#).map_err(map_err)?);
        let resp = http
            .get_once(USER_HAS_URL, &[("uid".into(), uid)])
            .map_err(map_err)?;
        if resp.status >= 300 || resp.location.is_some() {
            return Err(ExamError::LoginRequired("密钥交换被重定向，会话可能已失效".into()));
        }
        let value: Value = serde_json::from_str(&resp.text).map_err(|e| ExamError::Failed(e.to_string()))?;
        if raw_str(value.get("status")) != "200" {
            return Err(ExamError::Failed("共享课加密初始化被拒绝".into()));
        }
        let sl = value
            .get("rt")
            .and_then(|rt| rt.get("sl"))
            .and_then(Value::as_str)
            .ok_or_else(|| ExamError::Failed("密钥响应缺少 rt.sl".into()))?;
        self.key = Some(decode_signed_key(sl).map_err(map_err)?);
        Ok(())
    }

    /// 加密 POST，返回 `rt` 字段。错误分类见 [`ExamError`]。
    pub fn post(&mut self, http: &mut Http, path: &str, params: &Map<String, Value>) -> ExamResult<Value> {
        if self.key.is_none() {
            self.initialize(http)?;
        }
        let key = self.key.clone().unwrap();
        let secret = aes_encrypt_b64(&key, &compact(&Value::Object(params.clone()))).map_err(map_err)?;
        let resp = http
            .post_form_once(&format!("{EXAM_API}{path}"), &[("secretStr".into(), secret)])
            .map_err(map_err)?;

        // 未登录时网关会 302 到登录页
        if (300..400).contains(&resp.status) {
            return Err(ExamError::LoginRequired(
                "共享课测验未登录：现有凭证不能访问测试服务。请在界面左侧「1. 登录 → 扫码登录」重新扫码（命令行：zhs-brush login）".into(),
            ));
        }
        if matches!(resp.status, 401 | 403) {
            return Err(ExamError::LoginRequired(format!(
                "测试服务拒绝访问（HTTP {}），请重新登录",
                resp.status
            )));
        }
        if resp.status == 422 {
            return Err(ExamError::Failed("测试服务要求人工验证，请在测试网页完成后重试".into()));
        }
        if resp.status >= 400 {
            return Err(ExamError::Failed(format!("HTTP {} {}", resp.status, preview(&resp.text))));
        }
        let trimmed = resp.text.trim_start();
        if !trimmed.starts_with('{') {
            return Err(ExamError::LoginRequired(
                "测试服务返回非 JSON 页面，无法确认登录；停止操作".into(),
            ));
        }
        let value: Value =
            serde_json::from_str(&resp.text).map_err(|e| ExamError::Failed(e.to_string()))?;
        let code = value.get("code").and_then(Value::as_i64).unwrap_or(0);
        if code == -12 {
            return Err(ExamError::Failed("测试服务要求人工验证，停止自动操作".into()));
        }
        if matches!(code, -1 | -2 | -3) {
            return Err(ExamError::LoginRequired(
                "测试服务拒绝登录或选课权限，请重新扫码登录；不会继续交卷".into(),
            ));
        }
        if raw_str(value.get("status")) != "200" {
            let st = raw_str(value.get("status"));
            let msg = raw_str(value.get("msg").or_else(|| value.get("message")));
            // 风控拦截：需要人工滑块验证
            if msg.contains("滑块") || msg.contains("验证") || msg.contains("异常") {
                return Err(ExamError::NeedVerify {
                    msg: format!("平台要求人工验证（{msg}）：请在浏览器完成滑块后重试"),
                    url: self.identity.page_url(),
                });
            }
            return Err(ExamError::Failed(format!(
                "测试服务拒绝请求（status={st}，msg={msg}，响应={}）",
                value.to_string().chars().take(160).collect::<String>()
            )));
        }
        Ok(value.get("rt").cloned().unwrap_or(Value::Null))
    }

    /// 带重试的读操作（网络错误退避重试 2 次）。
    pub fn read(&mut self, http: &mut Http, path: &str, params: &Map<String, Value>) -> ExamResult<Value> {
        let mut last = ExamError::Failed("未发送".into());
        for attempt in 0..3u32 {
            match self.post(http, path, params) {
                Ok(v) => return Ok(v),
                // 登录/业务错误不重试
                Err(e @ ExamError::LoginRequired(_)) => return Err(e),
                Err(e) => {
                    last = e;
                    if attempt < 2 {
                        std::thread::sleep(Duration::from_secs(2u64.pow(attempt + 1)));
                    }
                }
            }
        }
        Err(ExamError::Failed(format!(
            "测试服务连续三次连接失败（{last}）。请检查网络和代理；Clash 用户请保持规则模式。草稿已保留，恢复后可重试"
        )))
    }

    /// SSO 跳转链（≤8 跳，域名白名单）。
    pub fn establish_sso(&mut self, http: &mut Http) -> ExamResult<()> {
        sso_login(http)
    }

    /// 开卷。遇到 CAS 需要服务票据时自动走一次 SSO 再试。
    pub fn open(&mut self, http: &mut Http) -> ExamResult<Value> {
        let params = self.identity.open_params();
        let result = match self.read(http, "student/doHomework", &params) {
            Ok(v) => v,
            Err(ExamError::LoginRequired(_)) => {
                self.establish_sso(http)?;
                self.read(http, "student/doHomework", &params)?
            }
            Err(e) => return Err(e),
        };
        if result.get("examBase").map(|v| !v.is_object()).unwrap_or(true) {
            return Err(ExamError::Failed("测试服务未返回试卷数据".into()));
        }
        Ok(result)
    }

    /// 本课程全部未提交的普通平时测试（见面课/互评跳过）。
    pub fn list_homeworks(&mut self, http: &mut Http) -> ExamResult<Vec<Value>> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for page in 0..100u32 {
            let mut params = Map::new();
            params.insert("recruitId".into(), json!(self.identity.recruit_id));
            params.insert("courseId".into(), json!(self.identity.course_id));
            params.insert("flag".into(), json!(1));
            params.insert("pageSize".into(), json!(100));
            params.insert("pageNum".into(), json!(page));
            let data = self.read(http, "student/getStudentHomework", &params)?;
            let Some(list) = data.get("studentHomeworkList").and_then(Value::as_array) else {
                // 网站对空结果也接受省略清单的返回
                if raw_str(data.get("next")) == "-1" {
                    return Ok(out);
                }
                return Err(ExamError::Failed("未提交测试清单数据不完整，停止批量任务".into()));
            };
            for item in list {
                let meet = raw_str(item.get("meetCourseType"));
                let review = raw_str(item.get("reviewType"));
                if meet != "0" {
                    continue;
                }
                if review == "1" {
                    continue;
                }
                let cid = raw_str(item.get("courseId"));
                if cid != self.identity.course_id {
                    return Err(ExamError::Failed("测试清单混入其他课程，停止操作".into()));
                }
                let has_id = item.get("id").map(|v| !v.is_null()).unwrap_or(false);
                let has_exam = item.get("examId").map(|v| !v.is_null()).unwrap_or(false);
                if !has_id || !has_exam {
                    return Err(ExamError::Failed("测试清单缺少试卷标识".into()));
                }
                let id = item.get("id").map(|v| v.to_string()).unwrap_or_default();
                if seen.insert(id) {
                    out.push(item.clone());
                }
            }
            if raw_str(data.get("next")) == "-1" {
                return Ok(out);
            }
        }
        Err(ExamError::Failed("测试清单超过分页上限，停止操作".into()))
    }

    /// 读回已保存的答案：`{ "<eid>": {answer: ...}, ... }`
    pub fn answers(&mut self, http: &mut Http, eids: &[String]) -> ExamResult<Map<String, Value>> {
        let mut params = self.identity.params();
        params.insert("questionIds".into(), json!(eids.join(",")));
        let result = self.read(http, "answer/getStuAnswerInfoNew", &params)?;
        result
            .as_object()
            .cloned()
            .ok_or_else(|| ExamError::Failed("已保存答案的数据格式无效".into()))
    }

    /// 读回答案是否与目标一致（多选按集合比较，其余按字符串）。
    pub fn matches(&self, record: &Value, saved: &Map<String, Value>) -> bool {
        let eid = record.get("eid").map(id_string).unwrap_or_default();
        let entry = match saved.get(&eid).and_then(Value::as_object) {
            Some(e) => e,
            None => return false,
        };
        let actual = match entry.get("answer") {
            Some(Value::Null) | None => return false,
            Some(a) => raw_str(Some(a)),
        };
        let qtype = record.get("questionType").and_then(Value::as_i64).unwrap_or(0);
        let want = record.get("answer").and_then(Value::as_str).unwrap_or_default();
        if qtype == 2 {
            let a: std::collections::HashSet<&str> = actual.split(',').collect();
            let w: std::collections::HashSet<&str> = want.split(',').collect();
            a == w
        } else {
            actual == want
        }
    }

    /// 保存一题，保存后读回核验（超时绝不当失败，重试 3 次）。
    pub fn save(&mut self, http: &mut Http, record: &Value) -> ExamResult<()> {
        let eid = record
            .get("eid")
            .and_then(Value::as_str)
            .ok_or_else(|| ExamError::Failed("记录缺少 eid".into()))?
            .to_string();
        let mut last = ExamError::Failed("未发送".into());
        for attempt in 0..3u32 {
            match self.try_save_once(http, record, &eid) {
                Ok(()) => return Ok(()),
                Err(e @ ExamError::LoginRequired(_)) => return Err(e),
                Err(e) => {
                    last = e;
                    if attempt < 2 {
                        std::thread::sleep(Duration::from_secs(2u64.pow(attempt + 1)));
                    }
                }
            }
        }
        Err(ExamError::Failed(format!("三次保存后读回仍不一致，停止交卷（{last}）")))
    }

    fn try_save_once(&mut self, http: &mut Http, record: &Value, eid: &str) -> ExamResult<()> {
        let mut params = self.identity.params();
        params.remove("courseId");
        let mut rec = record.clone();
        // 服务器要求这些字段都在记录里
        if let Some(obj) = rec.as_object_mut() {
            for (k, v) in &params {
                obj.insert(k.clone(), v.clone());
            }
            obj.entry("dataIds").or_insert(json!(""));
            obj.entry("deviceId").or_insert(json!(""));
            obj.entry("examType").or_insert(json!(""));
            obj.entry("fromType").or_insert(json!(3));
        }
        // Python 原版只发 recruitId + stuExamAnswer
        let mut save_params = Map::new();
        save_params.insert("recruitId".into(), json!(self.identity.recruit_id));
        save_params.insert("stuExamAnswer".into(), json!(vec![rec]));
        let result = self.post(http, "answer/saveStudentAnswer", &save_params)?;
        if raw_str(result.get("statu")) != "1" {
            return Err(ExamError::Failed("服务器明确拒绝保存答案，停止交卷".into()));
        }
        // 读回核验
        let saved = self.answers(http, &[eid.to_string()])?;
        if self.matches(record, &saved) {
            Ok(())
        } else {
            Err(ExamError::Failed("保存后读回不一致".into()))
        }
    }

    /// 交卷：先全量读回，确认后提交；提交超时用 state 回读兜底。
    pub fn submit(&mut self, http: &mut Http, records: &[Value], top_level_count: usize) -> ExamResult<Value> {
        let eids: Vec<String> = records
            .iter()
            .filter_map(|r| r.get("eid").and_then(Value::as_str).map(String::from))
            .collect();
        let saved = self.answers(http, &eids)?;
        if records.is_empty() || !records.iter().all(|r| self.matches(r, &saved)) {
            return Err(ExamError::Failed("交卷前答案读回不完整，停止交卷".into()));
        }
        let mut params = Map::new();
        params.insert("recruitId".into(), json!(self.identity.recruit_id));
        params.insert("examId".into(), json!(self.identity.exam_id));
        params.insert("stuExamId".into(), json!(self.identity.stu_exam_id));
        params.insert("achieveCount".into(), json!(top_level_count));

        match self.post(http, "answer/submit", &params) {
            Ok(result) => {
                let statu = raw_str(result.get("statu"));
                if !matches!(statu.as_str(), "0" | "1" | "-1") {
                    return Err(ExamError::Failed("交卷响应未确认成功，停止操作".into()));
                }
                let score = result.get("score").cloned().unwrap_or(Value::Null);
                Ok(json!({
                    "submitted": true,
                    "score": if statu == "0" { score } else { Value::Null },
                    "alreadySubmitted": statu == "-1",
                    "verifiedBy": "submit_response",
                }))
            }
            // 超时/网络类失败：读回状态兜底，绝不重复提交
            Err(e) => {
                let state = self.open(http)?;
                if is_submitted(&state) {
                    Ok(json!({
                        "submitted": true,
                        "score": Value::Null,
                        "verifiedBy": "state_readback",
                        "submit_error": e.to_string(),
                    }))
                } else {
                    Err(ExamError::Failed(format!(
                        "交卷结果未知（{e}），已停止；不会重复提交"
                    )))
                }
            }
        }
    }
}

fn preview(text: &str) -> String {
    text.chars().take(120).collect()
}

// ---------------------------------------------------------------------------
// 题目解析与校验
// ---------------------------------------------------------------------------

/// 题型 id（questionType 可能是 int 也可能是 {id}）
pub fn question_type(question: &Value) -> Option<i64> {
    let v = question.get("questionType")?;
    let v = if v.is_object() { v.get("id")? } else { v };
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) if s.parse::<i64>().is_ok() => s.parse::<i64>().ok(),
        _ => None,
    }
}

/// 题干/选项文字：剥 HTML 标签 + 实体解码；含图片/音视频题直接拒绝。
pub fn text_content(value: Option<&Value>) -> ExamResult<String> {
    let s = match value {
        Some(Value::String(s)) if !s.trim().is_empty() => s,
        _ => return Err(ExamError::Failed("题干或选项文字为空，停止答题".into())),
    };
    let lower = s.to_ascii_lowercase();
    for tag in ["<img", "<audio", "<video", "<canvas", "<svg"] {
        if lower.contains(tag) {
            return Err(ExamError::Failed(
                "题目含图片或音视频，当前文字答题接口不能可靠识别".into(),
            ));
        }
    }
    let stripped = strip_tags(s);
    Ok(unescape_html(&stripped).trim().to_string())
}

/// 每个标签替换成一个空格（对齐 Python `re.sub(r"<[^>]*>", " ", ...)`，
/// 保证 `<br/>` 分隔的单词不粘连），末尾再 trim。
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = false;
    for c in s.chars() {
        match c {
            '<' if !depth => {
                depth = true;
                out.push(' ');
            }
            '>' => depth = false,
            c if !depth => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// 常用 HTML 实体解码（题干场景够用）
fn unescape_html(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let semi = tail.find(';');
        let handled = semi.and_then(|l| {
            let entity = &tail[1..l];
            let decoded = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => {
                    if let Some(num) = entity.strip_prefix('#') {
                        if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                        } else {
                            num.parse::<u32>().ok().and_then(char::from_u32)
                        }
                    } else {
                        None
                    }
                }
            };
            decoded.map(|c| {
                out.push(c);
                l + 1
            })
        });
        rest = match handled {
            Some(consumed) => &tail[consumed..],
            None => {
                out.push('&');
                &tail[1..]
            }
        };
    }
    out.push_str(rest);
    out
}

/// 校验选项答案：id 必须存在于选项中；单选/判断只能一个。
pub fn validate_choice(question: &Value, selected: &[String]) -> ExamResult<String> {
    let kind = question_type(question).ok_or_else(|| ExamError::Failed("题型未知".into()))?;
    let options = question
        .get("questionOptions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ids: Vec<String> = options
        .iter()
        .map(|o| o.get("id").map(|v| v.to_string()).unwrap_or_default())
        .collect();
    if selected.is_empty() {
        return Err(ExamError::Failed("答案为空".into()));
    }
    if ids.iter().any(|id| id.is_empty()) || {
        let set: std::collections::HashSet<_> = ids.iter().collect();
        set.len() != ids.len()
    } {
        return Err(ExamError::Failed("选项 ID 重复或为空".into()));
    }
    let sel: std::collections::HashSet<&String> = selected.iter().collect();
    if sel.len() != selected.len() {
        return Err(ExamError::Failed("答案选项重复".into()));
    }
    if !selected.iter().all(|s| ids.contains(s)) {
        return Err(ExamError::Failed(format!(
            "AI 选项 ID 无效：{selected:?} 不在 {ids:?} 中"
        )));
    }
    if matches!(kind, 1 | 14) && selected.len() != 1 {
        return Err(ExamError::Failed("单选/判断题答案数量无效".into()));
    }
    Ok(selected.join(","))
}

/// 整卷校验：题型只认 1/2/14，拒绝组合题/附件题；返回题目数组。
pub fn prepare_questions(exam_base: &Value) -> ExamResult<Vec<Value>> {
    let mut questions = Vec::new();
    if let Some(parts) = exam_base.get("workExamParts").and_then(Value::as_array) {
        for part in parts {
            if let Some(qs) = part.get("questionDtos").and_then(Value::as_array) {
                questions.extend(qs.iter().cloned());
            }
        }
    }
    if questions.is_empty() {
        return Err(ExamError::Failed("试卷没有任何题目".into()));
    }
    let problem_num = exam_base
        .get("problemNum")
        .map(|v| v.to_string())
        .unwrap_or_default();
    if problem_num != questions.len().to_string() {
        return Err(ExamError::Failed("试卷题目数量不完整，停止答题".into()));
    }
    let mut ids = Vec::new();
    for q in &questions {
        let kind = question_type(q).unwrap_or(0);
        if !matches!(kind, 1 | 2 | 14) {
            return Err(ExamError::Failed(format!("共享课题型 {kind} 尚未验证；停止整份试卷，不提交部分答案")));
        }
        if q.get("questionChildrens").map(|v| !v.is_null()).unwrap_or(false) {
            return Err(ExamError::Failed("组合题尚未验证；停止整份试卷".into()));
        }
        if q.get("questionDatas").map(|v| !v.is_null()).unwrap_or(false) {
            return Err(ExamError::Failed("试题含附件，当前不能可靠识别，停止答题".into()));
        }
        text_content(q.get("name"))?;
        let options = q
            .get("questionOptions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if options.len() < 2 {
            return Err(ExamError::Failed("题目选项不完整".into()));
        }
        for o in &options {
            let id = o.get("id").map(|v| v.to_string()).unwrap_or_default();
            if id.is_empty() || id.contains(',') {
                return Err(ExamError::Failed("题目选项 ID 无效".into()));
            }
            text_content(o.get("content").or_else(|| o.get("name")))?;
        }
        let eid = q
            .get("eid")
            .map(|v| v.to_string())
            .filter(|s| !s.is_empty() && s != "null")
            .ok_or_else(|| ExamError::Failed("题目缺少 eid".into()))?;
        ids.push(eid);
    }
    let set: std::collections::HashSet<_> = ids.iter().collect();
    if set.len() != ids.len() {
        return Err(ExamError::Failed("试卷题目 ID 重复".into()));
    }
    Ok(questions)
}

/// 试卷指纹：题干/选项变化时不复用旧草稿。
pub fn paper_fingerprint(questions: &[Value]) -> Value {
    let mut canonical: Vec<Value> = questions
        .iter()
        .map(|q| {
            let mut choices: Vec<(String, String)> = q
                .get("questionOptions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|o| {
                    (
                        o.get("id").map(|v| v.to_string()).unwrap_or_default(),
                        o.get("content")
                            .or_else(|| o.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect();
            choices.sort();
            json!({
                "eid": q.get("eid").map(|v| v.to_string()).unwrap_or_default(),
                "type": question_type(q),
                "name": q.get("name"),
                "choices": choices,
            })
        })
        .collect();
    canonical.sort_by_key(|v| v.get("eid").map(|e| e.to_string()).unwrap_or_default());
    Value::Array(canonical)
}

/// 归一化服务器返回的字符串字段（Json 里带不带引号都接受）。
fn raw_str(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// 判断试卷是否已提交（state 3/4）。
pub fn is_submitted(state: &Value) -> bool {
    let st = raw_str(state.get("state"));
    st == "3" || st == "4"
}

/// 从开卷结果提取试卷基本信息。
pub fn paper_info(state: &Value) -> (String, Value, usize) {
    let base = state.get("examBase").cloned().unwrap_or(Value::Null);
    let name = raw_str(base.get("name"));
    let count = base
        .get("problemNum")
        .and_then(Value::as_i64)
        .unwrap_or(0) as usize;
    // 第二个返回值必须是整份 examBase（题目在 examBase.workExamParts 里）
    (name, base, count)
}

/// 归一化 id 字段为字符串（服务器可能返回 int 或 string）。
pub fn id_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_from_url() {
        let url = "https://onlineexamh5new.zhihuishu.com/stuExamWeb.html#/webExamList/dohomework/4e5a5c/109810/109811/4e5a50/4e5a5f/0";
        let id = HomeworkIdentity::from_url(url).unwrap();
        assert_eq!(id.recruit_id, "4e5a5c");
        assert_eq!(id.stu_exam_id, "109810");
        assert_eq!(id.exam_id, "109811");
        assert_eq!(id.course_id, "4e5a50");
        assert_eq!(id.school_id, "4e5a5f");
        assert_eq!(id.meet_course_type, "0");
        // 非共享课 / 见面课 / 域名错误都要拒绝
        assert!(HomeworkIdentity::from_url("http://a.com/#/webExamList/dohomework/1/2/3/4/5/0").is_err());
        assert!(HomeworkIdentity::from_url(&url.replace("/0", "/1")).is_err());
    }

    #[test]
    fn test_identity_tolerates_whitespace() {
        let messy = " https://onlineexamh5new.zhihuishu.com/stuExamWeb.html#/webExamList/\n\
                     dohomework/425787/wnQGMg3x/wKEjVXB5/1000154091/889/0\u{3000} ";
        let id = HomeworkIdentity::from_url(messy).unwrap();
        assert_eq!(id.recruit_id, "425787");
        assert_eq!(id.course_id, "1000154091");
    }

    #[test]
    fn test_rsa_envelope_roundtrip_shape() {
        // 加密块必须是 128 字节且模数运算可执行
        let block = rsa_encrypt_pkcs1(br#"{"module":3}"#).unwrap();
        assert_eq!(block.len(), 128);
        let out = rsa_public_op(&block).unwrap();
        assert_eq!(out.len(), 128);
    }

    #[test]
    fn test_validate_choice() {
        let q = serde_json::json!({
            "questionType": 1,
            "questionOptions": [
                {"id": 111, "content": "A"},
                {"id": 222, "content": "B"}
            ]
        });
        assert_eq!(validate_choice(&q, &["111".to_string()]).unwrap(), "111");
        // 单选两个答案 / 选项不存在 → 拒绝
        assert!(validate_choice(&q, &["111".into(), "222".into()]).is_err());
        assert!(validate_choice(&q, &["999".into()]).is_err());
        // 多选可以两个
        let multi = serde_json::json!({"questionType": 2, "questionOptions": q["questionOptions"]});
        assert_eq!(validate_choice(&multi, &["222".into(), "111".into()]).unwrap(), "222,111");
    }

    #[test]
    fn test_text_content() {
        assert_eq!(text_content(Some(&serde_json::json!("三基色是&lt;红&gt;绿蓝"))).unwrap(), "三基色是<红>绿蓝");
        assert_eq!(text_content(Some(&serde_json::json!("<p>hello<br/>world</p>"))).unwrap(), "hello world");
        assert!(text_content(Some(&serde_json::json!("<img src='x'>图题"))).is_err());
        assert!(text_content(None).is_err());
    }

    #[test]
    fn test_hex_and_compact() {
        assert_eq!(hex_to_bytes("00ff10").unwrap(), vec![0x00, 0xff, 0x10]);
        assert_eq!(compact(&serde_json::json!({"a":1,"b":"x"})), r#"{"a":1,"b":"x"}"#);
    }
}
