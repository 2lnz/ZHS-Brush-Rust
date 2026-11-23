# ZHS-Brush-RUST

智慧树 / 知到 课程学习工具，Rust 重写。协议、加密参数与刷课节奏对齐原 Python 项目
`ZHS-Fucker-Pro`，界面用 egui 实现，编译产物是单文件 exe，不依赖 Python 环境。

<p align="left">
  <img src="assets/icon.png" width="96" alt="ZHS-Brush-RUST">
</p>

| | |
|---|---|
| 语言 / 界面 | Rust 2021 · egui 0.29 |
| 形态 | `zhs-brush-gui.exe`（图形界面，双击无黑框）· `zhs-brush.exe`（命令行） |
| 平台 | Windows x64（`x86_64-pc-windows-gnu` 目标，rust-lld 链接，无需 MSVC） |
| 许可 | Apache-2.0 |

---

## 功能

### 刷课

| 能力 | 说明 |
|---|---|
| 扫码登录 | 知到 App 扫码，二维码直接渲染在窗口内，同时落盘 `qr\`，过期自动刷新 |
| 课程清单 | 拉取全部共享课写入 `execution.json`，支持手改后按清单执行 |
| 刷课主循环 | 秒级推进 + 倍速控制，30 秒上报一次进度，可设限时与结束阈值 |
| 弹题 | 到点自动取题并作答；接口异常自动重试 3 次 |
| 风控处理 | 识别到验证码暂停，自动拉起浏览器课程页，后台探测解除后自动续刷 |
| 断点续刷 | 已看完的视频直接跳过，按学习记录继续 |

### 平时测试（作业）

| 能力 | 说明 |
|---|---|
| 读取检查 | 只读模式：开卷 + 校验题型 + 落检查报告，不写服务器任何数据 |
| AI 作答 | AI 生成答案 → 逐题保存 → 读回核验 → 交卷，全程串行（每题间隔 3 秒） |
| 批量处理 | 一次处理同一课程全部未提交的普通平时测试，已提交自动跳过 |
| 断点恢复 | 草稿落盘 `homework\<stuExamId>.json`，中断后从断点继续，不重复提交 |
| 风控恢复 | 遇到滑块拦截自动打开对应试卷页，暂停 60 秒后重试（最多 3 次） |

### 界面

- 无边框窗口 + 自绘标题栏（拖动 / 最小化 / 最大化 / 关闭），可选圆角
- 深色 / 浅色主题，切换即写回 `config.json`
- 左侧四区块：登录 → 课程清单 → 刷课参数 → 平时测试；内容超长时可滚动，刷课按钮固定可见

---

## 快速开始

### 1. 构建

前置条件：Rust 工具链（<https://rustup.rs>，国内可用 <https://rsproxy.cn>）。

```bat
:: 双击 构建.bat，或手动执行
cargo build --release
```

产物：

```
target\release\zhs-brush-gui.exe    图形界面（用这个）
target\release\zhs-brush.exe        命令行
```

### 2. 启动与登录

双击 `启动.vbs`（无控制台闪窗）或 `启动.bat`。首次运行会在 exe 同目录生成
`config.json`，然后：

```
① 左侧「1. 登录」→ 扫码登录 → 知到 App → 我的 → 扫一扫
② 左侧「2. 课程清单」→ 拉取智慧树清单（写入 execution.json）
③ 左侧「3. 刷课参数」→ 设倍速 → 开始刷课
```

### 3. 平时测试

```
① 浏览器打开平时测试作答页，复制地址栏整段链接
   https://onlineexamh5new.zhihuishu.com/stuExamWeb.html#/webExamList/dohomework/...
② 左侧「4. 平时测试」→ 粘贴链接 → 「读取检查」
③ 确认题目都能识别后 →「AI 答题交卷」→ 再点一次确认
```

---

## 平时测试（作业）

### 支持范围

- ✅ 单选、多选、判断题
- ❌ 填空、组合题、图片/音视频题、见面课、互评作业 —— 遇到即**整卷停止**，绝不提交部分答案

### AI 接口

点左侧「4. 平时测试 → DS API」按钮，弹窗填写：

| 项 | 示例 |
|---|---|
| API 地址 | `https://api.deepseek.com` |
| API Key | `sk-...` |
| 模型 | `deepseek-chat` |

保存时自动写入 `config.json` 的 `ai` 段，并把 `enabled` 置 `true`、`use_zhidao_ai` 置 `false`
（必须走外部 OpenAI 兼容接口）。也可以直接手改配置。

### 安全设计

- 保存每道题后**读回核验**，不一致重试 3 次才继续
- 交卷前**全量读回**确认；交卷超时用状态回读兜底，绝不重复提交
- 试卷指纹：题干或选项变化时，不套用旧草稿
- 单实例锁：防止多个进程同时交卷
- 生成全卷答案后才开始写服务器，中途失败不留下半份答案

### 登录态

视频服务与测试服务的会话**不通用**。首次使用或提示「共享课测验未登录」时，点「1. 登录 → 扫码登录」
重扫一次：登录成功后会自动走一遍测试服务 SSO，并把含 CAS 票据的凭证写入
`shared_test_cookies.json`。之后测试服务直接复用该凭证。

### 风控

平台会对高频作答返回 `滑块验证未通过，不能答题`。此时程序会自动打开该试卷页面并暂停 60 秒重试。
如果持续被拦：

- 等待 10~15 分钟冷却
- 重新扫码登录换一个新会话
- 降低频率：单份跑、每份之间间隔几分钟

---

## 配置

`config.json` 采用「默认值 + 文件覆盖」合并，未识别的字段会保留。常用项：

