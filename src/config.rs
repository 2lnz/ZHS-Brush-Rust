//! 配置：结构对齐原项目 `main.py::DEFAULT_CONFIG`，读写仍是 `config.json`。
//!
//! 采用「默认值 + 文件覆盖」的方式合并，未识别的键原样保留，
//! 这样 Rust 版和 Python 版可以共用同一个 config.json 而不丢字段。

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub const CONFIG_VERSION: &str = "1.5.0";

/// 验证码风控策略（原 `captcha` 段）。
#[derive(Debug, Clone, Copy)]
pub struct CaptchaCfg {
    pub wait: bool,
    pub timeout: u64,
    pub interval: u64,
    pub max_retry: u32,
}

impl Default for CaptchaCfg {
    fn default() -> Self {
        Self {
            wait: true,
            timeout: 900,
            interval: 20,
            max_retry: 3,
        }
    }
}

/// 单次运行参数（CLI 选项与 GUI 控件都落到这里）。
#[derive(Debug, Clone)]
pub struct RunOpts {
    /// 播放倍速，None 表示用接口默认（知到 1.5）
    pub speed: Option<f64>,
    /// 每门课限时（分钟），0 表示不限
    pub limit_min: u64,
    /// 视频看完判定阈值，默认 0.91
    pub end_thre: f64,
}

impl Default for RunOpts {
    fn default() -> Self {
        Self {
            speed: None,
            limit_min: 0,
            end_thre: 0.91,
        }
    }
}

pub struct Config {
    pub dir: PathBuf,
    pub path: PathBuf,
    /// 合并后的完整配置（默认值 + config.json 覆盖）
    pub raw: Value,
}

impl Config {
    /// 与 Python 版一致的默认配置。
    pub fn defaults() -> Value {
        json!({
            "username": "",
            "password": "",
            "qrlogin": true,
            "save_cookies": true,
            "proxies": {},
            "logLevel": "INFO",
            "tree_view": true,
            "progressbar_view": true,
            "check_update": false,
            "qr_extra": {
                "show_in_terminal": null,
                "ensure_unicode": false
            },
            "image_path": "",
            "window": {
                "width": 900,
                "height": 700
            },
            "ui": {
                "theme": "dark",
                "borderless": true,
                "corner_radius": 0.0,
                "transparent": false
            },
            "captcha": {
                "wait": true,
                "timeout": 900,
                "interval": 20,
                "max_retry": 3
            },
            "pushplus": { "enable": false, "token": "" },
            "bark": { "enable": false, "token": "" },
            "config_version": CONFIG_VERSION,
            "ai": {
                "enabled": false,
                "use_zhidao_ai": true,
                "openai": {
                    "api_base": "https://api.deepseek.com",
                    "api_key": "",
                    "model_name": "deepseek-chat"
                },
                "ppt_processing": {
                    "provide_to_ai": false,
                    "moonShot": {
                        "base_url": "https://api.moonshot.cn/v1",
                        "api_key": "",
                        "delete_after_convert": true
                    }
                },
                "use_stream": true
            }
        })
    }

