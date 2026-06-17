/// read-bin: 终端十六进制查看/编辑器
///
/// 功能：文件分页浏览、三种显示模式（ASCII/HEX/UTF8）、
/// hex/nibble/字符串搜索、字节编辑、撤销/重做、鼠标支持。
///
/// 模块结构：
/// - app: 应用状态管理
/// - bitmap: 四级位图搜索引擎（804 字节固定内存）
/// - color_config: 颜色配置加载
/// - search: 搜索模式解析（hex/nibble/字符串）
/// - utf8: UTF-8 解码与分类
mod app;
mod bitmap;
mod color_config;
mod modified;
mod search;
mod utf8;

use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use memmap2::Mmap;
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Terminal,
};

use app::{App, DisplayMode, InputMode};

/// 程序入口
///
/// 支持 `--dump` 模式（纯文本 hex dump）和 TUI 交互模式。
/// 初始化终端、加载颜色配置、启动事件循环。
/// 无参数时进入文件浏览器，Ctrl+P 可随时重新打开文件浏览器。
/// 终端只创建一次，文件浏览器和查看器共享。
fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dump = args.get(2).map(|s| s.as_str()) == Some("--dump");

    let mut filename = if args.len() < 2 {
        String::new()
    } else {
        args[1].clone()
    };

    // 终端只创建一次
    enable_raw_mode().map_err(|e| io::Error::new(e.kind(), format!("enable_raw_mode: {}", e)))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("EnterAlternateScreen/EnableMouseCapture: {}", e),
        )
    })?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)
        .map_err(|e| io::Error::new(e.kind(), format!("Terminal::new: {}", e)))?;

    let exit_code = (|| -> io::Result<()> {
        loop {
            // 如果没有文件，进入文件浏览器
            if filename.is_empty() {
                match run_file_browser_only(&mut terminal)? {
                    Some(path) => filename = path,
                    None => return Ok(()),
                }
            }

            let file = if dump {
                File::open(&filename)?
            } else {
                OpenOptions::new().read(true).open(&filename)?
            };
            let file_size = file.metadata()?.len() as usize;
            if file_size == 0 {
                eprintln!("Empty file");
                return Ok(());
            }

            if dump {
                disable_raw_mode()?;
                let mmap = unsafe { Mmap::map(&file)? };
                let data: &[u8] = &mmap;
                let name = std::path::Path::new(&filename)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| filename.clone());
                println!("{}  ({})", name, App::format_size(file_size));
                println!("    0 1 2 3 4 5 6 7 8 9 a b c d e f ");
                for r in 0..(data.len() + 15) / 16 {
                    print!("{:04x}  ", r);
                    let off = r * 16;
                    for i in 0..16 {
                        if off + i < data.len() {
                            print!("{:02x} ", data[off + i]);
                        } else {
                            print!("   ");
                        }
                    }
                    print!(" |");
                    for i in 0..16 {
                        if off + i < data.len() {
                            let b = data[off + i];
                            print!(
                                "{}",
                                if (0x20..=0x7e).contains(&b) {
                                    b as char
                                } else {
                                    '.'
                                }
                            );
                        }
                    }
                    println!("|");
                }
                return Ok(());
            }

            let mmap = unsafe { Mmap::map(&file)? };
            let mut data = mmap[..file_size].to_vec();

            // 首次加载颜色配置，后续调用忽略（OnceLock 已设置）
            let _ = init_colors(std::path::Path::new("color.yaml"));
            color_config::init_terminal_palette(&std::path::Path::new(
                &std::env::var("HOME").unwrap_or_default(),
            ));

            enable_raw_mode()
                .map_err(|e| io::Error::new(e.kind(), format!("enable_raw_mode: {}", e)))?;
            terminal.clear()?;

            let base_name = std::path::Path::new(&filename)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| filename.clone());
            let mut app = App::new(file_size, base_name);
            let reopen = run(&mut terminal, &mut app, &mut data, &filename);

            match reopen {
                Ok(true) => {
                    // pending_file（包括 Sample 临时文件）或文件浏览器
                    if let Some(ref path) = app.pending_file {
                        filename = path.clone();
                        app.pending_file = None;
                    } else {
                        filename.clear();
                    }
                }
                Ok(false) => {
                    disable_raw_mode()?;
                    return Ok(());
                }
                Err(e) => {
                    disable_raw_mode()?;
                    eprintln!("Error: {}", e);
                    return Ok(());
                }
            }
        }
    })();

    // 统一清理终端
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();

    exit_code
}

/// 渲染一帧画面
///
/// 根据当前 input_mode 调度不同的渲染组合。
fn render_frame(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &App,
    data: &[u8],
) -> io::Result<()> {
    terminal.draw(|f| {
        let area = f.area();
        match app.input_mode {
            InputMode::Help => {
                draw_hex(f, app, data, area);
                draw_help(f, app, area);
            }
            InputMode::SaveConfirm => {
                draw_hex(f, app, data, area);
                draw_save_dialog(f, app, area);
            }
            InputMode::ModeSelect => {
                draw_hex(f, app, data, area);
                draw_status(f, app, data, area);
                draw_mode_dropdown(f, app, area);
            }
            InputMode::Menu => {
                draw_hex(f, app, data, area);
                draw_status(f, app, data, area);
                draw_menu_dropdown(f, app, area);
            }
            InputMode::About => {
                draw_hex(f, app, data, area);
                draw_status(f, app, data, area);
                draw_about(f, area);
            }
            _ => {
                draw_hex(f, app, data, area);
                draw_status(f, app, data, area);
            }
        }
    })?;
    Ok(())
}

