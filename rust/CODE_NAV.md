# read-bin 代码编辑导航

## 项目概览

终端 TUI 十六进制查看/编辑器，Rust 实现。支持跨页无缝滚动、鼠标拖拽选区、256 色渐变。

**v0.2.0 新增**：mmap+overlay 架构、即时写盘（pwrite）、文件变化跟踪（poll/inotify）、文件锁、模式菜单、临时文件快照

**依赖**：ratatui（TUI 框架）、crossterm（终端控制）、memmap2（内存映射文件）、arboard（剪贴板）

**构建**：`cargo build --release`，产物在 `target/release/read-bin`

## 文件结构

```
src/
├── main.rs          # 入口 + TUI 事件循环 + 渲染 + 输入处理（~4100 行）
├── app.rs           # 应用状态管理（跨页滚动、光标、搜索、undo/redo）（~706 行）
├── bitmap.rs        # 四级位图搜索引擎（L0~L3，804 字节固定内存）
├── modified.rs      # Sparse Hierarchical Bitmap（稀疏层级位图，追踪编辑字节）
├── color_config.rs  # YAML 颜色配置加载 + fg: auto 逻辑
├── search.rs        # 搜索模式解析（hex/nibble/字符串 → Needle）
└── utf8.rs          # UTF-8 字节分类与解码
```

## 模块依赖关系

```
main.rs
 ├── app.rs          （App 状态、跨页滚动、搜索导航）
 ├── bitmap.rs       （BitSearch 四级位图搜索引擎）
 ├── modified.rs     （ModifiedMap 稀疏层级位图）
 ├── color_config.rs （ColorConfig 颜色配置）
 ├── search.rs       （Needle 搜索模式解析）
 └── utf8.rs         （ByteClass 字节分类）
```

## 关键数据流

```
用户输入 → handle_*() → App 状态更新 → resolve() → style_for() → 渲染
```

---

## v0.2.0 架构变更

### mmap + Overlay 架构

**设计变更**：移除了 `to_vec()` 全量加载。文件通过 mmap 只读映射，所有编辑写入 `overlay: HashMap<usize, u8>`（内存中的脏字节表），保存时通过 `save_with_overlay()` 将 mmap 基础数据与 overlay 合并写回磁盘。

```
文件磁盘 → mmap (只读) ─┐
                         ├→ 渲染：先查 overlay，无则用 mmap
overlay (HashMap<u8>) ───┘

保存：mmap[0..n] 分段 + overlay 填充 → 写文件
```

**核心函数**：
- `save_with_overlay()`：分段写入，按 overlay key 排序，合并相邻脏区间，减少 write 系统调用
- `byte_at()`（app.rs）：overlay-first 字节读取，先查 HashMap，无则读 mmap

### Immediate Mode（即时写盘）

`--immediate` / `--imm` 标志。每次字节编辑立即通过 `pwrite(2)` 写入磁盘，不等用户 Ctrl+S。

- `flush_byte()`：单字节 pwrite，用于光标编辑
- `flush_last()`：最后一个编辑字节的 pwrite（批量编辑后 flush）
- 依赖 `pwrite(2)` 系统调用（extern "C"），避免影响 mmap 文件偏移

### Tracking Modes（文件变化跟踪）

检测外部文件修改，自动重新加载。

- `--track`：50ms 轮询检测文件 mtime 变化（`poll_track_event()`）
- `--inotify`：inotify 事件驱动检测（Linux/Android，`poll_track_event()`）
- `poll_track_event()`：平台特定事件轮询，返回 `bool`（是否需要重载）
- `last_modified` 字段：记录上次检测的文件修改时间

### Lock Modes（文件锁）

防止多进程并发写冲突。

- `--lock none`：不加锁（默认）
- `--lock 4k` / `--lock-4k`：4K 范围锁（fcntl F_SETLK，仅锁当前编辑区域）
- `--lock full` / `--lock-full`：全文件排他锁（flock LOCK_EX）
- `--unlock`：运行结束时主动解锁
- `flag_lock` 字段：`&'static str`，存储当前锁模式名

