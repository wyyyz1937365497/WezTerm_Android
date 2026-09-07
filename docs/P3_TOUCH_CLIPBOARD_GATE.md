# P3：触摸回滚、选择与剪贴板验证记录

日期：2026-09-07

## 结论

移动端终端交互核心闭环已在 Android 15/API 35 ARM64 真机通过：可浏览 WezTerm
scrollback，长按字符网格进入选择模式并拖动扩展，复制到 Android 系统剪贴板，再把
剪贴板内容粘贴回当前 SSHMUX pane。功能直接作用于 WezTerm cell/稳定历史行，不是
对截图或 Android TextView 做选择。

| 检查项 | 状态 | 实机结果 |
|---|---|---|
| scrollback 快照与边界钳制 | PASS | `seq 1 100` 后从实时底部回滚 12 行，画面由 `86–100` 切换到 `74–90` |
| 历史位置与回到底部 | PASS | 状态显示“位于实时底部上方 12 行”，`↓ 实时` 返回提示符 |
| 多客户端高度差 | PASS | 远端物理 pane 为 33 行时，Android 17 行视口仍从底部显示提示符 |
| 长按选择与拖动扩选 | PASS | 长按 `91` 按词选择，蓝色选区和浮动操作栏可见 |
| 系统剪贴板复制 | PASS | Copy 提示“已复制 2 个字符” |
| 系统剪贴板粘贴 | PASS | Paste 提示“已粘贴 2 个字符到终端”，远端 zsh 收到字面量 `91` |
| 原远端会话保护 | PASS | 测试仅使用临时 tab，结束后关闭临时 pane，原 `pane=32` 保留 |

## 数据与交互路径

```text
ClientPane stable rows / wezterm-term scrollback
                 ↓
TerminalSnapshot(viewport_offset, max_viewport_offset, wrapped_rows)
                 ↓
AndroidViewport：按 Android 行数从远端物理视口底部取行
                 ↓
wgpu 字符网格 + 选区背景
                 ↑
TerminalSurfaceView GestureDetector
  ├─ drag / fling -> nativeScrollByRows
  └─ long press / drag -> nativeSelectionStart / Update
                 ↓
ActionMode Copy / Paste / Select visible screen / Cancel
                 ↓
Android ClipboardManager <-> active SSH/SSHMUX pane
```

核心 terminal model 新增按 scrollback offset 生成只读快照，offset 会钳制到当前有效
历史范围。选区使用视口 cell 坐标，起止点可反向拖动；文本抽取按行序输出，双宽字符
的 continuation cell 回映射到原 grapheme，软换行不插入多余换行。用户发送终端输入时
会自动回到实时底部，避免在历史画面中输入却看不到回显。

如果桌面客户端同时附着同一远端 pane，它可能把远端物理尺寸保持得高于 Android
Surface。Android 端不再直接取物理视口顶部，而是在远端稳定行范围内建立底部对齐的
逻辑视口，并把上方隐藏的物理行计入可回滚范围。因此 17 行手机视口不会丢掉底部
prompt，也不会为了显示历史去破坏另一个客户端的尺寸。

## 真机步骤与证据

测试设备为 Android 15/API 35 ARM64，横屏 `2800x2000`，终端 Surface 为
`[0,334]–[2800,1266]`，即固定键盘上方的 `101x17` 字符区域。

1. 在临时 SSHMUX tab 执行 `seq 1 100`，确认实时画面含 `86–100` 和 zsh prompt；
2. 向下拖动终端，状态变为距实时底部 12 行，画面显示 `74–90`；
3. 回到实时底部后长按 `91`，确认蓝色选区及浮动操作栏；
4. 点击 Copy，确认复制 2 字符；再次打开操作栏点击 Paste，确认远端输入行出现 `91`；
5. 清除测试输入并关闭临时 tab，只保留原远端 pane。

证据图：

- [历史回滚与位置提示](../artifacts/p3-touch-selection/scrollback-history.png)
- [长按选择及浮动操作栏](../artifacts/p3-touch-selection/long-press-selection-3.png)
- [系统剪贴板粘贴回远端终端](../artifacts/p3-touch-selection/clipboard-pasted-2.png)

## 自动化校验

- `wezterm-android-core` 10 个测试通过，包括 scrollback 边界、宽字符和跨行文本抽取；
- `wezterm-android-mux` 4 个测试通过，包括 Android 底部对齐视口；
- font/ssh 合计 11 个测试通过；四个交付 crate 共 25 个测试串行通过；
- Kotlin/JVM 5 个应用内键盘规格测试通过；
- native host check、ARM64 native build、debug APK assemble 和安装通过。

遵循项目约定，本阶段没有计算或生成 SHA-256。

## 当前边界

- 当前是字符网格矩形/线性文本选区，没有 Android 原生左右选择手柄和自动边缘滚动；
- 尚未实现 URL/OSC 8 链接识别与点击；
- 鼠标报告模式与普通终端滚动的手势仲裁尚未专项验证；
- 横竖屏、分屏、系统 IME 弹出和长 scrollback 的压力测试仍需后续覆盖。