/// 处理鼠标事件
///
/// 包括：点击定位光标/选区、拖拽选区、滚轮翻页、帮助弹窗滚动条拖拽、
/// 状态栏点击（模式菜单/跳转/帮助）、顶栏点击（打开文件浏览器）。
fn handle_mouse_event(
    app: &mut App,
    mouse: crossterm::event::MouseEvent,
    size: ratatui::layout::Size,
    max_rows: usize,
    reopen_browser: &mut bool,
    should_break: &mut bool,
) {
    let mx = mouse.column;
    let my = mouse.row;
    let area_h = size.height;
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if app.input_mode == InputMode::Help {
                // 点击帮助弹窗外 → 关闭
                let (hx, hy, hw, hh) = app.help_rect.unwrap_or((0, 0, 0, 0));
                if mx < hx || mx >= hx + hw || my < hy || my >= hy + hh {
                    app.input_mode = InputMode::Normal;
                    app.help_rect = None;
                    app.help_dragging = false;
                } else {
                    // 点击滚动条 → 开始拖拽
                    let sb_x = hx + hw - 2;
                    if mx == sb_x && hh > 2 {
                        app.help_dragging = true;
                        let inner_h = hh as usize - 2;
                        let total = HELP_LINES.lines().count() + 2;
                        let max_scroll = total.saturating_sub(inner_h);
                        if max_scroll > 0 && inner_h > 0 {
                            let click_ratio = (my - hy - 1) as f64 / (inner_h - 1) as f64;
                            app.help_scroll = (click_ratio * max_scroll as f64).round() as usize;
                        }
                    }
                }
            } else if app.input_mode == InputMode::ModeSelect {
                // 模式下拉菜单点击（radio behavior for color modes）
                let dh = 12u16;
                let dy = area_h.saturating_sub(1) - dh;
                let dw = 10u16;
                if mx < dw && my >= dy && my < dy + dh {
                    let sel = my - dy;
                    match sel {
                        0 => app.mode = DisplayMode::Ascii,
                        1 => app.mode = DisplayMode::Hex,
                        2 => app.mode = DisplayMode::Utf8,
                        // Row 3 = separator (Color:), ignore
                        4 => {
                            // None: clear all color modes
                            app.is_color256 = false;
                            app.is_rgb_bg = false;
                            app.is_hsl_bg = false;
                            app.is_gray_bg = false;
                            app.is_heat_bg = false;
                            app.is_hslbit_bg = false;
                            app.is_rgbbit_bg = false;
                        }
                        5 => {
                            // 256: radio behavior
                            app.is_color256 = !app.is_color256;
                            if app.is_color256 {
                                app.is_rgb_bg = false;
                                app.is_hsl_bg = false;
                                app.is_gray_bg = false;
                                app.is_heat_bg = false;
                                app.is_hslbit_bg = false;
                                app.is_rgbbit_bg = false;
                            }
                        }
                        6 => {
                            // RGB: radio behavior
                            app.is_rgb_bg = !app.is_rgb_bg;
                            if app.is_rgb_bg {
                                app.is_color256 = false;
                                app.is_hsl_bg = false;
                                app.is_gray_bg = false;
                                app.is_heat_bg = false;
                                app.is_hslbit_bg = false;
                                app.is_rgbbit_bg = false;
                            }
                        }
                        7 => {
                            // HSL: radio behavior
                            app.is_hsl_bg = !app.is_hsl_bg;
                            if app.is_hsl_bg {
                                app.is_color256 = false;
                                app.is_rgb_bg = false;
                                app.is_gray_bg = false;
                                app.is_heat_bg = false;
                                app.is_hslbit_bg = false;
                                app.is_rgbbit_bg = false;
                            }
                        }
                        8 => {
                            // GRAY: radio behavior
                            app.is_gray_bg = !app.is_gray_bg;
                            if app.is_gray_bg {
                                app.is_color256 = false;
                                app.is_rgb_bg = false;
                                app.is_hsl_bg = false;
                                app.is_heat_bg = false;
                                app.is_hslbit_bg = false;
                                app.is_rgbbit_bg = false;
                            }
                        }
                        9 => {
                            // HEAT: radio behavior
                            app.is_heat_bg = !app.is_heat_bg;
                            if app.is_heat_bg {
                                app.is_color256 = false;
                                app.is_rgb_bg = false;
                                app.is_hsl_bg = false;
                                app.is_gray_bg = false;
                                app.is_hslbit_bg = false;
                                app.is_rgbbit_bg = false;
                            }
                        }
                        10 => {
                            // hsl: radio behavior
                            app.is_hslbit_bg = !app.is_hslbit_bg;
                            if app.is_hslbit_bg {
                                app.is_color256 = false;
                                app.is_rgb_bg = false;
                                app.is_hsl_bg = false;
                                app.is_gray_bg = false;
                                app.is_heat_bg = false;
                                app.is_rgbbit_bg = false;
                            }
                        }
                        11 => {
                            // rgb: radio behavior
                            app.is_rgbbit_bg = !app.is_rgbbit_bg;
                            if app.is_rgbbit_bg {
                                app.is_color256 = false;
                                app.is_rgb_bg = false;
                                app.is_hsl_bg = false;
                                app.is_gray_bg = false;
                                app.is_heat_bg = false;
                                app.is_hslbit_bg = false;
                            }
                        }
                        _ => {}
                    }
                    if sel <= 2 {
                        app.input_mode = InputMode::Normal;
                    }
                } else {
                    app.input_mode = InputMode::Normal;
                }
            } else if app.input_mode == InputMode::About {
                app.input_mode = InputMode::Normal;
            } else if app.input_mode == InputMode::Menu {
                let dw = 14u16;
                let items_count = 3u16;
                let dx = size.width.saturating_sub(dw);
                let dy = area_h.saturating_sub(1).saturating_sub(items_count);
                if mx >= dx && mx < dx + dw && my >= dy && my < dy + items_count {
                    let sel = my - dy;
                    match sel {
                        0 => {
                            app.input_mode = InputMode::Help;
                            app.help_scroll = 0;
                        }
                        1 => {
                            // Sample: 写临时文件，走正常文件打开流程
                            let sample_path = std::env::temp_dir().join("read-bin-sample.bin");
                            let sample_data: Vec<u8> = (0u8..=255).collect();
                            if std::fs::write(&sample_path, &sample_data).is_ok() {
                                app.pending_file = Some(sample_path.to_string_lossy().to_string());
                                app.input_mode = InputMode::Normal;
                                *should_break = true;
                            }
                        }
                        2 => {
                            app.input_mode = InputMode::About;
                        }
                        _ => {}
                    }
                } else {
                    app.input_mode = InputMode::Normal;
                }
            } else if my == 0 && app.input_mode == InputMode::Normal {
                // 点击顶栏 → 打开文件浏览器
                *reopen_browser = true;
                *should_break = true;
            } else if my >= 2 && mx >= 4 && my < area_h.saturating_sub(1) {
                // 点击数据区 → 定位光标 + 开始选区
                let global_row = my as usize - 2 + app.global_scroll_top();
                let col = mx as usize - 4;
                let bc = col / 2;
                if bc < 16 {
                    let off = global_row * 16 + bc;
                    if off < app.file_size {
                        app.cursor_byte = off;
                        app.cursor_focused = true;
                        // 点击只移动光标，拖拽才开始选区（避免闪烁）
                        app.dragging = true;
                        app.sel_start = None;
                        app.sel_end = None;
                        app.ensure_cursor_visible(area_h);
                    }
                }
            } else if app.input_mode != InputMode::Edit {
                app.cursor_focused = false;
                app.sel_start = None;
                app.sel_end = None;
            }
            // 状态栏点击
            if my == area_h.saturating_sub(1) && app.input_mode == InputMode::Normal {
                let dirty_len = if app.dirty { 11 } else { 0 };
                let hex_w = if app.file_size <= 0xff {
                    2
                } else if app.file_size <= 0xffff {
                    4
                } else if app.file_size <= 0xffffff {
                    6
                } else {
                    8
                };
                // 用实际模式标签长度替代硬编码的 6
                let mode_len = app.mode.label().len() as u16;
                let at_offset = mode_len + dirty_len as u16 + 2;
                let at_len = 1 + hex_w as u16;
                let pack_offset = at_offset + at_len + 2;
                let pack_str = format!(
                    "pack {:x}/{:x}",
                    (app.cursor_byte / app.pack_size) + 1,
                    app.total_packs
                );
                let pack_len = pack_str.len() as u16;
                let help_offset = pack_offset + pack_len + 2;
                if mx < 7 {
                    app.input_mode = InputMode::ModeSelect;
                } else if mx >= at_offset && mx < at_offset + at_len {
                    app.input_mode = InputMode::GotoByteInput;
                    app.input_buf.clear();
                    app.input_prompt = "Go to byte (hex):".into();
                } else if mx >= pack_offset && mx < pack_offset + pack_len {
                    app.input_mode = InputMode::GotoInput;
                    app.input_buf.clear();
                    app.input_prompt = "Go to pack (hex):".into();
                } else if mx >= help_offset {
                    app.input_mode = InputMode::Menu;
                    app.menu_selected = 0;
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if app.help_dragging && app.input_mode == InputMode::Help {
                // 拖拽帮助弹窗滚动条
                if let Some((hx, hy, hw, hh)) = app.help_rect {
                    let sb_x = hx + hw - 2;
                    let inner_h = hh as usize - 2;
                    let total = 2 + HELP_LINES.lines().count();
                    let max_scroll = total.saturating_sub(inner_h);
                    if max_scroll > 0 && inner_h > 0 && mx == sb_x {
                        let drag_ratio =
                            (my as isize - hy as isize - 1).max(0) as f64 / (inner_h - 1) as f64;
                        app.help_scroll = (drag_ratio * max_scroll as f64).round() as usize;
                    }
                }
            } else if app.dragging {
                // 拖拽选区
                if my >= 2 && mx >= 4 && my < area_h.saturating_sub(1) {
                    let global_row = my as usize - 2 + app.global_scroll_top();
                    let col = mx as usize - 4;
                    let bc = col / 2;
                    if bc < 16 {
                        let off = global_row * 16 + bc;
                        if off < app.file_size {
                            app.cursor_byte = off;
                            // 第一次拖拽时设置 sel_start（点击时未设）
                            if app.sel_start.is_none() {
                                app.sel_start = Some(off);
                            }
                            app.sel_end = Some(off);
                            app.ensure_cursor_visible(area_h);
                        }
                    }
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            app.help_dragging = false;
            if app.dragging {
                app.dragging = false;
                if app.sel_start == app.sel_end {
                    app.sel_start = None;
                    app.sel_end = None;
                }
            }
        }
        MouseEventKind::ScrollUp => {
            if app.input_mode == InputMode::Help {
                app.help_scroll = app.help_scroll.saturating_sub(1);
            } else {
                let gs = app.global_scroll_top();
                if gs >= 3 {
                    app.set_global_scroll(gs - 3);
                } else {
                    app.set_global_scroll(0);
                }
            }
        }
        MouseEventKind::ScrollDown => {
            if app.input_mode == InputMode::Help {
                app.help_scroll += 1;
            } else {
                let gs = app.global_scroll_top();
                let max = app.global_total_rows().saturating_sub(max_rows);
                app.set_global_scroll((gs + 3).min(max));
            }
        }
        _ => {}
    }
}

/// 处理键盘事件
///
/// 处理顺序：
/// 1. Release 事件过滤（Windows 兼容）
/// 2. Ctrl+K 前缀键（二次按键序列：R=还原字节，M=菜单）
/// 3. Ctrl 全局快捷键（Z/Y/Q/G/H/S/C/F/K/P）
/// 4. Alt 快捷键（J/K=选区，M=模式，↑↓=字节值微调）
/// 5. 模式分发（Help/ModeSelect/SaveConfirm/Search/Edit/Normal）
fn handle_key_event(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    data: &mut [u8],
    filename: &str,
    th: u16,
    max_rows: usize,
    reopen_browser: &mut bool,
    should_break: &mut bool,
) {
    if key.kind == event::KeyEventKind::Release {
        return;
    }

    // ─── Ctrl+K 前缀键（二次按键序列）──────────────────
    // 第一次按 Ctrl+K 设置 pending_ctrl_k = true，等待下一个键。
    // 支持：
    //   R → 还原光标字节到编辑前的原始值（restore_at）
    //   M → 打开菜单（Help / Sample / About）
    if app.pending_ctrl_k {
        app.pending_ctrl_k = false;
        match key.code {
            KeyCode::Char('r') | KeyCode::Char('R') => {
                app.restore_at(data, app.cursor_byte);
            }
            KeyCode::Char('m') | KeyCode::Char('M') => {
                app.input_mode = InputMode::Menu;
                app.menu_selected = 0;
            }
            _ => {}
        }
        return;
    }

    // ─── Ctrl 全局快捷键（所有模式下生效）──────────────
    // 在模式分发之前处理，任何模式下按 Ctrl+X 都会触发。
    // 每个分支处理后 return，不进入模式分发。
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('z') => {
                // 撤销
                app.undo(data);
                return;
            }
            KeyCode::Char('y') => {
                // 重做
                app.redo(data);
                return;
            }
            KeyCode::Char('q') => {
                // 退出（有修改弹确认）
                if app.dirty {
                    app.input_mode = InputMode::SaveConfirm;
                    app.save_selected = true;
                } else {
                    *should_break = true;
                }
                return;
            }
            KeyCode::Char('g') => {
                app.input_mode = InputMode::GotoInput;
                app.input_buf.clear();
                app.input_prompt = "Go to (hex):".into();
                return;
            }
            KeyCode::Char('h') => {
                app.input_mode = InputMode::Help;
                app.help_scroll = 0;
                return;
            }
            KeyCode::Char('s') => {
                let _ = std::fs::write(filename, &*data);
                app.dirty = false;
                return;
            }
            KeyCode::Char('k') => {
                app.pending_ctrl_k = true;
                return;
            }
            KeyCode::Char('c') => {
                if let (Some(start), Some(end)) = (app.sel_start, app.sel_end) {
                    let (lo, hi) = if start <= end {
                        (start, end)
                    } else {
                        (end, start)
                    };
                    let len = (hi - lo + 1).min(data.len() - lo);
                    let selected = &data[lo..lo + len];
                    let text = match app.mode {
                        DisplayMode::Ascii => selected
                            .iter()
                            .map(|b| {
                                if *b == 0 {
                                    '.'
                                } else if *b == 0x0d {
                                    'r'
                                } else if *b == 10 {
                                    '\n'
                                } else if *b == 0x1b {
                                    'e'
                                } else if (0x01..=0x1f).contains(b) {
                                    '.'
                                } else if *b == 0x20 {
                                    ' '
                                } else if (0x21..=0x7e).contains(b) {
                                    *b as char
                                } else {
                                    '.'
                                }
                            })
                            .collect(),
                        DisplayMode::Hex => selected
                            .iter()
                            .map(|b| format!("{:02x}", b))
                            .collect::<Vec<_>>()
                            .join(" "),
                        DisplayMode::Utf8 => {
                            let segs = crate::utf8::decode_row(data, lo, len, 0);
                            segs.iter()
                                .filter_map(|seg| {
                                    if let crate::utf8::Segment::Char { ch, .. } = seg {
                                        Some(*ch)
                                    } else {
                                        None
                                    }
                                })
                                .collect()
                        }
                    };
                    #[cfg(not(target_os = "android"))]
                    {
                        let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text));
                    }
                    #[cfg(target_os = "android")]
                    {
                        termux_clipboard_set(&text);
                    }
                }
                return;
            }
            KeyCode::Char('f') => {
                app.input_mode = InputMode::SearchInput;
                app.input_prompt = "search hex:".into();
                app.input_buf.clear();
                return;
            }
            KeyCode::Char('p') => {
                *reopen_browser = true;
                *should_break = true;
                return;
            }
            KeyCode::Left => {
                if app.input_mode == InputMode::Edit && app.current_pack > 0 {
                    app.current_pack -= 1;
                    app.scroll_top = 0;
                    app.cursor_byte = app.current_pack * app.pack_size;
                    app.ensure_cursor_visible(th);
                }
                return;
            }
            KeyCode::Right => {
                if app.input_mode == InputMode::Edit && app.current_pack + 1 < app.total_packs {
                    app.current_pack += 1;
                    app.scroll_top = 0;
                    app.cursor_byte = app.current_pack * app.pack_size;
                    app.ensure_cursor_visible(th);
                }
                return;
            }
            _ => {}
        }
    }

    // ─── Alt 快捷键 ──────────────────────────────────
    // Alt+J/K: 选区标记  Alt+M: 模式切换  Alt+↑/↓: 字节值微调
    if key.modifiers.contains(KeyModifiers::ALT) {
        match key.code {
            KeyCode::Char('j') => {
                // 选区起点
                app.sel_start = Some(app.cursor_byte);
                return;
            }
            KeyCode::Char('k') => {
                // 选区终点
                app.sel_end = Some(app.cursor_byte);
                return;
            }
            KeyCode::Char('m') => {
                // 切换显示模式
                app.mode = app.mode.next();
                return;
            }
            // Alt+↑/↓：字节值 ±1（编辑模式微调）
            KeyCode::Up => {
                if app.input_mode == InputMode::Edit && app.cursor_byte < app.file_size {
                    let val = data[app.cursor_byte];
                    if val < 0xFF {
                        app.modify(data, app.cursor_byte, val + 1);
                    }
                }
                return;
            }
            KeyCode::Down => {
                if app.input_mode == InputMode::Edit && app.cursor_byte < app.file_size {
                    let val = data[app.cursor_byte];
                    if val > 0x00 {
                        app.modify(data, app.cursor_byte, val - 1);
                    }
                }
                return;
            }
            _ => {}
        }
    }

    app.cursor_focused = true;

    // ─── 模式分发 ─────────────────────────────────────
    // 每个模式有自己的按键处理逻辑。
    // 处理顺序：Help → ModeSelect → SaveConfirm → SearchInput → Edit → Normal
    match app.input_mode {
        InputMode::Help => match key.code {
            // 帮助弹窗：滚动/关闭
            KeyCode::Esc | KeyCode::Char('q') => {
                app.input_mode = InputMode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.help_scroll = app.help_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.help_scroll += 1;
            }
            KeyCode::PageUp => {
                app.help_scroll = app.help_scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                app.help_scroll += 10;
            }
            _ => {}
        },
        InputMode::Menu => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                app.input_mode = InputMode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.menu_selected = app.menu_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if app.menu_selected < 2 {
                    app.menu_selected += 1;
                }
            }
            KeyCode::Enter => match app.menu_selected {
                0 => {
                    app.input_mode = InputMode::Help;
                    app.help_scroll = 0;
                }
                1 => {
                    let sample_path = std::env::temp_dir().join("read-bin-sample.bin");
                    let sample_data: Vec<u8> = (0u8..=255).collect();
                    if std::fs::write(&sample_path, &sample_data).is_ok() {
                        app.pending_file = Some(sample_path.to_string_lossy().to_string());
                        app.input_mode = InputMode::Normal;
                        *should_break = true;
                    }
                }
                2 => {
                    app.input_mode = InputMode::About;
                }
                _ => {}
            },
            KeyCode::Char('h') => {
                app.input_mode = InputMode::Help;
                app.help_scroll = 0;
            }
            KeyCode::Char('s') => {
                let sample_path = std::env::temp_dir().join("read-bin-sample.bin");
                let sample_data: Vec<u8> = (0u8..=255).collect();
                if std::fs::write(&sample_path, &sample_data).is_ok() {
                    app.pending_file = Some(sample_path.to_string_lossy().to_string());
                    app.input_mode = InputMode::Normal;
                    *should_break = true;
                }
            }
            KeyCode::Char('a') => {
                app.input_mode = InputMode::About;
            }
            _ => {}
        },
        InputMode::About => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                app.input_mode = InputMode::Normal;
            }
            _ => {}
        },
        InputMode::ModeSelect => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                app.input_mode = InputMode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.mode = app.mode.prev();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.mode = app.mode.next();
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                app.input_mode = InputMode::Normal;
            }
            KeyCode::Char('1') => {
                app.mode = DisplayMode::Ascii;
                app.input_mode = InputMode::Normal;
            }
            KeyCode::Char('2') => {
                app.mode = DisplayMode::Hex;
                app.input_mode = InputMode::Normal;
            }
            KeyCode::Char('3') => {
                app.mode = DisplayMode::Utf8;
                app.input_mode = InputMode::Normal;
            }
            KeyCode::Char('4') => {
                // 256: radio behavior
                app.is_color256 = !app.is_color256;
                if app.is_color256 {
                    app.is_rgb_bg = false;
                    app.is_hsl_bg = false;
                    app.is_gray_bg = false;
                    app.is_heat_bg = false;
                    app.is_hslbit_bg = false;
                    app.is_rgbbit_bg = false;
                }
            }
            KeyCode::Char('5') => {
                // RGB: radio behavior
                app.is_rgb_bg = !app.is_rgb_bg;
                if app.is_rgb_bg {
                    app.is_color256 = false;
                    app.is_hsl_bg = false;
                    app.is_gray_bg = false;
                    app.is_heat_bg = false;
                    app.is_hslbit_bg = false;
                    app.is_rgbbit_bg = false;
                }
            }
            KeyCode::Char('6') => {
                // HSL: radio behavior
                app.is_hsl_bg = !app.is_hsl_bg;
                if app.is_hsl_bg {
                    app.is_color256 = false;
                    app.is_rgb_bg = false;
                    app.is_gray_bg = false;
                    app.is_heat_bg = false;
                    app.is_hslbit_bg = false;
                    app.is_rgbbit_bg = false;
                }
            }
            KeyCode::Char('7') => {
                // GRAY: radio behavior
                app.is_gray_bg = !app.is_gray_bg;
                if app.is_gray_bg {
                    app.is_color256 = false;
                    app.is_rgb_bg = false;
                    app.is_hsl_bg = false;
                    app.is_heat_bg = false;
                    app.is_hslbit_bg = false;
                    app.is_rgbbit_bg = false;
                }
            }
            KeyCode::Char('8') => {
                // HEAT: radio behavior
                app.is_heat_bg = !app.is_heat_bg;
                if app.is_heat_bg {
                    app.is_color256 = false;
                    app.is_rgb_bg = false;
                    app.is_hsl_bg = false;
                    app.is_gray_bg = false;
                    app.is_hslbit_bg = false;
                    app.is_rgbbit_bg = false;
                }
            }
            KeyCode::Char('9') => {
                // hsl: radio behavior
                app.is_hslbit_bg = !app.is_hslbit_bg;
                if app.is_hslbit_bg {
                    app.is_color256 = false;
                    app.is_rgb_bg = false;
                    app.is_hsl_bg = false;
                    app.is_gray_bg = false;
                    app.is_heat_bg = false;
                    app.is_rgbbit_bg = false;
                }
            }
            KeyCode::Char('0') => {
                // rgb: radio behavior
                app.is_rgbbit_bg = !app.is_rgbbit_bg;
                if app.is_rgbbit_bg {
                    app.is_color256 = false;
                    app.is_rgb_bg = false;
                    app.is_hsl_bg = false;
                    app.is_gray_bg = false;
                    app.is_heat_bg = false;
                    app.is_hslbit_bg = false;
                }
            }
            _ => {}
        },
        InputMode::FileBrowser => {
            app.input_mode = InputMode::Normal;
        }
        InputMode::SaveConfirm => handle_save(app, key.code, data, filename, should_break),
        InputMode::SearchInput
        | InputMode::StringSearchInput
        | InputMode::GotoInput
        | InputMode::GotoByteInput => {
            handle_input(app, key.code, data, th);
        }
        InputMode::Edit => handle_edit(app, key.code, data, th),
        InputMode::Normal => handle_normal(app, key.code, data, th, max_rows, should_break),
    }
}

