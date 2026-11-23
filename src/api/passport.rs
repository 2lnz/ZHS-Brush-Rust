//! 对应 Python `Fucker.login` / `Fucker._qrlogin`：扫码登录与账号密码登录。

use super::{ApiError, ApiResult};
use crate::http::Http;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde_json::{json, Value};

const PASSPORT: &str = "https://passport.zhihuishu.com";
const LOGIN_PAGE: &str =
    "https://passport.zhihuishu.com/login?service=https://onlineservice-api.zhihuishu.com/login/gologin";

/// 二维码轮询状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QrState {
    /// -1 未扫描
    Waiting,
    /// 0 已扫描，等手机端点确认
    Scanned(String),
    /// 1 已确认，拿到一次性密码
    Success(String),
    /// 2 二维码过期
    Expired,
    /// 3 用户取消
    Canceled,
}

/// 取二维码：返回 (qrToken, PNG 字节)。
pub fn qr_image(http: &mut Http) -> ApiResult<(String, Vec<u8>)> {
    let url = format!("{PASSPORT}/qrCodeLogin/getLoginQrImg");
    let resp = http
        .get_json(&url, &[])
        .map_err(ApiError::Other)?;
    let token = resp
        .get("qrToken")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let img_b64 = resp.get("img").and_then(Value::as_str).unwrap_or("");
    if token.is_empty() || img_b64.is_empty() {
        return Err(ApiError::Other(anyhow::anyhow!(
            "二维码接口返回异常: {}",
            crate::http::preview(&resp.to_string())
        )));
    }
    let bytes = B64
        .decode(img_b64)
        .map_err(|e| ApiError::Other(anyhow::anyhow!("二维码 base64 解码失败: {e}")))?;
    Ok((token, bytes))
}

/// 轮询扫码状态。
pub fn qr_poll(http: &mut Http, qr_token: &str) -> ApiResult<QrState> {
    let url = format!("{PASSPORT}/qrCodeLogin/getLoginQrInfo");
    let params = vec![("qrToken".to_string(), qr_token.to_string())];
    let resp = http.get_json(&url, &params).map_err(ApiError::Other)?;
    let status = resp
        .get("status")
        .and_then(Value::as_i64)
        .unwrap_or(-99);
    let msg = resp
        .get("msg")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(match status {
        -1 => QrState::Waiting,
        0 => QrState::Scanned(msg),
        1 => {
            let once = resp
                .get("oncePassword")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            QrState::Success(once)
        }
        2 => QrState::Expired,
        3 => QrState::Canceled,
        _ => {
            return Err(ApiError::Other(anyhow::anyhow!(
                "未知扫码状态 {status}: {msg}"
            )))
        }
    })
}

/// 用一次性密码换正式 cookie。
pub fn finish_qr(http: &mut Http, once_password: &str) -> ApiResult<()> {
    let params = vec![("pwd".to_string(), once_password.to_string())];
    http.get(LOGIN_PAGE, &params).map_err(ApiError::Other)?;
    if http.cookies.is_empty() {
        return Err(ApiError::Other(anyhow::anyhow!("登录后没有拿到 cookie")));
    }
    // 正常情况下 CASLOGC 已经在上面那条跳转链里种下了；万一没有，
    // 再走一次 service 指定的 gologin，让服务端补种一次。
    if http.cookies.get("CASLOGC").is_none() {
        let _ = http.get(
            "https://onlineservice-api.zhihuishu.com/login/gologin",
            &[],
        );
    }
    Ok(())
}

/// 账号密码登录（知到已基本停用，保留以备需要）。
pub fn login_by_password(http: &mut Http, username: &str, password: &str) -> ApiResult<()> {
    if username.trim().is_empty() || password.is_empty() {
        return Err(ApiError::Other(anyhow::anyhow!("用户名或密码为空")));
    }
    http.set_header("Origin", PASSPORT);
    http.set_header("Referer", LOGIN_PAGE);
    // 先访问一次登录页拿初始 cookie
    http.get(LOGIN_PAGE, &[]).map_err(ApiError::Other)?;

    let valid_url = format!("{PASSPORT}/user/validateAccountAndPassword");
    let form = vec![
        ("account".to_string(), username.to_string()),
        ("password".to_string(), password.to_string()),
    ];
    let info = http
        .post_form_json(&valid_url, &form)
        .map_err(ApiError::Other)?;
    let status = info.get("status").and_then(Value::as_i64).unwrap_or(0);
    match status {
        1 => {}
        -2 => {
            return Err(ApiError::Other(anyhow::anyhow!(
                "用户名或密码错误"
            )))
        }
        -4 => {
            return Err(ApiError::Other(anyhow::anyhow!(
                "失败次数过多，需要验证码，请改用扫码登录"
            )))
        }
        -9 => {
            return Err(ApiError::Other(anyhow::anyhow!(
                "账号需要短信验证，请改用扫码登录"
            )))
        }
        _ => {}
    }
    let uuid = info
        .get("uuid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let pwd = info
        .get("pwd")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // 需要额外认证（短信/风控）时直接报错，别静默继续
    let check_url =
        "https://appcomm-user.zhihuishu.com/app-commserv-user/userInfo/checkNeedAuth";
    let check = http
        .post_form_json(
            check_url,
            &[("uuid".to_string(), uuid.clone())],
        )
        .map_err(ApiError::Other)?;
    let need_auth = check
        .get("rt")
        .and_then(|r| r.get("needAuth"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if need_auth {
        return Err(ApiError::Other(anyhow::anyhow!(
            "账号需要额外认证（短信/风控），请用浏览器登录或改用扫码登录"
        )));
    }

    let params = vec![
        ("account".to_string(), username.to_string()),
        ("pwd".to_string(), pwd),
        ("validate".to_string(), "0".to_string()),
    ];
    http.get(LOGIN_PAGE, &params).map_err(ApiError::Other)?;
    if http.cookies.is_empty() {
        return Err(ApiError::Other(anyhow::anyhow!("登录后没有拿到 cookie")));
    }
    Ok(())
}

/// 只读探测：当前 cookie 是否还能访问课程接口（用于登录态校验）。
pub fn probe_alive(http: &mut Http, rac_id: &str) -> bool {
    match super::zhidao::query_course(http, rac_id) {
        Ok(v) => !v.is_null(),
        Err(_) => false,
    }
}

/// 便于测试：手工拼一个「会话快照」请求体（不联网）。
pub fn session_hint(uuid: &str) -> Value {
    json!({ "uuid": uuid, "exitRecod": format!("exitRecod_{uuid}") })
}
