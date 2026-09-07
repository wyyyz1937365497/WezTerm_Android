# P1-A：WezTerm 终端核心与 Android cell 显示闭环

验证时间：2026-09-07（Asia/Shanghai）

## 结论

P1-A 通过。固定输入字节不再由演示代码直接布置到画面，而是先进入上游
`wezterm-term::Terminal::advance_bytes`，经过 WezTerm 的 escape parser、screen、
line 和 cell 模型，再由 `TerminalSnapshot` 上传给 Android wgpu renderer。

真机已经显示从 WezTerm screen 读取出的 ASCII、双宽 cell、非 ASCII 占位和真实
cursor 位置。P1-A 使用公共领域 8×8 bitmap font 临时显示 ASCII；它只用于确认
cell 到 GPU 的数据通路，不能作为 WezTerm 字体、shaping 或最终渲染已完成的证据。

## 固定上游版本

```text
repository: https://github.com/wezterm/wezterm.git
revision:   d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b
```

revision 同时写在 `rust/wezterm-android-core/Cargo.toml`、`Cargo.lock` 和运行时日志中。

## Gate 结果

| Gate | 结果 | 依据 |
|---|---|---|
| 上游 pin | PASS | Git dependency 使用完整 revision |
| `wezterm-term` Android check | PASS | `cargo ndk -t arm64-v8a -P 24 check -p wezterm-term` |
| core 主机测试 | PASS | 2 passed, 0 failed |
| Android ARM64 link | PASS | Gradle/cargo-ndk 全量构建成功 |
| WezTerm cell → GPU | PASS | 真机可读文本与 runtime snapshot 日志 |
| 双宽 cell | PASS（模型） | `A中B` 的 column 为 0/1/3，中文 width=2 |
| resize 保持模型 | PASS | 单元测试及同 PID Surface 重建 |
| 16 KB ELF/APK 对齐 | PASS（静态） | ELF `LOAD Align=0x4000`; zipalign PASS |
| X11-free 动态依赖 | PASS | 仍只有 Android/Linux 系统库 |
| WezTerm 字体 shaping | NOT IMPLEMENTED | P1-B |
| cell 颜色/属性渲染 | NOT IMPLEMENTED | parser 已处理，snapshot 尚未携带 |
| SSH | NOT IMPLEMENTED | P2 |

## 单元测试

命令：

```bash
cargo test --manifest-path rust/Cargo.toml -p wezterm-android-core --locked
```

结果：

```text
running 2 tests
test tests::parses_ansi_and_preserves_wide_cell_columns ... ok
test tests::resizes_without_recreating_the_terminal_model ... ok
test result: ok. 2 passed; 0 failed
```

第一项测试的关键断言为：

```text
A: column=0 width=1
中: column=1 width=2
B: column=3 width=1
cursor: column=4 row=0
```

## Android 15 真机结果

设备环境沿用 P0。冷启动日志：

```text
adapter name=Mali-G615 MC6 backend=Vulkan type=IntegratedGpu
configured native surface 2800x1896 density=400 format=Rgba8UnormSrgb present=Fifo
P1-A WezTerm terminal snapshot is ready
revision=d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b
grid=101x36 occupied=205 cursor=0,8
```

切到系统设置再返回时 PID 保持为 `4982`，日志顺序为：

```text
released ANativeWindow
destroyed wgpu Surface while retaining the WezTerm terminal model
configured native surface 2800x1896 ...
P1-A WezTerm terminal snapshot is ready ... occupied=205 cursor=0,8
```

这证明 GPU Surface 可以独立销毁和重建，Rust terminal model 没有绑定到
`ANativeWindow` 生命周期。

## 静态兼容检查

ELF 四个 load segments：

```text
LOAD ... R   0x4000
LOAD ... R E 0x4000
LOAD ... RW  0x4000
LOAD ... RW  0x4000
```

动态依赖：

```text
libandroid.so
liblog.so
libdl.so
libc.so
libm.so
```

没有 X11、XCB、Wayland 或 D-Bus 动态依赖。当前测试设备仍是 4 KB 页设备，
16 KB 结论仍是静态兼容性结论。

## 产物

下列 APK/`.so` 哈希是 P1-A 通过时的历史记录；同一路径随后已被 P1-B 构建覆盖。
长期保留的 P1-A 证据是截图文件。

| 文件 | 大小 | SHA-256 |
|---|---:|---|
| `app/build/outputs/apk/debug/app-debug.apk` | 17,097,655 bytes | `ba437c5ebb1750e4ebf44cfa796f5906f325d54e3e114c7ddc45b624c22a6776` |
| `app/build/generated/rustJniLibs/p0/arm64-v8a/libwezterm_android.so` | 15,856,160 bytes | `ad6af0a3feadb06d2a513a4451cd4d9b7b437533c857a94a0548f18521d02f6c` |
| `artifacts/p1a-terminal-core/android15-wezterm-cells.png` | 67,168 bytes | `9d397e337d460a64611bee89217b52ae012be407eb081bbd64850984448d760d` |

APK 为 debug 构建，不是发布签名产物。`generated/rustJniLibs/p0` 是尚未重命名的
Gradle 中间目录，其名称不代表当前 gate 仍为 P0。

## 当时的下一步边界

P1-B 将处理 `wezterm-font`、真实 font fallback/shaping、glyph atlas 和 cell 属性。
在 P1-B 通过之前，当前画面中的 CJK/emoji 方框只能证明 cell 宽度和 GPU 位置，
不能证明中文或 emoji 字形可正确显示。

P1-B 随后已完成真实 HarfBuzz/FreeType alpha glyph atlas 和 CJK fallback；其结论与
当前产物以 `P1B_FONT_ATLAS_GATE.md` 为准。彩色 emoji 仍未完成。