```json
{
  "window": { "width": 900, "height": 700 },
  "ui": {
    "theme": "dark",
    "borderless": true,
    "corner_radius": 0.0,
    "transparent": false
  },
  "captcha": { "wait": true, "timeout": 900, "interval": 20, "max_retry": 3 },
  "proxies": {}
}
```

| 键 | 说明 |
|---|---|
| `window.width/height` | 窗口初始尺寸，屏幕放不下时由系统压进工作区 |
| `ui.theme` | `dark` / `light`，界面切换会写回 |
| `ui.borderless` | 默认 `true`（无边框 + 自绘标题栏）；`false` 回到系统标题栏 |
| `ui.corner_radius` | 圆角半径，默认 `0`（直角） |
| `ui.transparent` | 默认 `false`。想用抗锯齿圆角就同时设 `corner_radius: 14` + `transparent: true`；若四角发黑改回 `false`，退化为区域硬裁剪 |
| `captcha` | 风控暂停的等待时长、探测间隔、单视频重试次数 |
| `proxies` | 留空跟随系统代理，例：`{"https": "http://127.0.0.1:7890"}` |

无边框模式不能拖边缩放，用标题栏的「□」最大化 / 还原。

### 运行数据

均位于数据目录（默认 exe 所在目录，可用 `--data-dir` 或环境变量 `ZHS_HOME` 指定）：

| 文件 | 内容 |
|---|---|
| `config.json` | 配置 |
| `cookies.json` | 视频服务登录凭证 |
| `shared_test_cookies.json` | 测试服务登录凭证 |
| `execution.json` | 课程清单 |
| `homework\` | 平时测试草稿与检查报告 |
| `qr\` `logs\` | 二维码图片、运行日志 |

> 分享或发布时不要带上 `cookies.json` / `shared_test_cookies.json`，里面是登录态。

---

## 命令行

```bat
zhs-brush.exe                      :: 打开图形界面（会带控制台）
zhs-brush.exe login                :: 扫码登录，写 cookies.json
zhs-brush.exe fetch                :: 拉共享课清单，写 execution.json
zhs-brush.exe run                  :: 按 execution.json 刷全部课
zhs-brush.exe run -c <ID> -l 25 --speed 3
zhs-brush.exe --data-dir D:\zhs run
```

验证码风控时：命令行输入 `go` 回车 = 已解完（`skip` 放弃）；界面里点「我已解完，继续刷课」。

---

## 代码结构

```
src/
├── main.rs        CLI（clap）：gui / login / fetch / run
├── bin/gui.rs     Windows 子系统入口（无控制台）
├── config.rs      配置合并读写、窗口 / 主题 / AI 设置
├── crypto.rs      AES-128-CBC、getEv、WatchPoint
├── cookies.rs     CookieJar ↔ cookies.json
├── http.rs        会话层：Cookie 注入、手动跟随重定向、单次请求、重试、代理
├── model.rs       课程/章/节/视频模型、parse_chapters、Value 取值助手
├── api/
│   ├── mod.rs       zhidaoQuery 通用封装（加密 / code 校验 / -12 识别）
│   ├── passport.rs  扫码 + 密码登录
│   ├── zhidao.rs    studyservice / onlineservice 全部接口
│   ├── exam.rs      考试服务：RSA 密钥交换、AES 请求体、开卷/保存/交卷、题型校验
│   └── ai.rs        OpenAI 兼容求解器（```answer``` 解析、串行重试）
├── homework.rs    平时测试编排：读取检查 / 生成 / 保存核验 / 交卷 / 批量
├── study.rs       上下文构建 + 刷课主循环 + 验证码等待
├── events.rs      界面↔工作线程的事件与控制位
├── gui/mod.rs     egui 界面（主题、二维码、AI 配置弹窗、日志、进度）
└── util.rs        数据目录、HMS、日志、打开路径
build.rs           嵌入 exe 图标与版本信息（windres）
assets/            icon.ico / icon.png
examples/          诊断工具：probe / probe_popup / probe_exam / probe_list / probe_check …
```

```bat
cargo test        :: 23 项单测：AES 往返、getEv、HMS、Cookie、配置合并、章节解析、
                  :: 弹题筛选、考试链接解析、RSA 信封、答案校验、HTML 清洗、AI 配置写回
```

---

## 构建细节

`构建.bat` 处理了两个 Windows 上的坑：

1. **中文路径**：raw-dylib 的 `dlltool` 会按 GBK 解析路径导致失败。脚本 `subst` 一个纯 ASCII
   盘符编译，结束后立即解除。
2. **dlltool 竞争**：多线程编译时 `windows-sys` / `parking_lot_core` 同时调用 dlltool 会抢临时文件。
   命中相关错误时自动退回 `-j 1` 重编。

`.cargo/config.toml` 指定 `rust-lld`，不依赖 MSVC 的 `link.exe` 或 MinGW 的 `ld`；
exe 图标由 `build.rs` 通过 windres 嵌入，缺失时仅警告、不中断编译。

---

## 已知限制

- 只处理普通共享课（`meetCourseType = 0`），见面课、翻转课、AI 课考试未实现
- 平时测试仅支持单选 / 多选 / 判断，其它题型整卷停止
- 平台风控（滑块）无法通过 API 直接解除，需要人工在浏览器完成一次
- AI 答案可能出错，成绩以智慧树平台为准；已提交的试卷不会为了提高分数自动重做

---

## 致谢

接口协议、加密参数与刷课节奏来自原项目 **ZHS-Fucker-Pro**（Python 实现）。本仓库是独立
重写，与其共享 `config.json` / `cookies.json` 格式。

## 许可证

本项目基于 **Apache License 2.0** 授权，全文见 [LICENSE](LICENSE)。

```
Copyright 2026 ZHS-Brush-RUST contributors

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0
```
