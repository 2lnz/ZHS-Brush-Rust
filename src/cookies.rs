//! Cookie 管理，格式与原项目 `cookies.json` 互转。
//!
//! Python 版用 `RequestsCookieJar` 并序列化成
//! `[{name,value,domain,path,expires,secure,...}]`；
//! 这里保留同样的字段名（多写的字段 Python 侧读得进，少写的也能 create_cookie），
//! 内部仍用简单的 name→value 映射：本项目所有请求都打在 zhihuishu.com 各子域上，
//! 不存在多站点同名 cookie 冲突。

use percent_encoding::percent_decode_str;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    map: BTreeMap<String, String>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.map.get(name).map(|s| s.as_str())
    }

    pub fn insert(&mut self, name: &str, value: &str) {
        if name.trim().is_empty() {
            return;
        }
        self.map.insert(name.trim().to_string(), value.to_string());
    }

    pub fn remove(&mut self, name: &str) {
        self.map.remove(name);
    }

    /// 拼成请求头里的 `Cookie:` 值。
    pub fn header(&self) -> String {
        self.map
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// 处理一条 `Set-Cookie`：只取 `name=value`，值为空视为删除。
    pub fn apply_set_cookie(&mut self, raw: &str) {
        let head = raw.split(';').next().unwrap_or("").trim();
        let Some((name, value)) = head.split_once('=') else {
            return;
        };
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let value = value.trim();
        if value.is_empty() || value == "\"\"" {
            self.map.remove(name);
        } else {
            self.map.insert(name.to_string(), value.to_string());
        }
    }

    /// 兼容读取：`[{name,value}, ...]` 或 `{name: value, ...}`。
    pub fn from_json(value: &Value) -> Self {
        let mut jar = CookieJar::new();
        match value {
            Value::Array(items) => {
                for item in items {
                    let (Some(name), Some(val)) = (
                        item.get("name").and_then(Value::as_str),
                        item.get("value").and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    jar.insert(name, val);
                }
            }
            Value::Object(map) => {
                for (k, v) in map {
                    if let Some(s) = v.as_str() {
                        jar.insert(k, s);
                    }
                }
            }
            _ => {}
        }
        jar
    }

    /// 序列化成 Python 版能读的数组格式。
    pub fn to_json(&self) -> Value {
        let items: Vec<Value> = self
            .map
            .iter()
            .map(|(k, v)| {
                json!({
                    "name": k,
                    "value": v,
                    "domain": ".zhihuishu.com",
                    "path": "/",
                    "secure": false,
                    "expires": null,
                    "port": null,
                    "rest": {},
                    "version": 0,
                    "comment": null,
                    "comment_url": null,
                    "rfc2109": false,
                    "discard": true
                })
            })
            .collect();
        Value::Array(items)
    }

    /// 从 `CASLOGC` 里解出 uuid（原版靠它拼 `exitRecod_{uuid}`）。
    pub fn uuid(&self) -> Option<String> {
        let cas = self.get("CASLOGC")?;
        let decoded = percent_decode_str(cas).decode_utf8().ok()?;
        let v: Value = serde_json::from_str(&decoded).ok()?;
        v.get("uuid").and_then(Value::as_str).map(|s| s.to_string())
    }

    /// 校验并抽出 uuid，同时补上 `exitRecod_{uuid}=2`（与 Python setter 行为一致）。
    pub fn normalize_for_session(&mut self) -> anyhow::Result<String> {
        let uuid = self.uuid().ok_or_else(|| {
            anyhow::anyhow!(
                "Cookies invalid: 缺少 CASLOGC 或其中没有 uuid（当前 cookie: {}）",
                if self.map.is_empty() {
                    "空".to_string()
                } else {
                    self.names().join(", ")
                }
            )
        })?;
        self.insert(&format!("exitRecod_{uuid}"), "2");
        Ok(uuid)
    }

    /// 所有 cookie 名，用于报错时给出可诊断的信息（不含值）。
    pub fn names(&self) -> Vec<String> {
        self.map.keys().cloned().collect()
    }

    /// 用于把 Map 直接塞进来（登录接口返回等场景）。
    pub fn extend_from_map(&mut self, map: &Map<String, Value>) {
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                self.insert(k, s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_cookie_parse() {
        let mut jar = CookieJar::new();
        jar.apply_set_cookie("CASTGC=TGT-123; Path=/; Domain=.zhihuishu.com; HttpOnly");
        assert_eq!(jar.get("CASTGC"), Some("TGT-123"));
        jar.apply_set_cookie("CASTGC=deleted; Max-Age=0");
        assert_eq!(jar.get("CASTGC"), Some("deleted"));
        jar.apply_set_cookie("CASTGC=; Max-Age=0");
        assert_eq!(jar.get("CASTGC"), None);
    }

    #[test]
    fn json_roundtrip() {
        let mut jar = CookieJar::new();
        jar.insert("a", "1");
        jar.insert("b", "2");
        let back = CookieJar::from_json(&jar.to_json());
        assert_eq!(back.get("a"), Some("1"));
        assert_eq!(back.len(), 2);
    }
}