    pub fn load(dir: &Path) -> Self {
        let path = dir.join("config.json");
        let mut merged = Self::defaults();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(Value::Object(file)) = serde_json::from_str::<Value>(&text) {
                merge(&mut merged, &file);
            }
        }
        let cfg = Self {
            dir: dir.to_path_buf(),
            path,
            raw: merged,
        };
        // 首次运行落盘，便于用户手改
        if !cfg.path.exists() {
            cfg.save();
        }
        cfg
    }

    pub fn save(&self) {
        if let Ok(text) = serde_json::to_string_pretty(&self.raw) {
            let _ = std::fs::write(&self.path, text);
        }
    }

    pub fn raw(&self) -> &Value {
        &self.raw
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.raw.get(key)
    }

    fn bool_of(&self, key: &str, default: bool) -> bool {
        self.raw
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(default)
    }

    fn u64_of(&self, key: &str, default: u64) -> u64 {
        self.raw.get(key).and_then(Value::as_u64).unwrap_or(default)
    }

    pub fn qrlogin(&self) -> bool {
        self.bool_of("qrlogin", true)
    }

    pub fn save_cookies(&self) -> bool {
        self.bool_of("save_cookies", true)
    }

    pub fn tree_view(&self) -> bool {
        self.bool_of("tree_view", true)
    }

    pub fn progressbar_view(&self) -> bool {
        self.bool_of("progressbar_view", true)
    }

    pub fn log_level(&self) -> String {
        self.raw
            .get("logLevel")
            .and_then(Value::as_str)
            .unwrap_or("INFO")
            .to_string()
    }

    pub fn image_path(&self) -> String {
        self.raw
            .get("image_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    /// `proxies` 段：`{"http": "...", "https": "..."}`，空表示跟随系统代理。
    pub fn proxies(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(Value::Object(map)) = self.raw.get("proxies") {
            for (k, v) in map {
                if let Some(url) = v.as_str() {
                    if !url.trim().is_empty() {
                        out.push((k.clone(), url.to_string()));
                    }
                }
            }
        }
        out
    }

    pub fn captcha(&self) -> CaptchaCfg {
        let seg = self.raw.get("captcha");
        let d = CaptchaCfg::default();
        let pick_u64 = |key: &str, dv: u64| -> u64 {
            seg.and_then(|s| s.get(key))
                .and_then(Value::as_u64)
                .unwrap_or(dv)
        };
        CaptchaCfg {
            wait: seg
                .and_then(|s| s.get("wait"))
                .and_then(Value::as_bool)
                .unwrap_or(d.wait),
            timeout: pick_u64("timeout", d.timeout).max(30),
            interval: pick_u64("interval", d.interval).max(5),
            max_retry: pick_u64("max_retry", d.max_retry as u64).max(1) as u32,
        }
    }

    /// 限时（分钟）来自配置，CLI/GUI 可覆盖。
    pub fn limit_min(&self) -> u64 {
        self.u64_of("limit", 0)
    }

    /// 窗口初始尺寸，可在 config.json 的 `window` 段调整（默认 900×600）。
    /// 屏幕装不下时由系统自动收窄，不会超出屏幕。
    pub fn window_size(&self) -> (f32, f32) {
        let seg = self.raw.get("window");
        let pick = |key: &str, dv: f64| -> f32 {
            seg.and_then(|s| s.get(key))
                .and_then(Value::as_f64)
                .unwrap_or(dv)
                .clamp(700.0, 4000.0) as f32
        };
        (pick("width", 900.0), pick("height", 700.0))
    }

    /// 界面主题：`"dark"`（默认）或 `"light"`（白色背景）。
    pub fn theme(&self) -> String {
        self.raw
            .get("ui")
            .and_then(|u| u.get("theme"))
            .and_then(Value::as_str)
            .unwrap_or("dark")
            .to_string()
    }

    /// 写回主题并立刻落盘（界面切换按钮用）。
    /// 写回 AI 配置（OpenAI 兼容接口），并强制启用「外部 AI」模式。
    pub fn set_ai(&mut self, api_base: &str, api_key: &str, model: &str) {
        let ai = serde_json::json!({
            "enabled": true,
            "use_zhidao_ai": false,
            "openai": {
                "api_base": api_base,
                "api_key": api_key,
                "model_name": model,
            }
        });
        // 保留原 ai 段里其它键（ppt_processing / use_stream 等）
        if let Some(Value::Object(prev)) = self.raw.get("ai").cloned() {
            let mut merged = ai.as_object().cloned().unwrap_or_default();
            for (k, v) in prev {
                merged.entry(k).or_insert(v);
            }
            // openai 段整体替换（避免出现半新半旧）
            merged.insert(
                "openai".into(),
                serde_json::json!({
                    "api_base": api_base,
                    "api_key": api_key,
                    "model_name": model,
                }),
            );
            if let Some(obj) = self.raw.as_object_mut() {
                obj.insert("ai".into(), Value::Object(merged));
            }
        } else if let Some(obj) = self.raw.as_object_mut() {
            obj.insert("ai".into(), ai);
        }
        self.save();
    }

    /// 当前 AI 配置（api_base, api_key, model）
    pub fn ai_settings(&self) -> (String, String, String) {
        let o = self.raw.get("ai").and_then(|a| a.get("openai")).cloned().unwrap_or(Value::Null);
        let g = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        (g("api_base"), g("api_key"), g("model_name"))
    }

    pub fn set_theme(&mut self, theme: &str) {
        match self.raw.get_mut("ui") {
            Some(Value::Object(ui)) => {
                ui.insert("theme".to_string(), Value::String(theme.to_string()));
            }
            _ => {
                self.raw.as_object_mut().map(|m| {
                    m.insert(
                        "ui".to_string(),
                        serde_json::json!({ "theme": theme }),
                    )
                });
            }
        }
        self.save();
    }

    /// 是否使用无边框窗口（自绘标题栏）。默认 false（先用系统边框，界面里可一键切换）。
    pub fn borderless(&self) -> bool {
        self.raw
            .get("ui")
            .and_then(|u| u.get("borderless"))
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    /// 切换无边框并立刻落盘。
    pub fn set_borderless(&mut self, on: bool) {
        match self.raw.get_mut("ui") {
            Some(Value::Object(ui)) => {
                ui.insert("borderless".to_string(), Value::Bool(on));
            }
            _ => {
                if let Some(m) = self.raw.as_object_mut() {
                    m.insert("ui".to_string(), serde_json::json!({ "borderless": on }));
                }
            }
        }
        self.save();
    }

    /// 是否用「透明窗口 + 自绘圆角」的方式做圆角（默认 false，即不透明直角）。
    /// 极少数显卡/驱动下透明窗口会渲染成黑底，这时改成 false：改为 SetWindowRgn 硬裁剪。
    pub fn transparent(&self) -> bool {
        self.raw
            .get("ui")
            .and_then(|u| u.get("transparent"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// 窗口圆角半径；0 表示直角（默认）。
    pub fn corner_radius(&self) -> f32 {
        self.raw
            .get("ui")
            .and_then(|u| u.get("corner_radius"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .clamp(0.0, 40.0) as f32
    }

    /// 数据目录下的固定文件路径
    pub fn cookies_path(&self) -> PathBuf {
        self.dir.join("cookies.json")
    }

    pub fn qr_dir(&self) -> PathBuf {
        self.dir.join("qr")
    }

    pub fn execution_path(&self) -> PathBuf {
        self.dir.join("execution.json")
    }

    pub fn ai_courses_path(&self) -> PathBuf {
        self.dir.join("ai_courses.json")
    }
}

/// 递归合并：文件里的键覆盖默认值，默认值里没有的键也保留。
fn merge(base: &mut Value, over: &Map<String, Value>) {
    if let Value::Object(base_map) = base {
        merge_map(base_map, over);
    }
}

fn merge_map(base: &mut Map<String, Value>, over: &Map<String, Value>) {
    for (k, incoming) in over {
        match base.get_mut(k) {
            Some(slot) => {
                if let (Value::Object(bm), Value::Object(om)) = (&mut *slot, incoming) {
                    merge_map(bm, om);
                } else {
                    *slot = incoming.clone();
                }
            }
            None => {
                base.insert(k.clone(), incoming.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_unknown_and_defaults() {
        let mut d = Config::defaults();
        let mut file = Map::new();
        file.insert("logLevel".into(), json!("DEBUG"));
        file.insert("custom".into(), json!(42));
        merge(&mut d, &file);
        assert_eq!(d["logLevel"], "DEBUG");
        assert_eq!(d["custom"], 42);
        assert_eq!(d["qrlogin"], true);
    }

    #[test]
    fn test_set_ai_roundtrip() {
        let dir = std::env::temp_dir().join("zhs_ai_test");
        let _ = std::fs::create_dir_all(&dir);
        let mut cfg = Config::load(&dir);
        cfg.set_ai("https://api.deepseek.com", "sk-test", "deepseek-chat");
        let again = Config::load(&dir);
        let (b, k, m) = again.ai_settings();
        assert_eq!(b, "https://api.deepseek.com");
        assert_eq!(k, "sk-test");
        assert_eq!(m, "deepseek-chat");
        let ai = again.raw.get("ai").unwrap();
        assert_eq!(ai.get("enabled").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(ai.get("use_zhidao_ai").and_then(|v| v.as_bool()), Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
