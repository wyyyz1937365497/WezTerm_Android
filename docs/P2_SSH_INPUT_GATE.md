# P2：单 SSH 与移动输入验证记录

日期：2026-09-07

## 结论

普通 SSH 客户端闭环已在 Android 15/API 35 ARM64 真机通过：应用直接复用固定 WezTerm
revision 的 `wezterm-ssh`/libssh，完成主机密钥确认、密码或应用私有密钥认证、
`xterm-256color` PTY、远端输出、输入和 Resize。Android 端不启动本地 PTY、shell、
mux daemon 或 X11 服务。

## 连接与凭据边界

- host key 首次连接由 Android 对话框显示并确认，接受后写入应用私有 `known_hosts`；
- host key 变化会作为阻断错误显示，不自动覆盖旧记录；
- 密码/keyboard-interactive 答案只在当前对话框与认证请求中存在，不写入偏好或日志；
- 默认 SSH agent 和用户目录中的身份文件被禁用，只允许显式位于应用私有 SSH 目录
  下的绝对、规范化 identity 路径；
- `scripts/provision-debug-identity.sh USER@HOST [ADB_SERIAL]` 仅为 debug 构建生成独立
  Ed25519 key，私钥不打包进 APK，也不进入仓库。

## 输入与布局

固定底部应用内键盘提供英文、Shift 符号层、Ctrl/Alt/Esc/Tab、方向与编辑键以及
F1–F12；右侧功能键使用 4 列网格，键盘作为普通布局子 View 消耗高度，不覆盖
Surface。中文输入只在独立 `EditText` 内交给系统 IME 完成 composing，点击发送后
才把完整 UTF-8 字符串写入远端 PTY，因此不需要伪造终端的 surrounding text。

Surface Resize 会同步 WezTerm terminal model 和远端 PTY 行列。Surface 销毁只释放
wgpu/`ANativeWindow`，不会销毁 terminal model 或 SSH session；普通 SSH 在进程被系统
结束或网络断开后不能恢复同一个 shell，这也是后续采用 SSHMUX 的原因。

## 校验

- `wezterm-android-ssh` 5 个主机测试通过，覆盖隔离配置、identity 路径、端点校验和
  Android 事件 JSON；
- Kotlin/JVM 5 个键盘规格测试通过；
- ARM64 native 构建、debug APK assemble、安装及 Android 15 真机输入闭环通过；
- 密码不持久化，native 日志只记录输入字节数/字符数，不记录输入内容。

遵循项目约定，本阶段没有计算或生成 SHA-256。

## 当前边界

- 普通 SSH 不提供跨进程会话持久化，应使用已实现的 SSHMUX；
- debug identity 依赖开发者显式 provision，尚未接入 Android Keystore；
- release 凭据导入、文件选择器和密钥口令 UI 尚未完成；
- Wi-Fi/蜂窝切换、Doze、横竖屏和分屏仍需专项压力测试。