### CLI 标志

```
--copy           临时文件快照模式（复制到 temp 目录编辑，保存时写回原文件）
--track          50ms 轮询跟踪文件变化
--inotify        inotify 事件驱动跟踪
--immediate/--imm  每次编辑立即 pwrite 写盘
--lock none/4k/full  文件锁模式
--lock-4k        等价 --lock 4k
--lock-full      等价 --lock full
--unlock         退出时解锁
--help / -h      帮助输出
--dump           纯文本 hex dump 输出（非 TUI）
```

### Mode Menu（运行时模式切换）

点击顶栏 `[filesize-mods]` 区域或按 `M` 键弹出模式菜单，在运行时切换模式标志。

- `InputMode::ModeMenu`：模式菜单弹窗状态
- `mode_menu_selected`：菜单当前选中项
- `draw_mode_menu_dropdown()`：渲染下拉菜单
- `mode_menu_rect()`：计算菜单位置矩形

**模式标志指示器**（顶栏显示）：
```
i=immediate  f=full-lock  4=4k-lock  t=track  T=inotify  c=copy
```

### 顶栏格式

```
[1234-5i] *filename.bin        ← 文件大小 + 修改计数 + 模式标志，文件名中间截断
```

- `truncate_filename()`：文件名中间截断，保留扩展名（`long_na...me.bin`）
- `mods_string()`（app.rs）：生成模式标志字符串（如 `ift` = immediate+full-lock+track）

### panic hook

注册 `std::panic::set_hook`，panic 时清理 `--copy` 模式创建的临时文件。

---

## main.rs 导航

### 初始化（30-125）
- `main()`：解析参数，打开文件，加载颜色配置，初始化终端

### 事件循环（127-380）
- `run()`：主循环 = 渲染 + 事件处理
- 鼠标事件：点击定位光标、拖拽选区、滚轮翻页、底栏点击
- 键盘事件：Ctrl 快捷键 → 模式分发
- Release 事件过滤 + Windows 40ms 节流（防双击）

### 输入处理（385-760）
- `handle_save()`：保存确认弹窗
- `handle_input()`：文本输入（搜索/跳转字节地址/跳转页码）
- `handle_edit()`：编辑模式（光标移动 + 字节编辑）
- `handle_normal()`：Normal 模式快捷键（跨页滚动）

### 快捷键体系

**Ctrl 前缀键**（全局，在模式分发之前处理）：
- `Ctrl+Z` / `Ctrl+Y`：撤销 / 重做
- `Ctrl+Q`：退出（有修改弹确认）
- `Ctrl+G`：跳转到偏移地址
- `Ctrl+H`：帮助弹窗
- `Ctrl+S`：保存
- `Ctrl+C`：复制选区
- `Ctrl+F`：搜索
- `Ctrl+P`：打开文件浏览器

**Ctrl+K 前缀键**（二次按键，`pending_ctrl_k` 状态）：
- `Ctrl+K, R`：还原光标字节到原始值（`restore_at`）
- `Ctrl+K, M`：打开菜单（Help / Sample / About）

**Alt 键**：
- `Alt+J` / `Alt+K`：设置选区起点 / 终点
- `Alt+M`：打开菜单
- `Alt+↑` / `Alt+↓`：字节值 ±1

**模式分发**（`handle_key_event` 中 `match app.input_mode`）：
- `Normal` → `handle_normal()`：导航/搜索/模式切换
- `Edit` → `handle_edit()`：光标移动 + 字节编辑
- `SearchInput` / `GotoInput` / `GotoByteInput` → `handle_input()`：文本输入
- `SaveConfirm` → `handle_save()`：y/n 确认
- `Help` / `Menu` / `About` / `ModeSelect`：各自 ESC 关闭
- `ModeMenu`：模式菜单选择（↑↓ 导航，Enter 切换，ESC 关闭）
- `FileBrowser`：文件浏览器模式（Enter 打开文件/目录，ESC 关闭）

