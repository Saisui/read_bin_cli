# Changelog

## v0.2.0 (2026-06-18)

### mmap + overlay 架构
- 移除 `to_vec()` 全量拷贝，改用 mmap 只读映射直接读取
- 编辑内容存储在 overlay HashMap 中，`byte_at()` 优先读 overlay
- `save_with_overlay()` 写入 mmap + overlay 分段，避免全量拷贝
- 1GB 文件堆内存从 ~1GB 降至 ~0

### 新增 CLI 参数
- `--copy`：临时文件快照模式（外部修改不可见）
- `--track`：50ms 轮询文件变更检测
- `--inotify`：inotify 事件驱动跟踪（Linux/Android，零延迟）
- `--immediate` / `--imm`：每次编辑直接 pwrite 写磁盘
- `--lock none/4k/full`：文件锁定（full 用 flock SH，4k 用 fcntl 范围锁）
- `--lock-4k` / `--lock-full` / `--unlock`：快捷别名
- `--help` / `-h`：用法帮助

### 模式菜单
- 点击顶栏 [filesize] 或按 M 打开模式菜单
- 运行时切换 Track / Inotify / Immediate / Lock
- Copy 显示为只读（灰色，底部）
- 键盘：j/k 导航，Enter/t/n/i/l 快捷切换

### 顶栏
- 格式改为 `[filesize-mods] *filename`
- 文件名过长时截断并保留扩展名（`...`）
- 模式标志：`i`=immediate `f`=full `4`=4k `t`=track `T`=inotify `c`=copy
- 响应窗口大小变化

### Bug 修复
- 修复：Sample 闪退（`run()` 返回值未处理 `pending_file`）
- 修复：`args[0]` 被当作文件名（跳过程序名）
- 修复：immediate 模式 fd 被立即 drop（File 必须保持存活）
- 修复：immediate 模式退出应跳过保存确认

### 其他
- panic hook 自动清理 `--copy` 产生的临时文件
- flock / fcntl / inotify 运行时动态切换
- `poll_track_event()` 平台抽象层
- `flush_byte()` / `flush_last()` 支持 immediate 模式逐字节写入

---

## v0.1.10 (2026-06-18)

### Sample 文件
- 文件浏览器顶部新增 `*sample (0x00-0xFF)` 条目（斜体青色），回车即可打开 256 字节示例
- Sample 走正常文件打开流程（临时文件），退出后返回原文件

### 菜单增强
- MENU 下拉菜单项首字母带下划线（H̲elp / S̲ample / A̲bout）
- 菜单打开时支持 h/s/a 快捷键直接跳转

### Bug 修复
- 修复 Sample 闪退：`run()` 返回值未考虑 `pending_file`，导致 Sample 打开后立即退出程序
- 修复 Sample 写入失败静默忽略：改为检查 `is_ok()` 再设置 pending_file

### 文档
- About 弹窗添加功能亮点（BitSearch / Sparse Hierarchical Bitmap / 8 色彩模式 / 编辑 / Sample）
- CODE_NAV.md 更新：InputMode 枚举、FileBrowser、菜单快捷键、废弃代码标注
- README.md 更新：菜单快捷键、File Browser + *sample

---

## v0.1.9 (2026-06-18)

### 新增显示模式
- Gray 灰阶：背景 RGB(v, v, v)，v = 字节值
- Heat 热力图：黑→蓝→红→黄→白 四段线性插值
- hsl 位分解：高4位=色相，中2位=亮度，低2位=饱和
- rgb 位分解：RR_GGGG_BB 2:4:2 位域（R×85, G×17, B×85）

### 模式菜单重构
- 新增 off 选项（关闭所有色彩模式）
- 色彩模式改为 radio 单选（选中项背景色高亮）
- 分隔线改为纯横线
- 菜单宽度优化（14→10）

### 编辑模式
- 256 色模式下底栏字节值背景为其调色板色，前景自适应
- 背景压暗改为 45%，Gray 模式不压暗
- Alt+↑/↓ 字节值 ±1（到限不动）
- 状态栏显示地址和页码

### 显示改进
- ASCII 模式：\t → ↹，\r → ↵

