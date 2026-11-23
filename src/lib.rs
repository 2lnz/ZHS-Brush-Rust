//! 智慧树/知到 客户端核心库。
//!
//! 结构对应原 Python 项目：
//! - `crypto`   ← `zd_utils.py`（AES-CBC + `getEv` 异或）
//! - `config`   ← `main.py` 的 `DEFAULT_CONFIG` 合并逻辑
//! - `cookies`  ← `utils.cookie_jar_to_list` / `list_to_cookie_jar`
//! - `http`     ← `Fucker._apiQuery` / `_sessionReady` 的会话层
//! - `api`      ← `Fucker` 里 passport / studyservice 两组接口
//! - `study`    ← `Fucker.getZhidaoContext` / `fuckZhidaoCourse` / `fuckZhidaoVideo`
//! - `gui`      ← `gui.py` 的 tkinter 界面（改为 egui 立即模式）

pub mod api;
pub mod config;
pub mod cookies;
pub mod crypto;
pub mod events;
pub mod gui;
pub mod homework;
pub mod http;
pub mod model;
pub mod study;
pub mod util;

pub use config::Config;
pub use events::{Control, Ev, Sink};

/// 原项目版本号，用于日志与兼容提示。
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