/// 主事件循环：渲染 → 处理输入 → 重复
fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    data: &mut [u8],
    filename: &str,
) -> io::Result<bool> {
    #[cfg(target_os = "windows")]
    let mut last_key_time = std::time::Instant::now();
    let mut reopen_browser = false;
    let mut should_break = false;

    loop {
        if should_break {
            break;
        }
        let area = terminal.size()?;
        let th = area.height;
        let max_rows = app.max_rows(th);

        // clamp scroll
        let gtr = app.global_total_rows();
        let gs = app.global_scroll_top();
        let max_gs = gtr.saturating_sub(max_rows);
        if gs > max_gs {
            app.set_global_scroll(max_gs);
        }

        // pre-compute help popup rect for click-outside detection
        if app.input_mode == InputMode::Help {
            if let Ok(sz) = terminal.size() {
                let aw = sz.width as usize;
                let ah = sz.height as usize;
                let max_h = (ah * 8 / 10).max(10);
                let max_w = (aw * 9 / 10).max(30);
                let h = max_h.min(40) as u16;
                let w = max_w.min(80) as u16;
                let y = (sz.height.saturating_sub(h)) / 2;
                let x = (sz.width.saturating_sub(w)) / 2;
                app.help_rect = Some((x, y, w, h));
            }
        } else {
            app.help_rect = None;
        }

        // render
        render_frame(terminal, app, data)?;

        // handle input
        let evt = event::read()?;
        match evt {
            Event::Mouse(mouse) => {
                handle_mouse_event(
                    app,
                    mouse,
                    area,
                    max_rows,
                    &mut reopen_browser,
                    &mut should_break,
                );
            }
            Event::Key(key) => {
                #[cfg(target_os = "windows")]
                if key.kind == event::KeyEventKind::Press
                    && last_key_time.elapsed().as_millis() < 40
                {
                    continue;
                }
                #[cfg(target_os = "windows")]
                {
                    last_key_time = std::time::Instant::now();
                }
                handle_key_event(
                    app,
                    key,
                    data,
                    filename,
                    th,
                    max_rows,
                    &mut reopen_browser,
                    &mut should_break,
                );
            }
            _ => {}
        }
    }
    Ok(reopen_browser || app.pending_file.is_some())
}

/// 保存确认弹窗的输入处理（y/n/space/esc）
fn handle_save(app: &mut App, code: KeyCode, data: &mut [u8], filename: &str, do_break: &mut bool) {
    match code {
        KeyCode::Left | KeyCode::Char('h') => {
            app.save_selected = !app.save_selected;
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.save_selected = !app.save_selected;
        }
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let _ = std::fs::write(filename, &*data);
            app.dirty = false;
            *do_break = true;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            *do_break = true;
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if app.save_selected {
                let _ = std::fs::write(filename, &*data);
                app.dirty = false;
            }
            *do_break = true;
        }
        _ => {}
    }
}

/// 文本输入模式处理（搜索/跳转输入框）
fn handle_input(app: &mut App, code: KeyCode, data: &mut [u8], th: u16) {
    match code {
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
            app.input_buf.clear();
        }
        KeyCode::Enter => {
            let buf = app.input_buf.clone();
            let mode = app.input_mode;
            app.input_mode = InputMode::Normal;
            app.input_buf.clear();
            match mode {
                InputMode::GotoInput => {
                    if let Some(val) = parse_hex_input(&buf) {
                        let target = val.saturating_sub(1);
                        if target < app.total_packs {
                            if app.search_active {
                                app.next_global(data, th);
                            } else {
                                app.current_pack = target;
                                app.scroll_top = 0;
                            }
                            app.cursor_byte = app.current_pack * app.pack_size;
                            app.cursor_focused = true;
                            app.ensure_cursor_visible(th);
                        }
                    }
                }
                InputMode::GotoByteInput => {
                    if let Some(val) = parse_hex_input(&buf) {
                        if val < app.file_size {
                            if app.search_active {
                                app.next_global(data, th);
                            } else {
                                app.current_pack = val / app.pack_size;
                                app.scroll_top = (val % app.pack_size) / 16;
                            }
                            app.cursor_byte = val;
                            app.cursor_focused = true;
                            app.ensure_cursor_visible(th);
                        }
                    }
                }
                InputMode::StringSearchInput => {
                    if let Some((label, bytes)) = search::parse_str_input(&buf) {
                        let len = bytes.len();
                        app.apply_search(crate::search::Needle::Lit(bytes), len, label, data, th);
                    }
                }
                InputMode::SearchInput => {
                    if let Some(kind) = search::parse_input(&buf) {
                        let (needle, needle_len, label) = match kind {
                            search::SearchKind::Hex { bytes, label } => {
                                let len = bytes.len();
                                (crate::search::Needle::Lit(bytes), len, label)
                            }
                            search::SearchKind::Pat { pat, label } => {
                                let len = pat.len() / 2;
                                (crate::search::Needle::Pat(pat), len, label)
                            }
                        };
                        app.apply_search(needle, needle_len, label, data, th);
                    }
                }
                _ => {}
            }
        }
        KeyCode::Backspace => {
            app.input_buf.pop();
        }
        KeyCode::Char(c) => {
            app.input_buf.push(c);
        }
        _ => {}
    }
}

/// 编辑模式输入处理：光标移动 + 字节编辑
///
/// 所有光标移动后调用 ensure_cursor_visible 防止越界。
/// 额外 clamp 确保 cursor_byte 在文件范围内。
fn handle_edit(app: &mut App, code: KeyCode, data: &mut [u8], th: u16) {
    match code {
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
        }
        KeyCode::Left => {
            if app.mode == DisplayMode::Hex {
                if app.cursor_nibble == 0 {
                    if app.cursor_byte > 0 {
                        app.cursor_byte -= 1;
                        app.cursor_nibble = 1;
                    } else if app.current_pack > 0 {
                        app.current_pack -= 1;
                        app.scroll_top = 0;
                        app.cursor_byte = (app.current_pack + 1) * app.pack_size - 1;
                        app.cursor_nibble = 1;
                    }
                } else {
                    app.cursor_nibble = 0;
                }
            } else if app.cursor_byte > 0 {
                app.cursor_byte -= 1;
            } else if app.current_pack > 0 {
                app.current_pack -= 1;
                app.scroll_top = 0;
                app.cursor_byte = (app.current_pack + 1) * app.pack_size - 1;
            }
            app.ensure_cursor_visible(th);
        }
        KeyCode::Right => {
            let last = app.file_size.saturating_sub(1);
            if app.mode == DisplayMode::Hex {
                if app.cursor_nibble == 0 {
                    app.cursor_nibble = 1;
                } else if app.cursor_byte < last {
                    app.cursor_byte += 1;
                    app.cursor_nibble = 0;
                } else if app.current_pack + 1 < app.total_packs {
                    app.current_pack += 1;
                    app.scroll_top = 0;
                    app.cursor_byte = app.current_pack * app.pack_size;
                    app.cursor_nibble = 0;
                }
            } else if app.cursor_byte < last {
                app.cursor_byte += 1;
            } else if app.current_pack + 1 < app.total_packs {
                app.current_pack += 1;
                app.scroll_top = 0;
                app.cursor_byte = app.current_pack * app.pack_size;
            }
            app.ensure_cursor_visible(th);
        }
        KeyCode::Up => {
            if app.cursor_byte >= 16 {
                app.cursor_byte -= 16;
            } else if app.current_pack > 0 {
                app.current_pack -= 1;
                app.scroll_top = 0;
                let rows = (app.pack_size / 16).saturating_sub(1);
                app.cursor_byte = app.current_pack * app.pack_size + rows * 16 + app.cursor_byte;
            }
            app.ensure_cursor_visible(th);
        }
        KeyCode::Down => {
            let last = app.file_size.saturating_sub(1);
            if app.cursor_byte + 16 <= last {
                app.cursor_byte += 16;
            } else if app.current_pack + 1 < app.total_packs {
                app.current_pack += 1;
                app.scroll_top = 0;
                let over = app.cursor_byte + 16 - last - 1;
                app.cursor_byte = (app.current_pack * app.pack_size + over).min(last);
            }
            app.ensure_cursor_visible(th);
        }
        KeyCode::PageUp => {
            let rows = app.max_rows(th);
            app.cursor_byte = app.cursor_byte.saturating_sub(rows * 16);
            app.ensure_cursor_visible(th);
        }
        KeyCode::PageDown => {
            let rows = app.max_rows(th);
            let target = app.cursor_byte + rows * 16;
            app.cursor_byte = if target < app.file_size {
                target
            } else {
                app.file_size - 1
            };
            app.ensure_cursor_visible(th);
        }
        KeyCode::Enter => {
            if app.mode == DisplayMode::Ascii {
                app.edit_char(data, '\n');
            }
            app.ensure_cursor_visible(th);
        }
        KeyCode::Tab => {
            if app.mode == DisplayMode::Ascii {
                app.edit_char(data, '\t');
            }
            app.ensure_cursor_visible(th);
        }
        KeyCode::Char(c) => {
            if app.mode == DisplayMode::Hex {
                if c.is_ascii_hexdigit() {
                    app.edit_hex(data, c);
                }
            } else {
                app.edit_char(data, c);
            }
            app.ensure_cursor_visible(th);
        }
        _ => {}
    }
    // 防止 cursor_byte 越界（边界情况）
    if app.file_size > 0 {
        app.cursor_byte = app.cursor_byte.min(app.file_size - 1);
    }
}