### Bug 修复
- 底栏 [HELP] 点击区域与实际显示位置不重合（硬编码宽度 → 实际长度）
- 底栏地址显示改为 & xxxx

---

## v0.1.8 (2026-06-18)

### 搜索引擎重构
- 新增 `bitmap.rs`：四级位图搜索引擎（L0~L3，固定 804 字节内存）
- 按需扫描，不预加载全文件，不拷贝 data.to_vec()
- 删除后台线程、mpsc channel、ranges Vec
- 内存从 ~2GB（1GB 文件）降至 804 字节

### 状态栏
- 字节位置前缀从 `@` 改为 `&`（如 `&1a3f`）
- 搜索时显示：`[项次/项数+] @当前页/总页`
- 点击 `&address` 跳转字节地址

### 搜索模式导航
- 所有导航键按各自步长跳到目标页第一个匹配
- ←→: ±1页, J/K: ±1屏, PGDN/PGUP: ±半屏
- H/L: ±16页, O/P: ±256页, Home: 第一个匹配
- ↑↓: 逐个匹配（不变）
- 目标页无匹配时继续向后扫描

### 代码质量
- bitmap.rs 全部 pub 函数有 `///` 文档注释
- 搜索模块职责分离：search.rs 负责模式解析，bitmap.rs 负责索引引擎

---

## v0.1.7 (2026-06-15)

### Code quality
- `run()` split into `render_frame()`, `handle_mouse_event()`, `handle_key_event()`
- Doc comments added to all pub functions and fields
- Code standards documented in CODE_NAV.md

### Visual
- Line numbers 00-FF with blue→purple→pink gradient

---

## v0.1.6 (2026-06-15)

### Top bar
- File size and name displayed in top bar
- Click top bar to open file browser

### Flicker fix
- Shared terminal between file browser and hex viewer (no teardown/rebuild)

### Help
- Help text moved to `help.txt` (external, easy to edit)
- Help scrollbar draggable (click + drag)

### File browser
- Click again on same item to open (touch-friendly)
- Mouse click and scroll wheel support

### Bug fixes
- Selection start byte now highlights correctly (sp(18) → sp(15))

---

## v0.1.5 (2026-06-15)

### File browser
- No-argument startup shows file browser (navigate dirs, select file)
- Ctrl+P opens file browser from hex viewer (switch files without restart)

### Search
- Ctrl+F global search (works in all modes including Edit)

### Scroll
- Scroll wheel crosses page boundaries (uses global rows)

### Bug fixes
- Windows: OnceLock color config idempotent on re-entry

---

## v0.1.4 (2026-06-15)

### Cross-page seamless scrolling
- Scroll past page boundaries without stopping — pages connect seamlessly
- Global row numbering for seamless navigation across all packs
- Row numbers show within-page offset (0x00-0xff), resetting at each page boundary
- Up/Down/j/k, PageUp/PageDown, Home all work cross-page
- Mouse click and drag work across page boundaries
- O/P (±1MB jump) uses global rows

### Status bar improvement
- Pack display shows last visible page number (not first)

---

## v0.1.3 (2026-06-15)

### UI overhaul — Status bar
- Removed top 2 rows (filename+size, pack+mode), replaced with clickable status bar
- Status bar layout: `[ASCII]  @00000042  pack 1/a  Ctrl+H:help`
- Pack info displayed in hexadecimal format

### Mode dropdown menu
- Click `[ASCII]` to open dropdown: ASCII / HEX / UTF8 / [256] checkbox
- Keyboard: 1/2/3 to select mode, 4/n to toggle 256-color
- Current mode highlighted in dropdown

### 256-color gradient label
- When 256-color enabled, `[ASCII]` renders with per-character gradient background
- 7 characters × 7 colors: blue → indigo → purple → magenta → pink → light pink
- White foreground on gradient background

### Clickable status bar regions
- Click `@address` → goto byte address input
- Click `pack` → goto pack number input
- Click `Ctrl+H:help` → open help window
- Jumping sets cursor position and focus

### Mouse drag selection
- Click and drag to select byte range
- Selection highlighted with sp(15)/sp(17) styles
- Click outside clears selection
- Status bar protected from selection bleed (Clear widget)