**Menu 模式快捷键**：
- `h`：打开帮助（Help）
- `s`：打开 Sample（*sample 入口，加载演示数据）
- `a`：打开关于（About）

### 渲染（1100-1600）
- `sp(n)`：编号 → 样式映射
- `resolve()`：样式优先级链（cursor > found > search > selection > edit-dim > base）
- `resolve_auto_fg()`：fg: auto 哨兵 → 实际 black/white
- `build_lines()`：构建渲染行（跨页渲染，每行独立计算 pack）
- `draw_hex()` / `draw_status()` / `draw_help()` / `draw_save_dialog()` / `draw_mode_dropdown()`
- `draw_mode_menu_dropdown()`：模式菜单下拉渲染

### 底栏点击区域
```
[ASCII]  & 1a3f  pack 2/5  Ctrl+H:help  [MENU]
  ↑         ↑        ↑           ↑          ↑
  模式菜单  跳转字节  跳转页      帮助       菜单
```

搜索时：
```
Search: "4f2a" [3/5678+] @3/ff  ↑↓:next ESC:clear
```

### 核心函数（v0.2.0 新增）

- `save_with_overlay(mmap, overlay, path)`：mmap + overlay 分段合并写入文件
- `poll_track_event(path, last_modified)`：平台特定文件变化检测（inotify/poll）
- `flush_byte(fd, offset, byte)`：pwrite 单字节即时写盘
- `flush_last(fd, offset, byte)`：pwrite 最后编辑字节
- `truncate_filename(name, max_width)`：文件名中间截断保留扩展名
- `draw_mode_menu_dropdown(frame, app, rect)`：模式菜单 UI 渲染
- `mode_menu_rect(app, area)`：模式菜单位置计算

---

## app.rs 导航

### 枚举
- `DisplayMode`：Ascii / Hex / Utf8（含 `next()` / `prev()`）
- `InputMode`：Normal / Edit / SearchInput / GotoInput / GotoByteInput / StringSearchInput / SaveConfirm / Help / ModeSelect / **ModeMenu** / Menu / About / FileBrowser

### App 核心字段
- 分页：`file_size`, `pack_size`(4096), `total_packs`, `current_pack`, `scroll_top`
- 光标：`cursor_byte`, `cursor_nibble`（hex 模式的半字节）
- 搜索：`search`(BitSearch), `search_active`, `search_len`, `current_match`
- 编辑：`undo_stack`, `redo_stack`, `dirty`, `modified`(ModifiedMap), `original_values`(HashMap)
- **overlay**：`HashMap<usize, u8>`，mmap+overlay 架构的脏字节表
- **last_modified**：`Option<usize>`，最后一次修改偏移（immediate mode flush 用）
- 选区：`sel_start`, `sel_end`, `dragging`
- 显示：`is_color256`, `is_rgb_bg`, `is_hsl_bg`, `is_gray_bg`, `is_heat_bg`, `is_hslbit_bg`, `is_rgbbit_bg`
- 菜单：`pending_ctrl_k`, `pending_file`, `menu_selected`, **`mode_menu_selected`**
- **模式标志**：`flag_copy`, `flag_track`, `flag_inotify`, `flag_immediate`, `flag_lock`（运行时活跃模式）

### 跨页滚动方法
- `global_total_rows()`：文件总行数
- `global_scroll_top()`：当前视口全局起始行号
- `global_to_local(grow)`：全局行号 → (页号, 页内行号)
- `set_global_scroll(g)`：设置全局滚动位置（自动计算 pack + scroll_top）
- `ensure_cursor_visible()`：跨页光标跟随

### 搜索导航方法
- `jump_to_match()`：跳转到指定字节位置的匹配
- `next_global()` / `prev_global()`：逐个匹配导航
- `jump_to_page_match()` / `jump_to_page_match_prev()`：跳到目标页第一个匹配
- `current_match_number()`：实时计算当前是第几个匹配

