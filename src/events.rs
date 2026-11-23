//! 线程间事件与「人工干预」控制位。
//!
//! GUI 的按钮不能直接操作工作线程，所以统一走：
//! - 工作线程 → 界面：`Ev`
//! - 界面 → 工作线程：`Control`（验证码等待时的「已解完 / 放弃」）

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 后台任务向界面/终端推送的事件。
#[derive(Debug, Clone)]
pub enum Ev {
    /// 普通日志
    Log(String),
    /// 状态栏文案
    Status(String),
    /// 进度：动作名 + 当前 + 总量（秒）
    Progress {
        action: String,
        cur: f64,
        end: f64,
    },
    /// 二维码 PNG 原始字节
    Qr(Vec<u8>),
    /// 需要人工过验证码
    Captcha {
        url: String,
        video: String,
        timeout_sec: u64,
    },
    /// 验证码已解除
    CaptchaOk,
    /// 验证码等待超时或被放弃
    CaptchaTimeout,
    /// 课程清单拉取结果
    Courses(Vec<crate::model::CourseRow>),
    /// 任务结束（Ok 为结论文案）
    Done(Result<String, String>),
    /// 登录态变化
    LoginState(bool),
}

pub type Sink = Arc<dyn Fn(Ev) + Send + Sync>;

/// 便于构造空接收器（CLI 场景）
pub fn noop_sink() -> Sink {
    Arc::new(|_| {})
}

/// 验证码等待期间的人工干预信号。
#[derive(Debug, Default)]
pub struct Control {
    pub resume: AtomicBool,
    pub skip: AtomicBool,
    pub stop: AtomicBool,
    /// 界面上已经点过「我已解完」的次数，用于让等待线程立刻探测一次
    pub poke: AtomicBool,
}

impl Control {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn request_resume(&self) {
        self.resume.store(true, Ordering::SeqCst);
        self.poke.store(true, Ordering::SeqCst);
    }

    pub fn request_skip(&self) {
        self.skip.store(true, Ordering::SeqCst);
        self.poke.store(true, Ordering::SeqCst);
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.poke.store(true, Ordering::SeqCst);
    }

    pub fn should_stop(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// 读取并清空 poke 标志
    pub fn take_poke(&self) -> bool {
        self.poke.swap(false, Ordering::SeqCst)
    }

    pub fn reset(&self) {
        self.resume.store(false, Ordering::SeqCst);
        self.skip.store(false, Ordering::SeqCst);
        self.stop.store(false, Ordering::SeqCst);
        self.poke.store(false, Ordering::SeqCst);
    }
}