### Ctrl+C clipboard copy
- Added `arboard` dependency for cross-platform clipboard
- Mode-aware copy output:
  - ASCII: printable chars as-is, non-printable as `.`
  - HEX: space-separated hex bytes (`48 65 6c 6c 6f`)
  - UTF8: decoded UTF-8 characters (multi-byte sequences properly decoded)

### Windows double-click fix
- Root cause: crossterm 0.28 on Windows generates both Press and Release events for each keypress; code never checked `key.kind`
- Fix: filter `KeyEventKind::Release` events (all platforms)
- Additional 40ms throttle for Windows auto-repeat (`#[cfg(target_os = "windows")]`)
- Removed old fragile 20ms debounce that only covered scroll keys
- No impact on Termux/Linux/macOS (Repeat events not generated on those platforms)

### Internal
- New `InputMode::ModeSelect` and `InputMode::GotoByteInput` variants
- New `DisplayMode::prev()` method
- New `App.dragging` state field
- `max_rows` changed from `saturating_sub(4)` to `saturating_sub(2)`

---

## v0.1.2 (2026-06-14)

### 256-color display mode
- New 256-color background display mode via `n` key toggle
- Each byte rendered with its terminal palette color as background
- Auto foreground: luminance-based black/white text for readability
- Applied across ASCII, HEX, and UTF8 display modes

### CI/CD
- Added cross-compilation for Linux and Windows
- Added aarch64 (ARM64) build for Termux/Raspberry Pi
- Binary trimmed to ~564K with `opt-level = "s"` + LTO + strip

---

## v0.1.1 (2026-06-14)

### Search engine overhaul
- Replaced regex-based search with custom nibble pattern matching
- New search syntax: exact hex (`4f2a`), nibble wildcards (`4x`), ranges (`[0-3]f`), any byte (`z`)
- Three-level bitmap index (L0=pack, L1=1MB, L2=1GB) for fast skip
- Background incremental search via channel
- Search input: `f` for hex/nibble, `F` for plain string

### Color configuration
- YAML-based color config (`color.yaml`)
- `fg: auto` — auto-select black/white foreground based on background luminance
- Terminal palette sync from `~/.termux/colors.properties`
- Named colors + RGB `[r, g, b]` format
- Dim alternating effect for same-type consecutive bytes

### Display improvements
- Gradient column header (blue → green)
- UTF-8 mode with cross-row multi-byte sequence handling
- Display width calculation for CJK characters (width=2)
- Byte classification: null, control, ASCII, hex, head2/3/4, tail

### Help popup
- Interactive help with `?` / `Ctrl+H`
- Scrollable help content
- Version and author display

### Navigation
- Full keyboard navigation: j/k/h/l, J/K/H/L, PgUp/PgDn, Home
- O/P for ±1MB area jump
- g/Ctrl+G for goto offset (hex input)
- Mouse wheel scroll support

### Edit mode
- `i` to enter edit mode, ESC to exit
- Hex nibble editing in HEX mode
- Character editing in ASCII/UTF8 mode
- Ctrl+Z undo, Ctrl+Y redo
- Tab inserts `\t`, Enter inserts `\n`

### Selection
- `Alt+J` to set selection start
- `Alt+K` to set selection end

### File operations
- `Ctrl+S` to save
- `q` / `Ctrl+Q` to quit with save prompt if modified
- Memory-mapped file I/O via `memmap2`

---

## v0.1.0 (2026-06-13)

### Initial release — Rust TUI hex viewer
- Terminal-based hex viewer/editor written in Rust
- Three display modes: ASCII, HEX, UTF-8
- Pack-based file browsing (4096 bytes per pack)
- Basic navigation: arrow keys, j/k/h/l
- Byte editing with cursor
- Save to file
- Color configuration support
- Dependencies: ratatui (TUI), crossterm (terminal), memmap2 (mmap)

### Original Python version
- `read_bin.py` — Python/curses hex viewer (original)
- `read_bin_new.py` — Python/curses hex viewer (enhanced)
- `read_bin_win.py` — Windows-specific Python version
- `read_bin_termux.py` — Termux-specific Python version

### Original Ruby version
- `read_bin.rb` — Ruby hex viewer (original)