/// Normal 模式快捷键处理：导航、搜索、模式切换
fn handle_normal(
    app: &mut App,
    code: KeyCode,
    data: &mut [u8],
    th: u16,
    max_rows: usize,
    do_break: &mut bool,
) {
    if code == KeyCode::Esc {
        if app.search_active {
            app.clear_search();
        }
        return;
    }
    match code {
        KeyCode::Char('q') => {
            if app.dirty {
                app.input_mode = InputMode::SaveConfirm;
                app.save_selected = true;
            } else {
                *do_break = true;
            }
        }
        KeyCode::Char('?') => {
            app.input_mode = InputMode::Help;
            app.help_scroll = 0;
        }
        KeyCode::Char('m') => app.mode = app.mode.next(),
        KeyCode::Char('n') => {
            if app.is_color256 {
                app.is_color256 = false;
                app.is_rgb_bg = true;
            } else if app.is_rgb_bg {
                app.is_rgb_bg = false;
                app.is_hsl_bg = true;
            } else if app.is_hsl_bg {
                app.is_hsl_bg = false;
                app.is_gray_bg = true;
            } else if app.is_gray_bg {
                app.is_gray_bg = false;
                app.is_heat_bg = true;
            } else if app.is_heat_bg {
                app.is_heat_bg = false;
                app.is_hslbit_bg = true;
            } else if app.is_hslbit_bg {
                app.is_hslbit_bg = false;
                app.is_rgbbit_bg = true;
            } else if app.is_rgbbit_bg {
                app.is_rgbbit_bg = false;
            } else {
                app.is_color256 = true;
            }
        }
        KeyCode::Char('i') => {
            app.input_mode = InputMode::Edit;
            if app.cursor_byte == 0 && !app.dirty {
                app.cursor_byte = app.current_pack * app.pack_size + app.scroll_top * 16;
            }
            app.ensure_cursor_visible(th);
        }
        KeyCode::Char('g') => {
            app.input_mode = InputMode::GotoInput;
            app.input_prompt = "Go to pack (hex):".into();
            app.input_buf.clear();
        }
        KeyCode::Char('f') => {
            app.input_mode = InputMode::SearchInput;
            app.input_prompt = "search hex:".into();
            app.input_buf.clear();
        }
        KeyCode::Char('F') => {
            app.input_mode = InputMode::StringSearchInput;
            app.input_prompt = "Search STR:".into();
            app.input_buf.clear();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.search_active {
                app.prev_global(data, th);
            } else {
                let gs = app.global_scroll_top();
                if gs > 0 {
                    app.set_global_scroll(gs - 1);
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.search_active {
                app.next_global(data, th);
            } else {
                let gs = app.global_scroll_top();
                let max = app.global_total_rows().saturating_sub(max_rows);
                if gs < max {
                    app.set_global_scroll(gs + 1);
                }
            }
        }
        // 搜索模式下：跳到目标页的第一个匹配（步长与非搜索模式相同）
        KeyCode::Left | KeyCode::Char('h') => {
            if app.search_active {
                let target = app.current_pack.saturating_sub(1);
                app.jump_to_page_match_prev(target, data, th);
            } else if app.current_pack > 0 {
                app.current_pack -= 1;
                app.scroll_top = 0;
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if app.search_active {
                let target = (app.current_pack + 1).min(app.total_packs - 1);
                app.jump_to_page_match(target, data, th);
            } else if app.current_pack + 1 < app.total_packs {
                app.current_pack += 1;
                app.scroll_top = 0;
            }
        }
        KeyCode::Char('K') => {
            if app.search_active {
                // K = 上翻一屏 → 回退 max_rows 个 pack
                let packs_per_screen = (max_rows * 16 / app.pack_size).max(1);
                let target = app.current_pack.saturating_sub(packs_per_screen);
                app.jump_to_page_match_prev(target, data, th);
            } else {
                let gs = app.global_scroll_top();
                app.set_global_scroll(gs.saturating_sub(max_rows));
            }
        }
        KeyCode::Char('J') => {
            if app.search_active {
                let packs_per_screen = (max_rows * 16 / app.pack_size).max(1);
                let target = (app.current_pack + packs_per_screen).min(app.total_packs - 1);
                app.jump_to_page_match(target, data, th);
            } else {
                let gs = app.global_scroll_top();
                let max = app.global_total_rows().saturating_sub(max_rows);
                app.set_global_scroll((gs + max_rows).min(max));
            }
        }
        KeyCode::Char('H') => {
            if app.search_active {
                let target = app.current_pack.saturating_sub(16);
                app.jump_to_page_match_prev(target, data, th);
            } else {
                let target = app.current_pack.saturating_sub(16);
                app.current_pack = target;
                app.scroll_top = 0;
            }
        }
        KeyCode::Char('L') => {
            if app.search_active {
                let target = (app.current_pack + 16).min(app.total_packs - 1);
                app.jump_to_page_match(target, data, th);
            } else {
                let target = (app.current_pack + 16).min(app.total_packs - 1);
                app.current_pack = target;
                app.scroll_top = 0;
            }
        }
        KeyCode::PageUp => {
            if app.search_active {
                let step = (max_rows / 2).max(1);
                let packs = (step * 16 / app.pack_size).max(1);
                let target = app.current_pack.saturating_sub(packs);
                app.jump_to_page_match_prev(target, data, th);
            } else {
                let step = (max_rows / 2).max(1);
                let gs = app.global_scroll_top();
                app.set_global_scroll(gs.saturating_sub(step));
            }
        }
        KeyCode::PageDown => {
            if app.search_active {
                let step = (max_rows / 2).max(1);
                let packs = (step * 16 / app.pack_size).max(1);
                let target = (app.current_pack + packs).min(app.total_packs - 1);
                app.jump_to_page_match(target, data, th);
            } else {
                let step = (max_rows / 2).max(1);
                let gs = app.global_scroll_top();
                let max = app.global_total_rows().saturating_sub(max_rows);
                app.set_global_scroll((gs + step).min(max));
            }
        }
        KeyCode::Home => {
            if app.search_active {
                // Home → 第一个匹配
                app.current_match = None;
                app.next_global(data, th);
            } else {
                app.current_pack = 0;
                app.scroll_top = 0;
            }
        }
        KeyCode::Char('O') | KeyCode::Char('o') => {
            if app.search_active {
                app.prev_global(data, th);
            } else {
                let gs = app.global_scroll_top();
                let step = 256 * (app.pack_size / 16);
                app.set_global_scroll(gs.saturating_sub(step));
            }
        }
        KeyCode::Char('P') | KeyCode::Char('p') => {
            if app.search_active {
                app.next_global(data, th);
            } else {
                let gs = app.global_scroll_top();
                let step = 256 * (app.pack_size / 16);
                let max = app.global_total_rows().saturating_sub(max_rows);
                app.set_global_scroll((gs + step).min(max));
            }
        }
        _ => {}
    }
}

/// Termux 剪贴板写入（通过 termux-clipboard-set 命令）
#[cfg(target_os = "android")]
fn termux_clipboard_set(text: &str) {
    use std::io::Write;
    if let Ok(mut child) = std::process::Command::new("termux-clipboard-set")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

/// 解析十六进制输入字符串（支持 0x 前缀）
fn parse_hex_input(s: &str) -> Option<usize> {
    let s = s.trim();
    let s = if s.starts_with("0x") || s.starts_with("0X") {
        &s[2..]
    } else {
        s
    };
    let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if clean.is_empty() {
        return None;
    }
    usize::from_str_radix(&clean, 16).ok()
}

// ─── Rendering ───────────────────────────────────────────────

// ─── 渲染 ──────────────────────────────────────────────────
static COLOR_CFG: OnceLock<color_config::ColorConfig> = OnceLock::new();

/// 初始化全局颜色配置
pub fn init_colors(path: &std::path::Path) -> Result<(), String> {
    let cfg = color_config::ColorConfig::load(path)?;
    COLOR_CFG
        .set(cfg)
        .map_err(|_| "color config already set".to_string())
}

/// 按编号获取字节类型样式
///
/// 编号映射：
/// - 1=null, 2=head2, 3=tail, 4=control, 5=ascii, 6=head3
/// - 8=head4, 10=hex, 12/13=found, 15/17=selection, 16=cursor
fn sp(n: u8) -> Style {
    let c = COLOR_CFG.get();
    match n {
        1 => c.map(|x| x.sp_null).unwrap_or_default(),
        2 => c.map(|x| x.sp_head2).unwrap_or_default(),
        3 => c.map(|x| x.sp_tail).unwrap_or_default(),
        4 => c.map(|x| x.sp_control).unwrap_or_default(),
        5 => c.map(|x| x.sp_ascii).unwrap_or_default(),
        6 => c.map(|x| x.sp_head3).unwrap_or_default(),
        8 => c.map(|x| x.sp_head4).unwrap_or_default(),
        10 => c.map(|x| x.sp_hex).unwrap_or_default(),
        12 => c.map(|x| x.sp_found).unwrap_or_default(),
        13 => c.map(|x| x.sp_found).unwrap_or_default(),
        15 => c.map(|x| x.sp_selection).unwrap_or_default(),
        16 => c.map(|x| x.sp_cursor).unwrap_or_default(),
        17 => c.map(|x| x.sp_selection).unwrap_or_default(),
        _ => Style::default(),
    }
}

/// UTF-8 字节分类 → 样式映射
///
/// 根据字节类型（Ascii/Duo/Trio/Quo/Tail/Invalid）返回对应的颜色样式。
fn utf8_cls_style(cls: utf8::ByteClass) -> Style {
    match cls {
        utf8::ByteClass::Ascii => sp(5),
        utf8::ByteClass::Duo => sp(2),
        utf8::ByteClass::Trio => sp(6),
        utf8::ByteClass::Quo => sp(8),
        utf8::ByteClass::Tail => sp(3),
        utf8::ByteClass::Invalid => sp(14),
    }
}

/// 计算渐变色（蓝→绿），用于列号头的颜色渐变
fn grad_color(i: usize, total: usize) -> Color {
    let t = if total > 1 {
        i as f64 / (total - 1) as f64
    } else {
        0.0
    };
    let r = ((400.0 + 0.0 * t).min(1000.0) * 255.0 / 1000.0) as u8;
    let g = ((400.0 + 600.0 * t).min(1000.0) * 255.0 / 1000.0) as u8;
    let b = ((1000.0 - 0.0 * t).max(0.0) * 255.0 / 1000.0) as u8;
    Color::Rgb(r, g, b)
}

/// 字节显示文本
///
/// ASCII 模式：可打印字符显示为 `c `，不可打印显示为 hex 或符号（`.`/`⏎`/`·`）。
/// HEX/UTF8 模式：统一显示为 2 位 hex。
fn byte_disp(b: u8, mode: DisplayMode) -> String {
    match mode {
        DisplayMode::Ascii => {
            if b == 0 {
                ". ".into()
            } else if b == 0x09 {
                "↹ ".into()
            } else if b == 0x0d {
                "↵ ".into()
            } else if b == 10 {
                "⏎ ".into()
            } else if b == 0x1b {
                "\\e".into()
            } else if (0x01..=0x1f).contains(&b) {
                format!("{:02x}", b)
            } else if b == 0x20 {
                "· ".into()
            } else if (0x21..=0x7e).contains(&b) {
                format!("{} ", b as char)
            } else {
                format!("{:02x}", b)
            }
        }
        _ => format!("{:02x}", b),
    }
}

/// 字节类型分组（用于交替 dim 效果）
///
/// 相同类型的连续字节交替显示亮/暗背景，增强可读性。
/// 返回值 0-8 代表不同字节类别。
fn byte_type_group(b: u8, mode: DisplayMode) -> u8 {
    match mode {
        DisplayMode::Ascii => {
            if b == 0 {
                1
            } else if b == 0x0d {
                2
            } else if b == 10 || b == 0x20 {
                3
            } else if b == 0x1b || (0x01..=0x1f).contains(&b) {
                4
            } else if (0x21..=0x7e).contains(&b) {
                5
            } else if (0x80..=0xbf).contains(&b) {
                6
            } else {
                7
            }
        }
        DisplayMode::Hex => {
            if (0x20..=0x7e).contains(&b) {
                1
            } else if b == 0 {
                2
            } else if b == 0x0d {
                3
            } else if b == 10 {
                4
            } else if b == 0x1b || (0x01..=0x1f).contains(&b) {
                5
            } else if (0x80..=0xbf).contains(&b) {
                6
            } else {
                7
            }
        }
        _ => 0,
    }
}

/// 字节 → 基础样式映射（根据字节值和显示模式选择对应颜色）
fn byte_style(b: u8, mode: DisplayMode) -> Style {
    match mode {
        DisplayMode::Ascii => {
            if b == 0 {
                sp(1)
            } else if b == 0x0d {
                sp(2)
            } else if b == 10 || b == 0x20 {
                sp(5)
            } else if b == 0x1b || (0x01..=0x1f).contains(&b) {
                sp(4)
            } else if (0x21..=0x7e).contains(&b) {
                sp(5)
            } else if (0x80..=0xbf).contains(&b) {
                sp(6)
            } else {
                sp(8)
            }
        }
        DisplayMode::Hex => {
            if (0x20..=0x7e).contains(&b) {
                sp(10)
            } else if b == 0 {
                sp(1)
            } else if b == 0x0d {
                sp(2)
            } else if b == 10 {
                sp(5)
            } else if b == 0x1b || (0x01..=0x1f).contains(&b) {
                sp(4)
            } else if (0x80..=0xbf).contains(&b) {
                sp(6)
            } else {
                sp(8)
            }
        }
        DisplayMode::Utf8 => sp(5),
    }
}

/// UTF-8 字符类型分组（用于交替 dim 效果）
///
/// 按 Unicode 区块分类：ASCII、控制符、CJK、韩文、假名、标点等。
fn char_type_group(ch: char) -> u8 {
    let cp = ch as u32;
    if cp >= 0x21 && cp <= 0x7e {
        1
    }
    // Printable ASCII
    else if ch == '\n' || ch == '\r' || ch == '\t' {
        2
    }
    // Common control
    else if cp < 0x20 {
        3
    }
    // Other control
    else if cp >= 0x4E00 && cp <= 0x9FFF {
        4
    }
    // CJK
    else if cp >= 0xAC00 && cp <= 0xD7AF {
        5
    }
    // Hangul
    else if cp >= 0x3000 && cp <= 0x30FF {
        6
    }
    // CJK symbols + kana
    else if cp >= 0x2000 && cp <= 0x206F {
        7
    }
    // General punctuation
    else {
        8
    } // Other Unicode
}

/// UTF-8 字符 → 基础样式映射（按 Unicode 区块选择颜色）
fn utf8_char_style(ch: char) -> Style {
    let cp = ch as u32;
    let base = if cp >= 0x21 && cp <= 0x7e {
        sp(5)
    } else if ch == '\n' || ch == '\r' || ch == '\t' {
        sp(6)
    } else if cp < 0x20 {
        sp(4)
    } else if cp >= 0x80 && cp <= 0xBF {
        sp(3)
    } else if cp >= 0x2000 && cp <= 0x206F {
        sp(8)
    } else if cp >= 0x4E00 && cp <= 0x9FFF {
        sp(2)
    } else if cp >= 0xAC00 && cp <= 0xD7AF {
        sp(6)
    } else if cp >= 0x3000 && cp <= 0x30FF {
        sp(8)
    } else {
        sp(10)
    };
    base
}

/// 将颜色亮度降至 45%（用于编辑模式背景压暗）
fn dim_color(c: Color) -> Color {
    let (r, g, b) = color_config::color_rgb(c);
    Color::Rgb(
        (r as u16 * 45 / 100) as u8,
        (g as u16 * 45 / 100) as u8,
        (b as u16 * 45 / 100) as u8,
    )
}

/// 压暗样式背景色至 10%，前景色根据压暗后背景亮度自动选择黑/白
fn dim_style(s: Style) -> Style {
    let bg = s.bg.unwrap_or(Color::Rgb(30, 30, 30));
    let dimmed = dim_color(bg);
    let fg = if color_config::luminance(dimmed) > 128.0 {
        Color::Black
    } else {
        Color::White
    };
    s.bg(dimmed).fg(fg)
}

/// 按 ColorConfig 配置压暗背景色（交替 dim 效果）
fn dim_bg_10pct(s: Style) -> Style {
    COLOR_CFG.get().map(|c| c.dim_bg(s)).unwrap_or(s)
}

/// RGB 背景色：R=prev, G=self, B=next
///
/// 边界处理：无 prev 取 off+16，无 next 取 off-16。
fn rgb_bg(data: &[u8], off: usize, file_size: usize) -> Color {
    let r = if off > 0 {
        data[off - 1]
    } else {
        data.get(off + 16).copied().unwrap_or(0)
    };
    let g = data[off];
    let b = if off + 1 < file_size {
        data[off + 1]
    } else {
        data.get(off + 16).copied().unwrap_or(0)
    };
    Color::Rgb(r, g, b)
}

/// HSL → RGB 转换
///
/// h: 0.0~360.0, s: 0.0~1.0, l: 0.0~1.0
/// 返回 (r, g, b) 各 0~255。
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

/// 计算 HSL 背景色：H=prev, L=self, S=next
///
/// 边界处理同 rgb_bg。H 映射 0~360°，L/S 映射 0~100%。
fn hsl_bg(data: &[u8], off: usize, file_size: usize) -> Color {
    let b1 = if off > 0 {
        data[off - 1]
    } else {
        data.get(off + 16).copied().unwrap_or(0)
    };
    let b2 = data[off];
    let b3 = if off + 1 < file_size {
        data[off + 1]
    } else {
        data.get(off + 16).copied().unwrap_or(0)
    };
    let h = b1 as f64 * 360.0 / 255.0;
    let l = b2 as f64 / 255.0;
    let s = b3 as f64 / 255.0;
    let (r, g, b) = hsl_to_rgb(h, s, l);
    Color::Rgb(r, g, b)
}

/// 灰阶背景色：字节值直接映射为 RGB(v, v, v)
fn gray_bg(b: u8) -> Color {
    Color::Rgb(b, b, b)
}

/// 热力图背景色：黑→蓝→红→黄→白 四段线性插值
fn heat_bg(b: u8) -> Color {
    let v = b as u16;
    let (r, g, bl) = if v < 64 {
        (0, 0, v * 4)
    } else if v < 128 {
        ((v - 64) * 4, 0, (128 - v) * 4)
    } else if v < 192 {
        (255, (v - 128) * 4, 0)
    } else {
        (255, 255, (v - 192) * 4)
    };
    Color::Rgb(r as u8, g as u8, bl as u8)
}

/// 位分解 RGB 背景色：高2位=R，中4位=G，低2位=B
///
/// 字节布局：`0bRR_GGGG_BB`
/// - R (bits 7-6): 红 0~3 → 0~255（×85）
/// - G (bits 5-2): 绿 0~15 → 0~255（×17）
/// - B (bits 1-0): 蓝 0~3 → 0~255（×85）
fn rgbbit_bg(b: u8) -> Color {
    let r = ((b >> 6) & 0x03) * 85;
    let g = ((b >> 2) & 0x0F) * 17;
    let bl = (b & 0x03) * 85;
    Color::Rgb(r, g, bl)
}

/// 位分解 HSL 背景色：高4位=色相，中2位=亮度，低2位=饱和度
///
/// 字节布局：`0bHHHH_LLSS`
/// - H (bits 7-4): 色相 0°~337.5°（16 级，步长 22.5°）
/// - L (bits 3-2): 亮度 20%~80%（4 级）
/// - S (bits 1-0): 饱和 25%~100%（4 级）
fn hslbit_bg(b: u8) -> Color {
    let h = ((b >> 4) as f64) * 22.5; // 0~337.5
    let l = 0.2 + ((b >> 2) & 0x03) as f64 * 0.2; // 0.2~0.8
    let s = 0.25 + (b & 0x03) as f64 * 0.25; // 0.25~1.0
    let (r, g, bl) = hsl_to_rgb(h, s, l);
    Color::Rgb(r, g, bl)
}

const STD_COLORS: [(u8, u8, u8); 8] = [
    (0, 0, 0),
    (170, 0, 0),
    (0, 170, 0),
    (170, 85, 0),
    (0, 0, 170),
    (170, 0, 170),
    (0, 170, 170),
    (170, 170, 170),
];
const BRIGHT_COLORS: [(u8, u8, u8); 8] = [
    (85, 85, 85),
    (255, 85, 85),
    (85, 255, 85),
    (255, 255, 85),
    (85, 85, 255),
    (255, 85, 255),
    (85, 255, 255),
    (255, 255, 255),
];

/// 将 256 色索引转换为 RGB（16-231 为 6×6×6 色立方体）
fn cube_rgb(idx: u8) -> (u8, u8, u8) {
    let i = (idx - 16) as usize;
    let r = i / 36;
    let g = (i / 6) % 6;
    let b = i % 6;
    (
        if r == 0 { 0 } else { 55 + r * 40 } as u8,
        if g == 0 { 0 } else { 55 + g * 40 } as u8,
        if b == 0 { 0 } else { 55 + b * 40 } as u8,
    )
}

/// 将 256 色索引转换为 RGB（232-255 为 24 级灰度）
fn gray_rgb(idx: u8) -> (u8, u8, u8) {
    let v = 8 + (idx - 232) * 10;
    (v, v, v)
}

/// 将 256 色索引转换为 RGB（标准色/亮色/色立方体/灰度）
fn indexed_rgb(idx: u8) -> (u8, u8, u8) {
    match idx {
        0..=7 => STD_COLORS[idx as usize],
        8..=15 => BRIGHT_COLORS[(idx - 8) as usize],
        16..=231 => cube_rgb(idx),
        _ => gray_rgb(idx),
    }
}

/// 计算 256 色索引的感知亮度（BT.601 公式）
fn indexed_luminance(idx: u8) -> f64 {
    let (r, g, b) = indexed_rgb(idx);
    0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64
}

/// 解析 fg: auto 哨兵 → 实际前景色
///
/// 检测 AUTO_FG_SENTINEL，根据当前 bg 亮度选择 Black（亮背景）或 White（暗背景）。
fn resolve_auto_fg(s: Style) -> Style {
    if s.fg == Some(color_config::AUTO_FG_SENTINEL) {
        match s.bg {
            Some(bg) => {
                let fg = if color_config::luminance(bg) > 128.0 {
                    Color::Black
                } else {
                    Color::White
                };
                s.fg(fg)
            }
            None => s,
        }
    } else {
        s
    }
}

/// 样式优先级解析（单出口，末尾统一 resolve_auto_fg）
///
/// 优先级从高到低：cursor > found match > search highlight > selection > edit-dim > base
fn resolve(app: &App, off: usize, base: Style, mr: Option<(usize, usize)>) -> Style {
    let s = if app.cursor_focused && app.cursor_byte == off {
        sp(16)
    } else if let Some((ms, me)) = mr {
        if ms <= off && off < me {
            sp(13)
        } else {
            base
        }
    } else if let (Some(a), Some(b)) = (app.sel_start, app.sel_end) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if off >= lo && off <= hi {
            if off == a {
                sp(15)
            } else {
                sp(17)
            }
        } else {
            base
        }
    } else if app.input_mode == InputMode::Edit && !app.is_gray_bg {
        dim_style(base)
    } else {
        base
    };
    // 编辑过的字节渲染为斜体（Sparse Hierarchical Bitmap O(1) 查询）
    let s = if app.modified.is_modified(off) {
        s.add_modifier(ratatui::style::Modifier::ITALIC)
    } else {
        s
    };
    resolve_auto_fg(s)
}

/// 绘制主视图（hex/ascii/utf8 内容区）
/// 绘制顶栏（文件名 + 大小）和主视图
fn draw_hex(f: &mut ratatui::Frame, app: &App, data_full: &[u8], area: Rect) {
    // 顶栏：*文件名 [大小]
    let size_str = App::format_size(app.file_size);
    let dirty_prefix = if app.dirty { "*" } else { "" };
    let top_bar = format!(
        "{}{} [{}]",
        dirty_prefix,
        app.filename,
        size_str.replace(' ', "_")
    );
    let pad = area.width.saturating_sub(top_bar.len() as u16) as usize;
    let top_bar_full = format!("{}{}", top_bar, " ".repeat(pad));
    let mut top_style = Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 60));
    if app.dirty {
        top_style = top_style.add_modifier(ratatui::style::Modifier::ITALIC);
    }
    f.render_widget(
        Paragraph::new(Span::styled(top_bar_full, top_style)),
        Rect::new(0, 0, area.width, 1),
    );
    // 数据区从第 2 行开始（第 0 行顶栏，第 1 行列号头在 build_lines 中）
    let data_area = Rect::new(0, 1, area.width, area.height.saturating_sub(2));
    let lines = build_lines(app, data_full, data_area);
    f.render_widget(Paragraph::new(lines), data_area);
}

/// 构建渲染行数据（跨页）
///
/// 以全局行号遍历，每行独立计算所在 pack 和页内偏移。
/// 支持滚过页边界时无缝渲染相邻页数据。
/// UTF8 模式下处理跨行多字节序列（tail bytes spill）。
/// 相同类型连续字节交替 dim 增强可读性。
fn build_lines<'a>(app: &App, data_full: &[u8], area: Rect) -> Vec<Line<'a>> {
    let mut lines: Vec<Line<'a>> = Vec::new();

    // gradient header
    let hdr = "    0 1 2 3 4 5 6 7 8 9 a b c d e f ";
    let leading = 4;
    let glen = hdr.len() - leading;
    let mut hspans: Vec<Span<'a>> = Vec::new();
    for (i, ch) in hdr.chars().enumerate() {
        let col = if i < leading {
            Color::White
        } else {
            grad_color(i - leading, glen)
        };
        hspans.push(Span::styled(ch.to_string(), Style::default().fg(col)));
    }
    lines.push(Line::from(hspans));

    let global_total = app.global_total_rows();
    // data_area 从第 1 行开始（顶栏在外部），减 1 行列号头 = 数据行数
    let max_rows = (area.height as usize).saturating_sub(1);
    let global_start = app.global_scroll_top();
    let global_end = (global_start + max_rows).min(global_total);
    let mr = app.current_match_range();
    let _rows_per_pack = app.pack_size / 16;

    let mut cross_row_tail: usize = 0;
    for gi in global_start..global_end {
        let (pack_idx, row_in_pack) = app.global_to_local(gi);
        let base_off = pack_idx * app.pack_size;
        let data = &data_full[base_off..(base_off + app.pack_size).min(data_full.len())];
        let off = row_in_pack * 16;
        let rem = 16.min(data.len().saturating_sub(off));

        let mut spans: Vec<Span<'a>> = Vec::new();
        // 行号渐变色：蓝(00) → 紫 → 粉(FF)
        let hex = format!("{:02x}", row_in_pack);
        let ratio = row_in_pack as f64 / 255.0;
        let (r, g, b) = if ratio < 0.5 {
            let t = ratio * 2.0;
            (
                (100.0 + (147.0 - 100.0) * t) as u8,
                (149.0 + (112.0 - 149.0) * t) as u8,
                (237.0 + (219.0 - 237.0) * t) as u8,
            )
        } else {
            let t = (ratio - 0.5) * 2.0;
            (
                (147.0 + (219.0 - 147.0) * t) as u8,
                112.0 as u8,
                (219.0 + (147.0 - 219.0) * t) as u8,
            )
        };
        let line_color = Color::Rgb(r, g, b);
        for ch in hex.chars() {
            spans.push(Span::styled(
                ch.to_string(),
                Style::default().fg(line_color),
            ));
        }
        spans.push(Span::raw("  "));

        if app.mode == DisplayMode::Utf8 {
            for t in 0..cross_row_tail {
                let p = off - cross_row_tail + t;
                let go = base_off + p;
                let tail_b = data[p];
                let ts = if app.is_rgb_bg {
                    let bg = rgb_bg(data_full, go, app.file_size);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    resolve(app, go, Style::default().bg(bg).fg(fg), mr)
                } else if app.is_hsl_bg {
                    let bg = hsl_bg(data_full, go, app.file_size);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    resolve(app, go, Style::default().bg(bg).fg(fg), mr)
                } else if app.is_gray_bg {
                    let bg = gray_bg(tail_b);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    resolve(app, go, Style::default().bg(bg).fg(fg), mr)
                } else if app.is_heat_bg {
                    let bg = heat_bg(tail_b);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    resolve(app, go, Style::default().bg(bg).fg(fg), mr)
                } else if app.is_hslbit_bg {
                    let bg = hslbit_bg(tail_b);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    resolve(app, go, Style::default().bg(bg).fg(fg), mr)
                } else if app.is_rgbbit_bg {
                    let bg = rgbbit_bg(tail_b);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    resolve(app, go, Style::default().bg(bg).fg(fg), mr)
                } else if app.is_color256 {
                    let fg = if indexed_luminance(tail_b) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    resolve(
                        app,
                        go,
                        Style::default().bg(Color::Indexed(tail_b)).fg(fg),
                        mr,
                    )
                } else {
                    resolve(app, go, sp(3), mr)
                };
                spans.push(Span::styled("··".to_string(), ts));
            }
            let segs = utf8::decode_row(data, off, rem, cross_row_tail);
            cross_row_tail = 0;
            let mut prev_type: u8 = 0;
            let mut same_count: usize = 0;
            for seg in &segs {
                match seg {
                    utf8::Segment::Char { pos, ch, len } => {
                        let bo = off + pos;
                        let go = base_off + bo;
                        let hb = data[bo];
                        let _cls = utf8::byte_class(hb);
                        let dw = utf8::display_width(*ch);
                        let dc = match *ch {
                            '\0' => ". ".into(),
                            '\n' => "⏎ ".into(),
                            '\r' => "↵ ".into(),
                            '\x1b' => "\\e".into(),
                            '\t' => "↹ ".into(),
                            c if (c as u32) < 0x20 => format!("{:02x}", c as u8),
                            _ => {
                                let s: String = ch.to_string();
                                if dw == 1 {
                                    format!("{} ", s)
                                } else {
                                    s
                                }
                            }
                        };
                        let cur_type = char_type_group(*ch);
                        if cur_type == prev_type {
                            same_count += 1;
                        } else {
                            same_count = 0;
                            prev_type = cur_type;
                        }
                        let dim = same_count % 2 == 1;
                        let base = if app.is_rgb_bg {
                            let bg = rgb_bg(data_full, go, app.file_size);
                            let fg = if color_config::luminance(bg) > 128.0 {
                                Color::Black
                            } else {
                                Color::White
                            };
                            Style::default().bg(bg).fg(fg)
                        } else if app.is_color256 {
                            let fg = if indexed_luminance(*ch as u8) > 128.0 {
                                Color::Black
                            } else {
                                Color::White
                            };
                            Style::default().bg(Color::Indexed(*ch as u8)).fg(fg)
                        } else if (*ch as u32) < 0x20 {
                            byte_style(*ch as u8, DisplayMode::Ascii)
                        } else {
                            utf8_char_style(*ch)
                        };
                        let sty = resolve(app, go, base, mr);
                        let final_sty = if dim { dim_bg_10pct(sty) } else { sty };
                        spans.push(Span::styled(dc, final_sty));
                        for ci in 1..*len {
                            if pos + ci >= rem {
                                cross_row_tail = *len - ci;
                                break;
                            }
                            let cgo = base_off + off + pos + ci;
                            let tail_b = data[off + pos + ci];
                            let ts = if app.is_rgb_bg {
                                let bg = rgb_bg(data_full, cgo, app.file_size);
                                let fg = if color_config::luminance(bg) > 128.0 {
                                    Color::Black
                                } else {
                                    Color::White
                                };
                                resolve(app, cgo, Style::default().bg(bg).fg(fg), mr)
                            } else if app.is_hsl_bg {
                                let bg = hsl_bg(data_full, cgo, app.file_size);
                                let fg = if color_config::luminance(bg) > 128.0 {
                                    Color::Black
                                } else {
                                    Color::White
                                };
                                resolve(app, cgo, Style::default().bg(bg).fg(fg), mr)
                            } else if app.is_gray_bg {
                                let bg = gray_bg(tail_b);
                                let fg = if color_config::luminance(bg) > 128.0 {
                                    Color::Black
                                } else {
                                    Color::White
                                };
                                resolve(app, cgo, Style::default().bg(bg).fg(fg), mr)
                            } else if app.is_heat_bg {
                                let bg = heat_bg(tail_b);
                                let fg = if color_config::luminance(bg) > 128.0 {
                                    Color::Black
                                } else {
                                    Color::White
                                };
                                resolve(app, cgo, Style::default().bg(bg).fg(fg), mr)
                            } else if app.is_hslbit_bg {
                                let bg = hslbit_bg(tail_b);
                                let fg = if color_config::luminance(bg) > 128.0 {
                                    Color::Black
                                } else {
                                    Color::White
                                };
                                resolve(app, cgo, Style::default().bg(bg).fg(fg), mr)
                            } else if app.is_rgbbit_bg {
                                let bg = rgbbit_bg(tail_b);
                                let fg = if color_config::luminance(bg) > 128.0 {
                                    Color::Black
                                } else {
                                    Color::White
                                };
                                resolve(app, cgo, Style::default().bg(bg).fg(fg), mr)
                            } else if app.is_color256 {
                                let fg = if indexed_luminance(tail_b) > 128.0 {
                                    Color::Black
                                } else {
                                    Color::White
                                };
                                resolve(
                                    app,
                                    cgo,
                                    Style::default().bg(Color::Indexed(tail_b)).fg(fg),
                                    mr,
                                )
                            } else {
                                resolve(app, cgo, sp(3), mr)
                            };
                            spans.push(Span::styled("··".to_string(), ts));
                        }
                    }
                    utf8::Segment::Invalid { pos } => {
                        let bo = off + pos;
                        let go = base_off + bo;
                        let b = data[bo];
                        let base = if app.is_rgb_bg {
                            let bg = rgb_bg(data_full, go, app.file_size);
                            let fg = if color_config::luminance(bg) > 128.0 {
                                Color::Black
                            } else {
                                Color::White
                            };
                            Style::default().bg(bg).fg(fg)
                        } else if app.is_hsl_bg {
                            let bg = hsl_bg(data_full, go, app.file_size);
                            let fg = if color_config::luminance(bg) > 128.0 {
                                Color::Black
                            } else {
                                Color::White
                            };
                            Style::default().bg(bg).fg(fg)
                        } else if app.is_gray_bg {
                            let bg = gray_bg(b);
                            let fg = if color_config::luminance(bg) > 128.0 {
                                Color::Black
                            } else {
                                Color::White
                            };
                            Style::default().bg(bg).fg(fg)
                        } else if app.is_heat_bg {
                            let bg = heat_bg(b);
                            let fg = if color_config::luminance(bg) > 128.0 {
                                Color::Black
                            } else {
                                Color::White
                            };
                            Style::default().bg(bg).fg(fg)
                        } else if app.is_hslbit_bg {
                            let bg = hslbit_bg(b);
                            let fg = if color_config::luminance(bg) > 128.0 {
                                Color::Black
                            } else {
                                Color::White
                            };
                            Style::default().bg(bg).fg(fg)
                        } else if app.is_rgbbit_bg {
                            let bg = rgbbit_bg(b);
                            let fg = if color_config::luminance(bg) > 128.0 {
                                Color::Black
                            } else {
                                Color::White
                            };
                            Style::default().bg(bg).fg(fg)
                        } else if app.is_color256 {
                            let fg = if indexed_luminance(b) > 128.0 {
                                Color::Black
                            } else {
                                Color::White
                            };
                            Style::default().bg(Color::Indexed(b)).fg(fg)
                        } else {
                            utf8_cls_style(utf8::byte_class(b))
                        };
                        let sty = resolve(app, go, base, mr);
                        spans.push(Span::styled(format!("{:02x}", b), sty));
                    }
                }
            }
        } else {
            let mut prev_type: u8 = 0;
            let mut same_count: usize = 0;
            for i in 0..16 {
                if off + i >= data.len() {
                    spans.push(Span::raw("  ".to_string()));
                    continue;
                }
                let b = data[off + i];
                let go = base_off + off + i;
                let cur_type = byte_type_group(b, app.mode);
                if cur_type == prev_type {
                    same_count += 1;
                } else {
                    same_count = 0;
                    prev_type = cur_type;
                }
                let dim = same_count % 2 == 1;
                if app.input_mode == InputMode::Edit && app.cursor_byte == go {
                    let d = byte_disp(b, app.mode);
                    let non_cursor_style = if app.is_rgb_bg {
                        let bg = rgb_bg(data_full, go, app.file_size);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Style::default().bg(bg).fg(fg)
                    } else if app.is_hsl_bg {
                        let bg = hsl_bg(data_full, go, app.file_size);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Style::default().bg(bg).fg(fg)
                    } else if app.is_gray_bg {
                        let bg = gray_bg(b);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Style::default().bg(bg).fg(fg)
                    } else if app.is_heat_bg {
                        let bg = heat_bg(b);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Style::default().bg(bg).fg(fg)
                    } else if app.is_hslbit_bg {
                        let bg = hslbit_bg(b);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Style::default().bg(bg).fg(fg)
                    } else if app.is_rgbbit_bg {
                        let bg = rgbbit_bg(b);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Style::default().bg(bg).fg(fg)
                    } else if app.is_color256 {
                        let fg = if indexed_luminance(b) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Style::default().bg(Color::Indexed(b)).fg(fg)
                    } else if app.is_gray_bg {
                        byte_style(b, app.mode)
                    } else {
                        dim_style(byte_style(b, app.mode))
                    };
                    match (app.mode, app.cursor_nibble) {
                        (DisplayMode::Hex, 0) => {
                            let c0: String = d.chars().take(1).collect();
                            let c1: String = d.chars().skip(1).take(1).collect();
                            spans.push(Span::styled(c0, sp(16)));
                            spans.push(Span::styled(c1, non_cursor_style));
                        }
                        (DisplayMode::Hex, 1) => {
                            let c0: String = d.chars().take(1).collect();
                            let c1: String = d.chars().skip(1).take(1).collect();
                            spans.push(Span::styled(c0, non_cursor_style));
                            spans.push(Span::styled(c1, sp(16)));
                        }
                        _ => spans.push(Span::styled(d, sp(16))),
                    }
                    continue;
                }
                let base = if app.is_rgb_bg {
                    let bg = rgb_bg(data_full, go, app.file_size);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    Style::default().bg(bg).fg(fg)
                } else if app.is_hsl_bg {
                    let bg = hsl_bg(data_full, go, app.file_size);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    Style::default().bg(bg).fg(fg)
                } else if app.is_gray_bg {
                    let bg = gray_bg(b);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    Style::default().bg(bg).fg(fg)
                } else if app.is_heat_bg {
                    let bg = heat_bg(b);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    Style::default().bg(bg).fg(fg)
                } else if app.is_hslbit_bg {
                    let bg = hslbit_bg(b);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    Style::default().bg(bg).fg(fg)
                } else if app.is_rgbbit_bg {
                    let bg = rgbbit_bg(b);
                    let fg = if color_config::luminance(bg) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    Style::default().bg(bg).fg(fg)
                } else if app.is_color256 {
                    let fg = if indexed_luminance(b) > 128.0 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    Style::default().bg(Color::Indexed(b)).fg(fg)
                } else {
                    byte_style(b, app.mode)
                };
                let sty = resolve(app, go, base, mr);
                let final_sty = if app.is_color256
                    || app.is_rgb_bg
                    || app.is_hsl_bg
                    || app.is_gray_bg
                    || app.is_heat_bg
                    || app.is_hslbit_bg
                    || app.is_rgbbit_bg
                {
                    sty
                } else if dim {
                    dim_bg_10pct(sty)
                } else {
                    sty
                };
                spans.push(Span::styled(byte_disp(b, app.mode), final_sty));
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// 绘制底部状态栏（模式、偏移、搜索状态、帮助提示）
///
/// 底栏可点击区域：
/// - [ASCII]/[HEX]/[UTF8] → 模式选择下拉菜单
/// - &address → 跳转到字节地址
/// - pack → 跳转到指定页
/// - Ctrl+H:help → 打开帮助窗口
fn draw_status(f: &mut ratatui::Frame, app: &App, data: &[u8], area: Rect) {
    f.render_widget(Clear, Rect::new(0, area.height - 1, area.width, 1));
    let text = match app.input_mode {
        InputMode::Edit => {
            // 编辑模式状态栏：模式 + 字节值 + 地址 + 页码
            let b = if app.cursor_byte < app.file_size {
                data[app.cursor_byte]
            } else {
                0
            };
            let byte_span = if app.is_color256 && app.cursor_byte < app.file_size {
                // 256 色模式：字节值背景为其调色板色，前景自适应
                let bg = Color::Indexed(b);
                let fg = if indexed_luminance(b) > 128.0 {
                    Color::Black
                } else {
                    Color::White
                };
                Span::styled(format!(" [{:02X}]", b), Style::default().fg(fg).bg(bg))
            } else if app.is_gray_bg && app.cursor_byte < app.file_size {
                let bg = gray_bg(b);
                let fg = if color_config::luminance(bg) > 128.0 {
                    Color::Black
                } else {
                    Color::White
                };
                Span::styled(format!(" [{:02X}]", b), Style::default().fg(fg).bg(bg))
            } else if app.is_heat_bg && app.cursor_byte < app.file_size {
                let bg = heat_bg(b);
                let fg = if color_config::luminance(bg) > 128.0 {
                    Color::Black
                } else {
                    Color::White
                };
                Span::styled(format!(" [{:02X}]", b), Style::default().fg(fg).bg(bg))
            } else if app.is_hslbit_bg && app.cursor_byte < app.file_size {
                let bg = hslbit_bg(b);
                let fg = if color_config::luminance(bg) > 128.0 {
                    Color::Black
                } else {
                    Color::White
                };
                Span::styled(format!(" [{:02X}]", b), Style::default().fg(fg).bg(bg))
            } else if app.is_rgbbit_bg && app.cursor_byte < app.file_size {
                let bg = rgbbit_bg(b);
                let fg = if color_config::luminance(bg) > 128.0 {
                    Color::Black
                } else {
                    Color::White
                };
                Span::styled(format!(" [{:02X}]", b), Style::default().fg(fg).bg(bg))
            } else {
                Span::raw(format!(" [{:02X}]", b))
            };
            let hex_w = if app.file_size <= 0xff {
                2
            } else if app.file_size <= 0xffff {
                4
            } else if app.file_size <= 0xffffff {
                6
            } else {
                8
            };
            let offset_str = format!("& {:0width$x}", app.cursor_byte, width = hex_w);
            let cur_pack = app.cursor_byte / app.pack_size + 1;
            let pack_str = format!("@{:x}/{:x}", cur_pack, app.total_packs);
            return f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        match app.mode {
                            DisplayMode::Ascii => "[EDIT ASCII]",
                            DisplayMode::Utf8 => "[EDIT UTF8]",
                            DisplayMode::Hex => "[EDIT HEX]",
                        },
                        sp(16),
                    ),
                    byte_span,
                    Span::raw(format!("  {}  {}", offset_str, pack_str)),
                ])),
                Rect::new(0, area.height - 1, area.width, 1),
            );
        }
        InputMode::SearchInput
        | InputMode::StringSearchInput
        | InputMode::GotoInput
        | InputMode::GotoByteInput => {
            return f.render_widget(
                Paragraph::new(Span::raw(format!("{} {}", app.input_prompt, app.input_buf))),
                Rect::new(0, area.height - 1, area.width, 1),
            );
        }
        InputMode::SaveConfirm => "Save changes? [Yes] [No]",
        InputMode::Help => "Press any key to close",
        InputMode::Normal => {
            if app.search_active {
                if let Some(ref s) = app.search {
                    let total = s.count;
                    let plus = if s.has_more() { "+" } else { "" };
                    // 搜索时显示当前匹配所在的页码
                    let cur_pack = app
                        .current_match
                        .map(|pos| pos / app.pack_size + 1)
                        .unwrap_or(0);
                    // 当前是第几个匹配
                    let cur_num = app.current_match_number(data);
                    let mut disp = s.label.clone();
                    if disp.len() > 24 {
                        disp.truncate(24);
                        disp.push_str("...");
                    }
                    let status = format!(
                        "Search: {} [{}/{}{}] @{:x}/{:x}  ↑↓:next ESC:clear",
                        disp, cur_num, total, plus, cur_pack, app.total_packs
                    );
                    return f.render_widget(
                        Paragraph::new(Span::styled(status, sp(5))),
                        Rect::new(0, area.height - 1, area.width, 1),
                    );
                }
            }
            let dirty = if app.dirty { " [MODIFIED]" } else { "" };
            // hex width based on file size
            let hex_w = if app.file_size <= 0xff {
                2
            } else if app.file_size <= 0xffff {
                4
            } else if app.file_size <= 0xffffff {
                6
            } else {
                8
            };
            let offset_str = format!("& {:0width$x}", app.cursor_byte, width = hex_w);
            let max_rows = app.max_rows(area.height);
            let last_global_row = (app.global_scroll_top() + max_rows - 1)
                .min(app.global_total_rows().saturating_sub(1));
            let last_pack = last_global_row / (app.pack_size / 16);
            let pack_str = format!("pack {:x}/{:x}", last_pack + 1, app.total_packs);
            let help_spans: Vec<Span> = vec![
                Span::raw("  ["),
                Span::styled(
                    "M",
                    Style::default().add_modifier(ratatui::style::Modifier::UNDERLINED),
                ),
                Span::raw("ENU]"),
            ];

            let mode_label = app.mode.label();
            let mut spans = if app.is_rgb_bg {
                let label_chars: Vec<char> = mode_label.chars().collect();
                let n = label_chars.len();
                label_chars
                    .iter()
                    .enumerate()
                    .map(|(i, &c)| {
                        let r = if i > 0 {
                            label_chars[i - 1] as u8
                        } else {
                            c as u8
                        };
                        let g = c as u8;
                        let b = if i + 1 < n {
                            label_chars[i + 1] as u8
                        } else {
                            c as u8
                        };
                        let bg = Color::Rgb(r, g, b);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Span::styled(c.to_string(), Style::default().fg(fg).bg(bg))
                    })
                    .collect::<Vec<_>>()
            } else if app.is_hsl_bg {
                let label_chars: Vec<char> = mode_label.chars().collect();
                let n = label_chars.len();
                label_chars
                    .iter()
                    .enumerate()
                    .map(|(i, &c)| {
                        let b1 = if i > 0 {
                            label_chars[i - 1] as u8
                        } else {
                            c as u8
                        };
                        let b2 = c as u8;
                        let b3 = if i + 1 < n {
                            label_chars[i + 1] as u8
                        } else {
                            c as u8
                        };
                        let h = b1 as f64 * 360.0 / 255.0;
                        let l = b2 as f64 / 255.0;
                        let s = b3 as f64 / 255.0;
                        let (rr, gg, bb) = hsl_to_rgb(h, s, l);
                        let bg = Color::Rgb(rr, gg, bb);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Span::styled(c.to_string(), Style::default().fg(fg).bg(bg))
                    })
                    .collect::<Vec<_>>()
            } else if app.is_gray_bg {
                let label_chars: Vec<char> = mode_label.chars().collect();
                label_chars
                    .iter()
                    .map(|&c| {
                        let bg = gray_bg(c as u8);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Span::styled(c.to_string(), Style::default().fg(fg).bg(bg))
                    })
                    .collect::<Vec<_>>()
            } else if app.is_heat_bg {
                let label_chars: Vec<char> = mode_label.chars().collect();
                label_chars
                    .iter()
                    .map(|&c| {
                        let bg = heat_bg(c as u8);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Span::styled(c.to_string(), Style::default().fg(fg).bg(bg))
                    })
                    .collect::<Vec<_>>()
            } else if app.is_hslbit_bg {
                let label_chars: Vec<char> = mode_label.chars().collect();
                label_chars
                    .iter()
                    .map(|&c| {
                        let bg = hslbit_bg(c as u8);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Span::styled(c.to_string(), Style::default().fg(fg).bg(bg))
                    })
                    .collect::<Vec<_>>()
            } else if app.is_rgbbit_bg {
                let label_chars: Vec<char> = mode_label.chars().collect();
                label_chars
                    .iter()
                    .map(|&c| {
                        let bg = rgbbit_bg(c as u8);
                        let fg = if color_config::luminance(bg) > 128.0 {
                            Color::Black
                        } else {
                            Color::White
                        };
                        Span::styled(c.to_string(), Style::default().fg(fg).bg(bg))
                    })
                    .collect::<Vec<_>>()
            } else if app.is_color256 {
                let grad = [
                    Color::Rgb(100, 149, 237),
                    Color::Rgb(123, 137, 231),
                    Color::Rgb(147, 125, 225),
                    Color::Rgb(171, 113, 219),
                    Color::Rgb(195, 101, 213),
                    Color::Rgb(219, 89, 207),
                    Color::Rgb(219, 112, 147),
                ];
                mode_label
                    .chars()
                    .enumerate()
                    .map(|(i, c)| {
                        Span::styled(c.to_string(), Style::default().fg(Color::White).bg(grad[i]))
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![Span::styled(mode_label, sp(5))]
            };
            spans.push(Span::styled(dirty, sp(5)));
            spans.push(Span::styled(
                format!("  {}  {}", offset_str, pack_str),
                sp(5),
            ));
            spans.extend(help_spans);
            return f.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(0, area.height - 1, area.width, 1),
            );
        }
        InputMode::ModeSelect => {
            return f.render_widget(
                Paragraph::new(Span::styled(
                    "↑↓:select Enter:confirm Esc:cancel 1/2/3:mode 4:256 5:RGB 6:HSL 7:GRAY 8:HEAT 9:hsl",
                    sp(5),
                )),
                Rect::new(0, area.height - 1, area.width, 1),
            );
        }
        InputMode::FileBrowser => {
            return f.render_widget(
                Paragraph::new(Span::styled("File Browser", sp(5))),
                Rect::new(0, area.height - 1, area.width, 1),
            );
        }
        InputMode::Menu | InputMode::About => "",
    };
    f.render_widget(
        Paragraph::new(Span::styled(text, sp(5))),
        Rect::new(0, area.height - 1, area.width, 1),
    );
}

/// 绘制菜单下拉弹窗（Help / Sample / About）
///
/// 位置：右对齐 [MENU] 按钮正上方
fn draw_menu_dropdown(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let dw = 14u16;
    let dh = 3u16;
    // 计算 [MENU] 按钮的水平位置（与 draw_status 中的 help_offset 一致）
    let dirty_len = if app.dirty { 11 } else { 0 };
    let hex_w = if app.file_size <= 0xff {
        2
    } else if app.file_size <= 0xffff {
        4
    } else if app.file_size <= 0xffffff {
        6
    } else {
        8
    };
    let mode_len = app.mode.label().len() as u16;
    let at_offset = mode_len + dirty_len as u16 + 2;
    let at_len = 1 + hex_w as u16;
    let pack_offset = at_offset + at_len + 2;
    let pack_str_len = format!(
        "pack {:x}/{:x}",
        (app.cursor_byte / app.pack_size) + 1,
        app.total_packs
    )
    .len() as u16;
    let help_offset = pack_offset + pack_str_len + 2;
    let dx = help_offset;
    let dy = area.height.saturating_sub(1).saturating_sub(dh);
    let dialog = Rect::new(dx, dy, dw, dh);
    f.render_widget(Clear, dialog);
    for i in 0..3 {
        let (base, shortcut) = match i {
            0 => ("Help", 'H'),
            1 => ("Sample", 'S'),
            2 => ("About", 'A'),
            _ => ("", ' '),
        };
        let sty = if i == app.menu_selected {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default()
        };
        let line = Line::from(vec![
            Span::styled(" ", sty),
            Span::styled(
                shortcut.to_string(),
                sty.add_modifier(ratatui::style::Modifier::UNDERLINED),
            ),
            Span::styled(&base[1..], sty),
            Span::styled(" ", sty),
        ]);
        f.render_widget(Paragraph::new(line), Rect::new(dx, dy + i as u16, dw, 1));
    }
}

/// 绘制 About 弹窗（居中显示版本、作者、仓库、许可证）
fn draw_about(f: &mut ratatui::Frame, area: Rect) {
    let ver = env!("CARGO_PKG_VERSION");
    let text = vec![
        format!("read-bin v{}", ver),
        String::new(),
        "Terminal hex viewer/editor".to_string(),
        "Author: Saisui".to_string(),
        String::new(),
        "Features:".to_string(),
        "  BitSearch 4-lv bitmap (804B)".to_string(),
        "  Sparse Hierarchical Bitmap".to_string(),
        "  8 color modes".to_string(),
        "  Edit + undo/redo".to_string(),
        "  Sample file (0x00-0xFF)".to_string(),
        String::new(),
        "github.com/Saisui/read_bin_cli".to_string(),
        "License: AGPL-3.0".to_string(),
    ];
    let h = text.len() as u16 + 2;
    let w = 36u16;
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);
    f.render_widget(Clear, popup);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title("About")
            .style(Style::default().bg(Color::Rgb(20, 20, 40))),
        popup,
    );
    let inner = Rect::new(x + 1, y + 1, w - 2, h - 2);
    let lines: Vec<Line> = text
        .iter()
        .map(|l| Line::from(Span::raw(l.to_string())))
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