### 编辑方法
- `modify()` / `undo()` / `redo()`：字节编辑 + 撤销（modify 自动标记到 ModifiedMap + 存原始值）
- `restore_at()`：还原光标字节到原始值（Ctrl+K,R），不影响 undo/redo
- `edit_hex()` / `edit_char()`：hex/字符编辑

### v0.2.0 新增方法
- `byte_at(offset)`：overlay-first 字节读取（先查 HashMap overlay，无则读 mmap）
- `mods_string()`：生成模式标志字符串（如 `ift` = immediate+full-lock+track）

---

## color_config.rs 导航

### 核心机制
- `AUTO_FG_SENTINEL`：`Rgb(1,1,1)` 哨兵，标记 `fg: auto`
- `luminance()`：BT.601 亮度公式（>128 亮 → 黑前景，否则白前景）
- `color_rgb()`：命名色 → RGB（优先终端调色板）

### 解析流程
```
YAML 文本 → yaml_parse_style() → parse_style_val()
  ↓ fg: auto + 有 bg → 存哨兵
  ↓ fg: auto + 无 bg → fg null（终端默认）
```

### 样式编号映射
```
1=null, 2=head2, 3=tail, 4=control, 5=ascii, 6=head3
8=head4, 10=hex, 12/13=found, 15/17=selection, 16=cursor
```

## search.rs 导航

### 搜索模式
- `Needle::Lit`：精确字节序列
- `Needle::Pat`：nibble 模式（`NibAtom::Exact/Range/Any`）

### 三级位图索引
```
L0: 每 pack 1 bit   → 快速跳过空 pack
L1: 每 1MB 1 bit    → 快速跳过空 MB
L2: 每 1GB 1 bit    → 快速跳过空 GB
```

⚠️ **Dead Code**：search.rs 中的旧 `Search` 结构体已废弃，被 bitmap.rs 的 `BitSearch` 替代。`Needle` 枚举仍在使用。

### 搜索语法
```
4f2a        → 精确 hex
4x          → 高 nibble=4，低 nibble 任意
[0-3]f      → 高 nibble 在 0-3，低 nibble=f
z           → 任意字节（= 两个 Any nibble）
```

## bitmap.rs 导航

### 四级位图
```
L3: 128B — 全文件 1024 个 1GB 块的存在性
L2: 128B — 当前 1GB 内 1024 个 1MB 块的存在性
L1:  32B — 当前 1MB 内 256 个 4K 页的存在性
L0: 512B — 当前 4K 页内 4096 字节的存在性
```

### 核心方法
- `scan_chunk()`：按需扫描 1MB，标记匹配到位图
- `next_match_after()` / `prev_match_before()`：位图下降查找下一个/上一个匹配
- `matches_at()`：检查指定位置是否匹配
- `pack_matches()`：获取指定 pack 内的匹配范围（供渲染高亮）（⚠️ dead code，未使用）
- `ensure_cached()` / `ensure_mb_cached()`：切换区域时重建 L0/L1 缓存

### 位操作
- `set_bit()` / `next_set()` / `prev_set()`：位图设置和扫描
- `encode()` / `decode()`：偏移量 ↔ 四级索引编解码

## modified.rs 导航

### Sparse Hierarchical Bitmap（作者 Saisui 发明）

稀疏层级位图，用于追踪文件中被编辑过的字节。
灵感来源于四级位图搜索引擎（BitSearch），但采用按需分配策略。

**设计思想**：将文件偏移空间按 4K → 1MB → 1GB → 1TB 分层，
每层用位图标记"该区域是否有编辑"。
查询时从顶层向下逐级缩小范围，任意一层"无"则跳过整棵子树。
只为实际有编辑的区域分配下级位图，未编辑区域零开销。

