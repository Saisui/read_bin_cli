# read-bin 代码编辑导航

## 项目概览

终端 TUI 十六进制查看/编辑器，Rust 实现。支持跨页无缝滚动、鼠标拖拽选区、256 色渐变。

**依赖**：ratatui（TUI 框架）、crossterm（终端控制）、memmap2（内存映射文件）、arboard（剪贴板）

**构建**：`cargo build --release`，产物在 `target/release/read-bin`

## 文件结构

```
src/
├── main.rs          # 入口 + TUI 事件循环 + 渲染 + 输入处理（~2500 行）
├── app.rs           # 应用状态管理（跨页滚动、光标、搜索、undo/redo）
├── bitmap.rs        # 四级位图搜索引擎（L0~L3，804 字节固定内存）
├── color_config.rs  # YAML 颜色配置加载 + fg: auto 逻辑
├── search.rs        # 搜索模式解析（hex/nibble/字符串 → Needle）
└── utf8.rs          # UTF-8 字节分类与解码
```

## 模块依赖关系

```
main.rs
 ├── app.rs          （App 状态、跨页滚动、搜索导航）
 ├── bitmap.rs       （BitSearch 四级位图搜索引擎）
 ├── color_config.rs （ColorConfig 颜色配置）
 ├── search.rs       （Needle 搜索模式解析）
 └── utf8.rs         （ByteClass 字节分类）
```

## 关键数据流

```
用户输入 → handle_*() → App 状态更新 → resolve() → style_for() → 渲染
```

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

### 渲染（1100-1600）
- `sp(n)`：编号 → 样式映射
- `resolve()`：样式优先级链（cursor > found > search > selection > edit-dim > base）
- `resolve_auto_fg()`：fg: auto 哨兵 → 实际 black/white
- `build_lines()`：构建渲染行（跨页渲染，每行独立计算 pack）
- `draw_hex()` / `draw_status()` / `draw_help()` / `draw_save_dialog()` / `draw_mode_dropdown()`

### 底栏点击区域
```
[ASCII]  & 1a3f  pack 2/5  Ctrl+H:help
  ↑         ↑        ↑           ↑
  模式菜单  跳转字节  跳转页      帮助
```

搜索时：
```
Search: "4f2a" [3/5678+] @3/ff  ↑↓:next ESC:clear
```

## app.rs 导航

### 枚举
- `DisplayMode`：Ascii / Hex / Utf8（含 `next()` / `prev()`）
- `InputMode`：Normal / Edit / SearchInput / GotoInput / GotoByteInput / StringSearchInput / SaveConfirm / Help / ModeSelect

### App 核心字段
- 分页：`file_size`, `pack_size`(4096), `total_packs`, `current_pack`, `scroll_top`
- 光标：`cursor_byte`, `cursor_nibble`（hex 模式的半字节）
- 搜索：`search`(BitSearch), `search_active`, `search_len`, `current_match`
- 编辑：`undo_stack`, `redo_stack`, `dirty`
- 选区：`sel_start`, `sel_end`, `dragging`
- 显示：`is_color256`, `is_rgb_bg`, `is_hsl_bg`, `is_gray_bg`, `is_heat_bg`, `is_hslbit_bg`, `is_rgbbit_bg`

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
- `modify()` / `undo()` / `redo()`：字节编辑 + 撤销
- `edit_hex()` / `edit_char()`：hex/字符编辑

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
- `pack_matches()`：获取指定 pack 内的匹配范围（供渲染高亮）
- `ensure_cached()` / `ensure_mb_cached()`：切换区域时重建 L0/L1 缓存

### 位操作
- `set_bit()` / `next_set()` / `prev_set()`：位图设置和扫描
- `encode()` / `decode()`：偏移量 ↔ 四级索引编解码

## utf8.rs 导航

- `ByteClass`：字节类型（Ascii/Tail/Duo/Trio/Quo/Invalid）
- `decode_row()`：行级 UTF-8 解码，处理跨行序列
- `display_width()`：CJK 等宽字符 width=2

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