/// 帮助文本内容（从 help.txt 加载，编译时嵌入）
const HELP_LINES: &str = include_str!("../help.txt");

/// 绘制帮助弹窗（自适应大小，带可拖拽滚动条）
fn draw_help(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let ver = env!("CARGO_PKG_VERSION");
    let title = format!("=== read-bin v{} by Saisui ===", ver);
    let mut lines_text: Vec<&str> = Vec::new();
    lines_text.push(&title);
    lines_text.push("");
    for line in HELP_LINES.lines() {
        lines_text.push(line);
    }
    let total = lines_text.len();
    // 自适应：高度取终端 80%，宽度取终端 90%，最少 20×30
    let max_h = ((area.height as usize) * 8 / 10).max(10).min(total + 2);
    let max_w = ((area.width as usize) * 9 / 10).max(30);
    let inner_h = max_h.saturating_sub(2); // minus border top+bottom
    let inner_w = max_w.saturating_sub(3); // minus border left+right + scrollbar
    let max_scroll = total.saturating_sub(inner_h);
    let scroll = app.help_scroll.min(max_scroll);
    let end = (scroll + inner_h).min(total);
    let visible = &lines_text[scroll..end];
    let h = max_h as u16;
    let w = max_w as u16;
    let y = (area.height.saturating_sub(h)) / 2;
    let x = (area.width.saturating_sub(w)) / 2;
    let popup = Rect::new(x, y, w, h);
    f.render_widget(Clear, popup);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Gray).bg(Color::Rgb(20, 20, 40)))
            .title(format!("Help {}/{}", end, total))
            .style(Style::default().bg(Color::Rgb(20, 20, 40))),
        popup,
    );
    // text content (left side, auto wrap)
    let inner = Rect::new(x + 1, y + 1, inner_w as u16, inner_h as u16);
    let help: Vec<Line> = visible
        .iter()
        .map(|l| {
            Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(Color::White),
            ))
        })
        .collect();
    f.render_widget(Paragraph::new(help).wrap(Wrap { trim: false }), inner);
    // scrollbar on right edge
    if max_scroll > 0 && inner_h > 0 {
        let sb_x = x + w - 2; // inside right border
        let sb_y = y + 1;
        let sb_h = inner_h as u16;
        // thumb position
        let thumb_pos = (scroll as f64 / max_scroll as f64 * (sb_h - 1) as f64).round() as u16;
        for row in 0..sb_h {
            let ch = if row == thumb_pos { "█" } else { "░" };
            let style = if row == thumb_pos {
                Style::default().fg(Color::Cyan).bg(Color::Rgb(20, 20, 40))
            } else {
                Style::default()
                    .fg(Color::DarkGray)
                    .bg(Color::Rgb(20, 20, 40))
            };
            f.render_widget(
                Paragraph::new(Span::styled(ch, style)),
                Rect::new(sb_x, sb_y + row, 1, 1),
            );
        }
    }
}