### 层级结构
```
L3: [u8; 128]                    128B 固定（1024 个 1GB 块）
L2: HashMap<usize, [u8; 128]>    每个有编辑的 1GB 块分配 128B
L1: HashMap<usize, [u8; 32]>     每个有编辑的 1MB 块分配 32B
L0: HashMap<usize, [u8; 512]>    每个有编辑的 4K 页分配 512B
```

### 核心方法
- `mark(offset)`：标记字节为"已编辑"，按需分配 L0/L1/L2
- `unmark(offset)`：清除标记，级联释放空的父节点
- `is_modified(offset)`：查询是否编辑过，O(1) 最多 4 次位操作

### 性能
- 固定开销 ~328B（L3 + 3 个 HashMap 头）
- 10000 个编辑 ≈ 252KB 内存
- 渲染时每字节 4 次 HashMap 查找，480 字节/帧 ≈ 24μs

## utf8.rs 导航

- `ByteClass`：字节类型（Ascii/Tail/Duo/Trio/Quo/Invalid）
- `decode_row()`：行级 UTF-8 解码，处理跨行序列
- `display_width()`：CJK 等宽字符 width=2

## FileBrowser（main.rs 内）

### 关键函数
- `run_file_browser_only()`：独立文件浏览器入口（`Ctrl+P` 触发），覆盖整个终端
- `draw_file_browser()`：渲染文件浏览器 UI（目录列表 + 路径栏 + 快捷键提示）
- `*sample`：Menu 中 `s` 快捷键触发，加载演示/示例数据

### 工作模式
文件浏览器有独立的事件循环，不依赖 App 状态。打开文件后返回到主界面。

## 编辑规范

### 代码规范

#### 注释要求
- **所有 pub 函数必须有 `///` 文档注释**，说明用途、参数、返回值
- **所有 pub struct 字段必须有注释**，说明含义
- **复杂逻辑必须有行内注释**，解释"为什么"而非"做什么"
- **模块顶部必须有模块级注释**，说明模块职责

```rust
/// 计算文件总行数
///
/// 每行 16 字节，向上取整。
/// 用于跨页滚动的全局行号范围计算。
pub fn global_total_rows(&self) -> usize {
    (self.file_size + 15) / 16
}
```

#### 命名规范
- **函数名**：动词开头，如 `draw_hex()`、`handle_input()`、`ensure_visible()`
- **结构体名**：名词，如 `App`、`FileBrowser`、`DirEntry`
- **字段名**：名词或形容词，如 `cursor_byte`、`is_color256`、`sel_start`
- **常量名**：全大写下划线，如 `AUTO_FG_SENTINEL`、`FIND_CHUNK`
- **枚举变体**：PascalCase，如 `DisplayMode::Ascii`、`InputMode::Normal`
- **布尔字段**：`is_`/`has_`/`should_` 前缀，如 `is_color256`、`search_active`、`dirty`
- **flag 字段**：`flag_` 前缀，如 `flag_copy`、`flag_track`、`flag_immediate`
- **避免缩写**：用 `scroll_top` 而非 `st`，用 `current_pack` 而非 `cp`

#### 代码风格
- **函数长度**：单个函数不超过 100 行，超过则拆分
- **match 分支**：每个分支不超过 20 行，复杂逻辑提取为辅助函数
- **错误处理**：`?` 传播或 `map_err` 转换，不吞错误
- **dead code**：未使用的字段/函数加 `_` 前缀或删除，不留废代码

### 重命名函数
```bash
# 正确：用 \b word boundary
sed -i 's/\bsp(/style_for(/g' src/main.rs

# 错误：replaceAll 会误匹配 byte_disp → byte_distyle_for
```

### 添加新样式
1. `color_config.rs`：在 `ColorConfig` 加字段，`parse()` 中解析
2. `main.rs`：在 `sp()` 加编号映射
3. `color.yaml`：加对应条目

### 添加新字节类型
1. `byte_type_group()`：加分类逻辑
2. `byte_style()`：加样式映射
3. `byte_disp()`：加显示文本

### 编译验证
```bash
cargo build --release    # 编译
cargo clippy             # lint
```
