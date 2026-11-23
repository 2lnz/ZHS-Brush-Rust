//! 诊断示例：检查登录跳转链上的 Set-Cookie 是否被完整吸收。
//!
//! 用法：`cargo run --example probe`
//! 只发匿名请求，不需要扫码、不写入任何数据。用来确认：
//! - 重定向被手动跟随（`follow_redirects`）
//! - 每一跳的 `Set-Cookie` 都进了 CookieJar
//!
//! 未登录时 `CASLOGC` 不存在是正常的，它要等真实扫码确认后由登录接口种下。

use zhs_brush::http::Http;

fn main() -> anyhow::Result<()> {
    let mut http = Http::new(&[])?;
    println!("初始 cookie: {:?}", http.cookies.names());

    // 1. 登录页（通常会 302 到 passport 的 CAS）
    let body = http.get(
        "https://passport.zhihuishu.com/login?service=https://onlineservice-api.zhihuishu.com/login/gologin",
        &[],
    )?;
    println!("登录页 -> HTTP {} {}", body.status, body.url);
    println!("跳转后 cookie: {:?}", http.cookies.names());

    // 2. studyservice 的 gologin（刷课前的跨站登录）
    let fromurl = "https://studyh5.zhihuishu.com/videoStudy.html#/studyVideo?recruitAndCourseId=0000";
    let body = http.get(
        "https://studyservice-api.zhihuishu.com/login/gologin",
        &[("fromurl".to_string(), fromurl.to_string())],
    )?;
    println!("gologin -> HTTP {} {}", body.status, body.url);
    println!("gologin 后 cookie: {:?}", http.cookies.names());
    println!("CASLOGC 存在: {}", http.cookies.get("CASLOGC").is_some());
    Ok(())
}