/// 绘制保存确认弹窗
fn draw_save_dialog(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let dw = 50u16;
    let dh = 5u16;
    let dy = (area.height.saturating_sub(dh)) / 2;
    let dx = (area.width.saturating_sub(dw)) / 2;
    let dialog = Rect::new(dx, dy, dw, dh);
    f.render_widget(Clear, dialog);
    f.render_widget(Block::default().borders(Borders::ALL), dialog);
    f.render_widget(
        Paragraph::new(Span::raw("Save changes before quitting?")),
        Rect::new(dx + 2, dy + 1, dw - 4, 1),
    );
    let focus_style = COLOR_CFG
        .get()
        .map(|c| c.sp_focused_button)
        .unwrap_or_else(|| Style::default().fg(Color::Black).bg(Color::White));
    let ys = if app.save_selected {
        focus_style
    } else {
        Style::default()
    };
    let ns = if !app.save_selected {
        focus_style
    } else {
        Style::default()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Yes ", ys),
            Span::raw("   "),
            Span::styled(" No ", ns),
        ])),
        Rect::new(dx + 2, dy + 2, dw - 4, 1),
    );
}

/// 绘制模式选择下拉菜单（从状态栏 [ASCII] 下方展开）
///
/// 包含三个显示模式（ASCII/HEX/UTF8）和颜色模式（None/256/RGB/HSL/GRAY/HEAT/hsl/rgb）。
/// 颜色模式使用背景色高亮表示选中，标签背景色匹配对应显示效果。
fn draw_mode_dropdown(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let modes = [
        (DisplayMode::Ascii, "[ASCII]"),
        (DisplayMode::Hex, "[HEX]  "),
        (DisplayMode::Utf8, "[UTF8] "),
    ];
    let dw = 10u16;
    let dh = 12u16;
    let dy = area.height.saturating_sub(1) - dh;
    let dx = 0u16;
    let dialog = Rect::new(dx, dy, dw, dh);
    f.render_widget(Clear, dialog);
    for (i, (mode, label)) in modes.iter().enumerate() {
        let sty = if app.mode == *mode {
            sp(16)
        } else {
            Style::default()
        };
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {} ", label), sty)),
            Rect::new(dx, dy + i as u16, dw, 1),
        );
    }
    // Separator row
    let line_str = "─".repeat((dw as usize).saturating_sub(2));
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {} ", line_str),
            Style::default().fg(Color::DarkGray),
        )),
        Rect::new(dx, dy + 3, dw, 1),
    );
    // Color mode radio buttons
    let none_sel = !app.is_color256
        && !app.is_rgb_bg
        && !app.is_hsl_bg
        && !app.is_gray_bg
        && !app.is_heat_bg
        && !app.is_hslbit_bg
        && !app.is_rgbbit_bg;
    let color_items: [(bool, &str, Option<Color>); 8] = [
        (none_sel, "off ", None),
        (app.is_color256, "256 ", Some(Color::Indexed(208))),
        (app.is_rgb_bg, "RGB ", Some(Color::Rgb(200, 100, 50))),
        (app.is_hsl_bg, "HSL ", Some(Color::Rgb(100, 200, 150))),
        (app.is_gray_bg, "GRAY", Some(Color::Rgb(160, 160, 160))),
        (app.is_heat_bg, "HEAT", Some(heat_bg(160))),
        (app.is_hslbit_bg, "hsl ", Some(hslbit_bg(160))),
        (app.is_rgbbit_bg, "rgb ", Some(rgbbit_bg(160))),
    ];
    for (i, (sel, label, bg)) in color_items.iter().enumerate() {
        let row_rect = Rect::new(dx, dy + 4 + i as u16, dw, 1);
        let line = if let Some(bg_color) = bg {
            let fg = if color_config::luminance(*bg_color) > 128.0 {
                Color::Black
            } else {
                Color::White
            };
            if *sel {
                // 选中：整行用该模式的背景色
                let pad = " ".repeat(dw.saturating_sub(label.len() as u16 + 4) as usize);
                Line::from(Span::styled(
                    format!("  {}{} ", label, pad),
                    Style::default().bg(*bg_color).fg(fg),
                ))
            } else {
                // 未选中：纯文本，无背景色，右移
                Line::from(Span::raw(format!("  {} ", label)))
            }
        } else {
            // off 选项（无背景色）
            if *sel {
                let pad = " ".repeat(dw.saturating_sub(label.len() as u16 + 4) as usize);
                Line::from(Span::styled(format!("  {}{} ", label, pad), sp(16)))
            } else {
                Line::from(Span::raw(format!("  {} ", label)))
            }
        };
        f.render_widget(Paragraph::new(line), row_rect);
    }
}

