# Android 原生 WezTerm 客户端路线图

## 总目标

建立一个单 Activity、客户端优先的 Android 前端：

```text
Android UI / IME / lifecycle
              ↓ JNI
Rust client core
  ├─ WezTerm terminal model
  ├─ WezTerm font shaping and glyph cache
  ├─ Android wgpu renderer
  └─ SSH, then SSHMUX/TLS domains
```

不会把现有 `wezterm-gui` 整体交叉编译，也不会引入 X11 兼容层。

## 阶段与停止条件

### P0：平台显示闭环 — PASS

- Android 原生 Surface；
- Rust/JNI 所有权；
- wgpu/Vulkan 绘制；
- Surface 销毁与重建；
- IME 事件桥冒烟；
- ARM64 和 16 KB 静态兼容检查。

证据见 `P0_NATIVE_SURFACE_GATE.md`。

### P1：WezTerm 终端字符闭环 — PASS

目标：把固定字节流送入 WezTerm 终端状态机，并在 Android Surface 上显示真实
cell 内容，而不是 P0 网格 shader。

已完成 P1-A：

- 固定并引入上游 WezTerm revision；
- ARM64 交叉编译 `wezterm-term`/`termwiz`；
- 建立独立 `wezterm-android-core` 和只读 `TerminalSnapshot`；
- 真机画面由 WezTerm cell 数据驱动；
- ANSI、中文双宽 cell 与 resize 主机测试通过；
- terminal model 与 Surface 生命周期分离。

已完成 P1-B：

- 完整 `wezterm-font` 的 Android 预检定位到桌面依赖耦合，未把 OpenSSL 当作字体依赖引入；
- 复用同一上游 revision 中的 FreeType/HarfBuzz 包，建立 Android 专用字体 seam；
- 打包 JetBrains Mono，并在 Android 上使用 Noto Sans CJK 系统 fallback；
- 接入 HarfBuzz shaping、FreeType alpha rasterization 和 wgpu glyph atlas；
- cell 快照携带前景色、背景色、强度、下划线、删除线等属性；
- ASCII、ANSI/TrueColor、中文双宽 cell、组合字符和 Surface 重建通过真机验证；
- 删除 P1-A 的 `font8x8` 路径。

停止条件：真机画面来自 WezTerm terminal cell 数据，并通过宽字符与组合字符的
确定性用例。该条件已满足，证据见 `P1B_FONT_ATLAS_GATE.md`。P1 不包含 SSH；
彩色 emoji、复杂多 glyph cluster、完整粗斜体 face 选择和通用 Android 字体发现
作为后续渲染强化项保留，不冒充已完成。

### P2：单 SSH 连接闭环 — PASS

状态：已完成普通 SSH、远端 PTY、应用私有凭据路径和 Surface 生命周期解耦。

- Android 网络权限；
- 固定测试服务器的 host-key 校验；
- 密钥导入和应用私有存储；
- 单 pane SSH 读写；
- 网络中断与 Activity 重建解耦。

停止条件：硬件键盘可完成交互式 SSH，会话不会因 Surface 重建而退出。

### P3：移动端日用输入与交互 — 核心闭环 PASS

- 固定底部英文/符号/特殊键盘 — PASS；
- 独立中文 composing 编辑框与整串 UTF-8 发送 — PASS；
- 单指通过 WezTerm `Pane::mouse_event` 向远端 TUI 发送滚轮事件 — PASS；
- 双指拖动浏览 WezTerm scrollback、历史位置提示与一键回到底部 — PASS；
- 长按按词选择、拖动扩选和选择可见屏幕 — PASS；
- Android 系统剪贴板复制/粘贴 — PASS；
- 多客户端远端行数不一致时的 Android 底部对齐视口 — PASS；
- 链接识别与点击；
- Android 原生选择手柄；
- Esc/Ctrl/Alt/Tab/方向键辅助栏；
- 横竖屏、分屏、软键盘和 Insets 专项测试。

当前中文输入刻意采用独立编辑框整串发送，不把系统 IME 的 composing 区直接映射到
终端，因此不以伪造 surrounding-text 为完成条件。证据见
`P3_TOUCH_CLIPBOARD_GATE.md`。

### P4：持久远程客户端

- SSHMUX 上游客户端复用、标签控制、安全分离与重新附着 — ANDROID 15 RUNTIME PASS；
- Activity 后台且进程存活时保持连接 — ANDROID 15 RUNTIME PASS；
- 后台 transport 脱离但 session handle 残留时自动检测、安全清理并重附着 — ANDROID 15 RUNTIME PASS；
- 跨应用 Surface producer 释放/重建与键盘 Resize 压力 — ANDROID 15 RUNTIME PASS；
- 前台网络故障指数退避重连、冷启动自动重附着 — ANDROID 15 RUNTIME PASS；
- 显式 Detach 关闭自动重附着 — ANDROID 15 RUNTIME PASS；
- TLS domain；
- Android Keystore；
- 前台服务、Doze 与跨网络长时后台压力测试。

### P5：发布工程化

- release profile、体积和启动耗时；
- 16 KB 页真机/模拟器运行验证；
- crash reporting 与隐私审计；
- 签名、版本升级和兼容矩阵。

## 明确排除项

第一阶段不做：

- 本地 PTY/shell；
- 本地 mux daemon；
- Unix socket 单实例；
- 多 Android 顶层窗口；
- X11/Wayland；
- 桌面更新器、桌面通知与串口。