/// 绘制文件浏览器
///
/// 显示当前目录路径、文件列表（目录在前，文件在后）、操作提示。
/// 选中行高亮，目录用 `/` 后缀标记。
fn draw_file_browser(f: &mut ratatui::Frame, fb: &app::FileBrowser, area: Rect) {
    // 路径行
    let path_str = format!("Path: {}", fb.current_dir.display());
    f.render_widget(
        Paragraph::new(Span::styled(path_str, Style::default().fg(Color::Cyan))),
        Rect::new(0, 0, area.width, 1),
    );

    // 文件列表
    let list_area = Rect::new(0, 1, area.width, area.height.saturating_sub(2));
    let max_rows = list_area.height as usize;
    let mut spans_list: Vec<Line> = Vec::new();

    for i in 0..max_rows {
        let idx = fb.scroll_top + i;
        if idx >= fb.entries.len() {
            break;
        }
        let entry = &fb.entries[idx];
        let display = if entry.name == "*sample" {
            format!("  *sample (0x00-0xFF)")
        } else if entry.is_dir {
            format!("  {}/", entry.name)
        } else {
            format!("  {}", entry.name)
        };
        let sty = if idx == fb.cursor {
            Style::default().fg(Color::Black).bg(Color::White)
        } else if entry.name == "*sample" {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(ratatui::style::Modifier::ITALIC)
        } else if entry.is_dir {
            Style::default().fg(Color::Blue)
        } else {
            Style::default()
        };
        spans_list.push(Line::from(Span::styled(display, sty)));
    }
    f.render_widget(Paragraph::new(spans_list), list_area);

    // 底部提示
    let help = "↑↓:navigate Enter:open Backspace:up q:quit";
    f.render_widget(
        Paragraph::new(Span::styled(help, sp(5))),
        Rect::new(0, area.height - 1, area.width, 1),
    );
}

/// 运行文件浏览器（仅浏览器，不打开文件）
///
/// 接收外部终端，不管理终端生命周期。
/// 返回 Some(path) 表示选中了文件，None 表示用户退出。
fn run_file_browser_only(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> io::Result<Option<String>> {
    let start_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut fb = app::FileBrowser::new(start_dir);
    terminal.clear()?;

    loop {
        let area = terminal.size()?;
        let max_rows = (area.height as usize).saturating_sub(2);
        fb.ensure_visible(max_rows);

        terminal.draw(|f| {
            let area = f.area();
            draw_file_browser(f, &fb, area);
        })?;

        let evt = event::read()?;
        match evt {
            Event::Key(key) => {
                if key.kind == event::KeyEventKind::Release {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        return Ok(None);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        fb.move_up();
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        fb.move_down();
                    }
                    KeyCode::PageUp => {
                        fb.page_up(max_rows);
                    }
                    KeyCode::PageDown => {
                        fb.page_down(max_rows);
                    }
                    KeyCode::Enter => {
                        if let Some(path) = fb.enter() {
                            return Ok(Some(path.to_string_lossy().to_string()));
                        }
                    }
                    KeyCode::Backspace => {
                        if fb.current_dir.parent().is_some() {
                            let new_dir = fb
                                .current_dir
                                .parent()
                                .unwrap_or(&fb.current_dir)
                                .to_path_buf();
                            fb.current_dir = new_dir;
                            fb.refresh_entries();
                        }
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let my = mouse.row as usize;
                    if my >= 1 && my - 1 + fb.scroll_top < fb.entries.len() {
                        let idx = my - 1 + fb.scroll_top;
                        if fb.last_click_idx == Some(idx) {
                            // 再次点击同一项 → 打开
                            if let Some(path) = fb.enter() {
                                return Ok(Some(path.to_string_lossy().to_string()));
                            }
                            fb.last_click_idx = None;
                        } else {
                            fb.cursor = idx;
                            fb.last_click_idx = Some(idx);
                            fb.ensure_visible(max_rows);
                        }
                    }
                }
                MouseEventKind::ScrollUp => {
                    fb.move_up();
                    fb.move_up();
                    fb.move_up();
                }
                MouseEventKind::ScrollDown => {
                    fb.move_down();
                    fb.move_down();
                    fb.move_down();
                }
                _ => {}
            },
            _ => {}
        }
    }
}
