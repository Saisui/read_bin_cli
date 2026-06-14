/// read-bin: 终端十六进制查看/编辑器
///
/// 功能：文件分页浏览、三种显示模式（ASCII/HEX/UTF8）、
/// hex/nibble/字符串搜索、字节编辑、撤销/重做、鼠标支持。
///
/// 模块结构：
/// - app: 应用状态管理
/// - color_config: 颜色配置加载
/// - search: 搜索引擎（三级位图索引）
/// - utf8: UTF-8 解码与分类
mod app;
mod color_config;
mod search;
mod utf8;

use std::fs::{File, OpenOptions};
use std::io;
use std::sync::OnceLock;

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers,
        MouseButton, MouseEventKind,
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
fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <file> [--dump]", args[0]);
        std::process::exit(1);
    }
    let filename = args[1].clone();
    let dump = args.get(2).map(|s| s.as_str()) == Some("--dump");

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
                    print!("{}", if (0x20..=0x7e).contains(&b) { b as char } else { '.' });
                }
            }
            println!("|");
        }
        return Ok(());
    }

    let mmap = unsafe { Mmap::map(&file)? };
    let mut data = mmap[..file_size].to_vec();

    // load color config (embedded defaults if color.yaml missing)
    if let Err(e) = init_colors(std::path::Path::new("color.yaml")) {
        eprintln!("color.yaml: {e}");
        return Err(io::Error::new(io::ErrorKind::Other, e));
    }

    // load terminal palette for accurate dimming
    color_config::init_terminal_palette(
        &std::path::Path::new(&std::env::var("HOME").unwrap_or_default()),
    );

    enable_raw_mode()
        .map_err(|e| io::Error::new(e.kind(), format!("enable_raw_mode: {}", e)))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).map_err(|e| {
        io::Error::new(e.kind(), format!("EnterAlternateScreen/EnableMouseCapture: {}", e))
    })?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)
        .map_err(|e| io::Error::new(e.kind(), format!("Terminal::new: {}", e)))?;
    terminal.clear()?;

    let base_name = std::path::Path::new(&filename)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.clone());
    let mut app = App::new(file_size, base_name);
    let result = run(&mut terminal, &mut app, &mut data, &filename);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Error: {}", e);
    }
    Ok(())
}

/// 主事件循环：渲染 → 处理输入 → 重复
fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    data: &mut [u8],
    filename: &str,
) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    let mut last_key_time = std::time::Instant::now();
    loop {
        let area = terminal.size()?;
        let th = area.height;
        let max_rows = app.max_rows(th);

        // drain background search results
        app.drain_search_rx();

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
                _ => {
                    draw_hex(f, app, data, area);
                    draw_status(f, app, data, area);
                }
            }
        })?;

        // handle input
        let evt = event::read()?;
        let mut should_break = false;
        match evt {
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let mx = mouse.column;
                    let my = mouse.row;
                    if app.input_mode == InputMode::Help {
                        // click outside help popup -> close
                        let (hx, hy, hw, hh) = app.help_rect.unwrap_or((0, 0, 0, 0));
                        if mx < hx || mx >= hx + hw || my < hy || my >= hy + hh {
                            app.input_mode = InputMode::Normal;
                            app.help_rect = None;
                        }
                    } else if app.input_mode == InputMode::ModeSelect {
                        let dh = 5u16;
                        let dy = area.height.saturating_sub(1) - dh;
                        let dw = 10u16;
                        if mx < dw && my >= dy && my < dy + dh {
                            let sel = my - dy;
                            match sel {
                                0 => app.mode = DisplayMode::Ascii,
                                1 => app.mode = DisplayMode::Hex,
                                2 => app.mode = DisplayMode::Utf8,
                                3 => app.is_color256 = !app.is_color256,
                                _ => {}
                            }
                            if sel <= 2 {
                                app.input_mode = InputMode::Normal;
                            }
                        } else {
                            app.input_mode = InputMode::Normal;
                        }
                    } else if my >= 1 && mx >= 4 && my < area.height.saturating_sub(1) {
                        let global_row = my as usize - 1 + app.global_scroll_top();
                        let col = mx as usize - 4;
                        let bc = col / 2;
                        if bc < 16 {
                            let off = global_row * 16 + bc;
                            if off < app.file_size {
                                app.cursor_byte = off;
                                app.cursor_focused = true;
                                app.sel_start = Some(off);
                                app.sel_end = Some(off);
                                app.dragging = true;
                                app.ensure_cursor_visible(th);
                            }
                        }
                    } else if app.input_mode != InputMode::Edit {
                        app.cursor_focused = false;
                        app.sel_start = None;
                        app.sel_end = None;
                    }
                    // status bar click
                    if my == area.height.saturating_sub(1) && app.input_mode == InputMode::Normal {
                        let dirty_len = if app.dirty { 11 } else { 0 };
                        let hex_w = if app.file_size <= 0xff { 2 }
                            else if app.file_size <= 0xffff { 4 }
                            else if app.file_size <= 0xffffff { 6 }
                            else { 8 };
                        let at_offset = (6 + dirty_len + 2) as u16;
                        let at_len = (1 + hex_w) as u16;
                        let pack_offset = at_offset + at_len + 2;
                        let pack_total_hex = format!("{:x}", app.total_packs).len();
                        let pack_len = (5 + hex_w + 1 + pack_total_hex) as u16;
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
                            app.input_mode = InputMode::Help;
                            app.help_scroll = 0;
                        }
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    if app.dragging {
                        let mx = mouse.column;
                        let my = mouse.row;
                        if my >= 1 && mx >= 4 && my < area.height.saturating_sub(1) {
                            let global_row = my as usize - 1 + app.global_scroll_top();
                            let col = mx as usize - 4;
                            let bc = col / 2;
                            if bc < 16 {
                                let off = global_row * 16 + bc;
                                if off < app.file_size {
                                    app.cursor_byte = off;
                                    app.sel_end = Some(off);
                                    app.ensure_cursor_visible(th);
                                }
                            }
                        }
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
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
                        app.scroll_top = app.scroll_top.saturating_sub(3);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if app.input_mode == InputMode::Help {
                        app.help_scroll += 1;
                    } else {
                        let tr = app.total_rows();
                        if app.scroll_top + 3 + max_rows <= tr {
                            app.scroll_top += 3;
                        }
                    }
                }
                _ => {}
            },
            Event::Key(key) => {
                if key.kind == event::KeyEventKind::Release {
                    continue;
                }
                #[cfg(target_os = "windows")]
                if last_key_time.elapsed().as_millis() < 40 {
                    continue;
                }
                #[cfg(target_os = "windows")]
                {
                    last_key_time = std::time::Instant::now();
                }
                // Ctrl shortcuts (global)
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match key.code {
                        KeyCode::Char('z') => {
                            app.undo(data);
                            continue;
                        }
                        KeyCode::Char('y') => {
                            app.redo(data);
                            continue;
                        }
                        KeyCode::Char('q') => {
                            if app.dirty {
                                app.input_mode = InputMode::SaveConfirm;
                                app.save_selected = true;
                            } else {
                                break;
                            }
                            continue;
                        }
                        KeyCode::Char('g') => {
                            app.input_mode = InputMode::GotoInput;
                            app.input_buf.clear();
                            app.input_prompt = "Go to (hex):".into();
                            continue;
                        }
                        KeyCode::Char('h') => {
                            app.input_mode = InputMode::Help;
                            app.help_scroll = 0;
                            continue;
                        }
                        KeyCode::Char('s') => {
                            let _ = std::fs::write(filename, &*data);
                            app.dirty = false;
                            continue;
                        }
                        KeyCode::Char('c') => {
                            if let (Some(start), Some(end)) = (app.sel_start, app.sel_end) {
                                let (lo, hi) = if start <= end { (start, end) } else { (end, start) };
                                let len = (hi - lo + 1).min(data.len() - lo);
                                let selected = &data[lo..lo + len];
                                let text = match app.mode {
                                    DisplayMode::Ascii => {
                                        selected.iter().map(|b| {
                                            if *b == 0 { '.' }
                                            else if *b == 0x0d { 'r' }
                                            else if *b == 10 { '\n' }
                                            else if *b == 0x1b { 'e' }
                                            else if (0x01..=0x1f).contains(b) { '.' }
                                            else if *b == 0x20 { ' ' }
                                            else if (0x21..=0x7e).contains(b) { *b as char }
                                            else { '.' }
                                        }).collect()
                                    }
                                    DisplayMode::Hex => {
                                        selected.iter().map(|b| format!("{:02x}", b))
                                            .collect::<Vec<_>>().join(" ")
                                    }
                                    DisplayMode::Utf8 => {
                                        let segs = crate::utf8::decode_row(data, lo, len, 0);
                                        segs.iter().filter_map(|seg| {
                                            if let crate::utf8::Segment::Char { ch, .. } = seg {
                                                Some(*ch)
                                            } else {
                                                None
                                            }
                                        }).collect()
                                    }
                                };
                                let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text));
                            }
                            continue;
                        }
                        KeyCode::Left => {
                            if app.input_mode == InputMode::Edit
                                && app.current_pack > 0
                            {
                                app.current_pack -= 1;
                                app.scroll_top = 0;
                                app.cursor_byte = app.current_pack * app.pack_size;
                                app.ensure_cursor_visible(th);
                            }
                            continue;
                        }
                        KeyCode::Right => {
                            if app.input_mode == InputMode::Edit
                                && app.current_pack + 1 < app.total_packs
                            {
                                app.current_pack += 1;
                                app.scroll_top = 0;
                                app.cursor_byte = app.current_pack * app.pack_size;
                                app.ensure_cursor_visible(th);
                            }
                            continue;
                        }
                        _ => {}
                    }
                }

                if key.modifiers.contains(KeyModifiers::ALT) {
                    match key.code {
                        KeyCode::Char('j') => {
                            app.sel_start = Some(app.cursor_byte);
                            continue;
                        }
                        KeyCode::Char('k') => {
                            app.sel_end = Some(app.cursor_byte);
                            continue;
                        }
                        KeyCode::Char('m') => {
                            app.mode = app.mode.next();
                            continue;
                        }
                        _ => {}
                    }
                }

                app.cursor_focused = true;

                match app.input_mode {
                InputMode::Help => match key.code {
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
                }
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
                        KeyCode::Char('1') => { app.mode = DisplayMode::Ascii; app.input_mode = InputMode::Normal; }
                        KeyCode::Char('2') => { app.mode = DisplayMode::Hex; app.input_mode = InputMode::Normal; }
                        KeyCode::Char('3') => { app.mode = DisplayMode::Utf8; app.input_mode = InputMode::Normal; }
                        KeyCode::Char('4') | KeyCode::Char('n') => { app.is_color256 = !app.is_color256; }
                        _ => {}
                    }
                    InputMode::SaveConfirm => handle_save(app, key.code, data, filename, &mut should_break),
                    InputMode::SearchInput
                    | InputMode::StringSearchInput
                    | InputMode::GotoInput
                    | InputMode::GotoByteInput => {
                        handle_input(app, key.code, data, th);
                    }
                    InputMode::Edit => handle_edit(app, key.code, data, th),
                    InputMode::Normal => {
                        handle_normal(app, key.code, data, th, max_rows, &mut should_break)
                    }
                }
                if should_break {
                    break;
                }
            }
            _ => {}
        }
    }
    Ok(())
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
                                let off = target * app.pack_size;
                                if let Some(ref mut s) = app.search {
                                    if let Some(idx) = s.find_after(data, off) {
                                        app.jump_global(idx);
                                    }
                                }
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
                                if let Some(ref mut s) = app.search {
                                    if let Some(idx) = s.find_after(data, val) {
                                        app.jump_global(idx);
                                    }
                                }
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
                        let needle = bytes.clone();
                        let acc = search::Search::new_hex(bytes, app.pack_size, app.file_size, label);
                        app.apply_search(acc, data, th);
                        app.start_bg_search(needle, data.to_vec());
                    }
                }
                InputMode::SearchInput => {
                    if let Some(kind) = search::parse_input(&buf) {
                        let (acc, needle) = match kind {
                            search::SearchKind::Hex { bytes, label } => {
                                let needle = bytes.clone();
                                (search::Search::new_hex(bytes, app.pack_size, app.file_size, label), needle)
                            }
                            search::SearchKind::Pat { pat, label } => {
                                let needle: Vec<u8> = pat.iter().flat_map(|a| match a {
                                    search::NibAtom::Exact(n) => vec![*n],
                                    _ => vec![],
                                }).collect();
                                (search::Search::new_pat(pat, app.pack_size, app.file_size, label), needle)
                            }
                        };
                        app.apply_search(acc, data, th);
                        app.start_bg_search(needle, data.to_vec());
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
            app.cursor_byte = if target < app.file_size { target } else { app.file_size - 1 };
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
        KeyCode::Char('?') => { app.input_mode = InputMode::Help; app.help_scroll = 0; }
        KeyCode::Char('m') => app.mode = app.mode.next(),
        KeyCode::Char('n') => app.is_color256 = !app.is_color256,
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
                app.prev_global();
            } else {
                let gs = app.global_scroll_top();
                if gs > 0 {
                    app.set_global_scroll(gs - 1);
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.search_active {
                app.next_global(data);
            } else {
                let gs = app.global_scroll_top();
                let max = app.global_total_rows().saturating_sub(max_rows);
                if gs < max {
                    app.set_global_scroll(gs + 1);
                }
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if app.search_active {
                app.prev_page_match();
            } else if app.current_pack > 0 {
                app.current_pack -= 1;
                app.scroll_top = 0;
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if app.search_active {
                app.next_page_match();
            } else if app.current_pack + 1 < app.total_packs {
                app.current_pack += 1;
                app.scroll_top = 0;
            }
        }
        KeyCode::Char('K') => {
            let gs = app.global_scroll_top();
            app.set_global_scroll(gs.saturating_sub(max_rows));
        }
        KeyCode::Char('J') => {
            let gs = app.global_scroll_top();
            let max = app.global_total_rows().saturating_sub(max_rows);
            app.set_global_scroll((gs + max_rows).min(max));
        }
        KeyCode::Char('H') => {
            let target = app.current_pack.saturating_sub(16);
            if app.search_active {
                let off = target * app.pack_size;
                if let Some(ref mut s) = app.search {
                    if let Some(idx) = s.find_after(data, off) {
                        app.jump_global(idx);
                    }
                }
            } else {
                app.current_pack = target;
                app.scroll_top = 0;
            }
        }
        KeyCode::Char('L') => {
            let target = (app.current_pack + 16).min(app.total_packs - 1);
            if app.search_active {
                let off = target * app.pack_size;
                if let Some(ref mut s) = app.search {
                    if let Some(idx) = s.find_after(data, off) {
                        app.jump_global(idx);
                    }
                }
            } else {
                app.current_pack = target;
                app.scroll_top = 0;
            }
        }
        KeyCode::PageUp => {
            let step = (max_rows / 2).max(1);
            let gs = app.global_scroll_top();
            app.set_global_scroll(gs.saturating_sub(step));
        }
        KeyCode::PageDown => {
            let step = (max_rows / 2).max(1);
            let gs = app.global_scroll_top();
            let max = app.global_total_rows().saturating_sub(max_rows);
            app.set_global_scroll((gs + step).min(max));
        }
        KeyCode::Home => {
            if app.search_active {
                if let Some(ref mut s) = app.search {
                    if let Some(idx) = s.find_after(data, 0) {
                        app.jump_global(idx);
                    }
                }
            } else {
                app.current_pack = 0;
                app.scroll_top = 0;
            }
        }
        KeyCode::Char('O') | KeyCode::Char('o') => {
            if app.search_active {
                let cur = app.global_scroll_top() * 16;
                let min = cur.saturating_sub(search::FIND_CHUNK);
                if let Some(ref mut s) = app.search {
                    if let Some(idx) = s.find_after(data, min) {
                        if idx != app.global_match_idx.unwrap_or(usize::MAX)
                            || min <= s.ranges[idx].0
                        {
                            app.jump_global(idx);
                        }
                    }
                }
            } else {
                let gs = app.global_scroll_top();
                let step = 256 * (app.pack_size / 16);
                app.set_global_scroll(gs.saturating_sub(step));
            }
        }
        KeyCode::Char('P') | KeyCode::Char('p') => {
            if app.search_active {
                let cur = app.global_scroll_top() * 16;
                let min = cur + search::FIND_CHUNK;
                if let Some(ref mut s) = app.search {
                    if let Some(idx) = s.find_after(data, min) {
                        app.jump_global(idx);
                    }
                }
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
    COLOR_CFG.set(cfg).map_err(|_| "color config already set".to_string())
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

/// 列号渐变色（蓝→绿，用于表头 0-F 的颜色渐变）
fn grad_color(i: usize, total: usize) -> Color {
    let t = if total > 1 { i as f64 / (total - 1) as f64 } else { 0.0 };
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
            if b == 0 { ". ".into() }
            else if b == 0x0d { "\\r".into() }
            else if b == 10 { "⏎ ".into() }
            else if b == 0x1b { "\\e".into() }
            else if (0x01..=0x1f).contains(&b) { format!("{:02x}", b) }
            else if b == 0x20 { "· ".into() }
            else if (0x21..=0x7e).contains(&b) { format!("{} ", b as char) }
            else { format!("{:02x}", b) }
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
            if b == 0 { 1 }
            else if b == 0x0d { 2 }
            else if b == 10 || b == 0x20 { 3 }
            else if b == 0x1b || (0x01..=0x1f).contains(&b) { 4 }
            else if (0x21..=0x7e).contains(&b) { 5 }
            else if (0x80..=0xbf).contains(&b) { 6 }
            else { 7 }
        }
        DisplayMode::Hex => {
            if (0x20..=0x7e).contains(&b) { 1 }
            else if b == 0 { 2 }
            else if b == 0x0d { 3 }
            else if b == 10 { 4 }
            else if b == 0x1b || (0x01..=0x1f).contains(&b) { 5 }
            else if (0x80..=0xbf).contains(&b) { 6 }
            else { 7 }
        }
        _ => 0,
    }
}

/// 字节 → 基础样式映射（根据字节值和显示模式选择对应颜色）
fn byte_style(b: u8, mode: DisplayMode) -> Style {
    match mode {
        DisplayMode::Ascii => {
            if b == 0 { sp(1) }
            else if b == 0x0d { sp(2) }
            else if b == 10 || b == 0x20 { sp(5) }
            else if b == 0x1b || (0x01..=0x1f).contains(&b) { sp(4) }
            else if (0x21..=0x7e).contains(&b) { sp(5) }
            else if (0x80..=0xbf).contains(&b) { sp(6) }
            else { sp(8) }
        }
        DisplayMode::Hex => {
            if (0x20..=0x7e).contains(&b) { sp(10) }
            else if b == 0 { sp(1) }
            else if b == 0x0d { sp(2) }
            else if b == 10 { sp(5) }
            else if b == 0x1b || (0x01..=0x1f).contains(&b) { sp(4) }
            else if (0x80..=0xbf).contains(&b) { sp(6) }
            else { sp(8) }
        }
        DisplayMode::Utf8 => sp(5),
    }
}

/// UTF-8 字符类型分组（用于交替 dim 效果）
///
/// 按 Unicode 区块分类：ASCII、控制符、CJK、韩文、假名、标点等。
fn char_type_group(ch: char) -> u8 {
    let cp = ch as u32;
    if cp >= 0x21 && cp <= 0x7e { 1 }           // Printable ASCII
    else if ch == '\n' || ch == '\r' || ch == '\t' { 2 } // Common control
    else if cp < 0x20 { 3 }                      // Other control
    else if cp >= 0x4E00 && cp <= 0x9FFF { 4 }   // CJK
    else if cp >= 0xAC00 && cp <= 0xD7AF { 5 }   // Hangul
    else if cp >= 0x3000 && cp <= 0x30FF { 6 }   // CJK symbols + kana
    else if cp >= 0x2000 && cp <= 0x206F { 7 }   // General punctuation
    else { 8 }                                    // Other Unicode
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

/// 将颜色亮度降至 50%（用于编辑模式背景压暗）
fn dim_color(c: Color) -> Color {
    let (r, g, b) = color_config::color_rgb(c);
    Color::Rgb((r as u16 * 50 / 100) as u8, (g as u16 * 50 / 100) as u8, (b as u16 * 50 / 100) as u8)
}

/// 压暗样式背景色 50%（编辑模式下非光标字节使用）
fn dim_style(s: Style) -> Style {
    let bg = s.bg.unwrap_or(Color::Rgb(30, 30, 30));
    s.bg(dim_color(bg))
}

/// 按 ColorConfig 配置压暗背景色（交替 dim 效果）
fn dim_bg_10pct(s: Style) -> Style {
    COLOR_CFG.get().map(|c| c.dim_bg(s)).unwrap_or(s)
}

const STD_COLORS: [(u8,u8,u8); 8] = [
    (0,0,0), (170,0,0), (0,170,0), (170,85,0),
    (0,0,170), (170,0,170), (0,170,170), (170,170,170),
];
const BRIGHT_COLORS: [(u8,u8,u8); 8] = [
    (85,85,85), (255,85,85), (85,255,85), (255,255,85),
    (85,85,255), (255,85,255), (85,255,255), (255,255,255),
];

fn cube_rgb(idx: u8) -> (u8,u8,u8) {
    let i = (idx - 16) as usize;
    let r = i / 36; let g = (i / 6) % 6; let b = i % 6;
    (if r==0 {0} else {55+r*40} as u8, if g==0 {0} else {55+g*40} as u8, if b==0 {0} else {55+b*40} as u8)
}

fn gray_rgb(idx: u8) -> (u8,u8,u8) {
    let v = 8 + (idx - 232) * 10;
    (v, v, v)
}

fn indexed_rgb(idx: u8) -> (u8,u8,u8) {
    match idx {
        0..=7 => STD_COLORS[idx as usize],
        8..=15 => BRIGHT_COLORS[(idx-8) as usize],
        16..=231 => cube_rgb(idx),
        _ => gray_rgb(idx),
    }
}

fn indexed_luminance(idx: u8) -> f64 {
    let (r,g,b) = indexed_rgb(idx);
    0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64
}

/// 解析 fg: auto 哨兵 → 实际前景色
///
/// 检测 AUTO_FG_SENTINEL，根据当前 bg 亮度选择 Black（亮背景）或 White（暗背景）。
/// 无 bg 时保持 fg 为 null（使用终端默认前景色）。
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
        if ms <= off && off < me { sp(13) } else { base }
    } else if app.search_active && app.pack_ranges.iter().any(|&(s, e)| s <= off && off < e) {
        sp(12)
    } else if let (Some(a), Some(b)) = (app.sel_start, app.sel_end) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if off >= lo && off <= hi {
            if off == a { sp(18) } else { sp(17) }
        } else { base }
    } else if app.input_mode == InputMode::Edit {
        dim_style(base)
    } else {
        base
    };
    resolve_auto_fg(s)
}

/// 绘制主视图（hex/ascii/utf8 内容区）
fn draw_hex(f: &mut ratatui::Frame, app: &App, data_full: &[u8], area: Rect) {
    let lines = build_lines(app, data_full, area);
    f.render_widget(Paragraph::new(lines), area);
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
        let col = if i < leading { Color::White } else { grad_color(i - leading, glen) };
        hspans.push(Span::styled(ch.to_string(), Style::default().fg(col)));
    }
    lines.push(Line::from(hspans));

    let global_total = app.global_total_rows();
    let max_rows = app.max_rows(area.height);
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
        spans.push(Span::raw(format!("{:02x}  ", row_in_pack)));

        if app.mode == DisplayMode::Utf8 {
            for t in 0..cross_row_tail {
                let p = off - cross_row_tail + t;
                let go = base_off + p;
                let tail_b = data[p];
                let ts = if app.is_color256 {
                    let fg = if indexed_luminance(tail_b) > 128.0 { Color::Black } else { Color::White };
                    resolve(app, go, Style::default().bg(Color::Indexed(tail_b)).fg(fg), mr)
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
                            '\r' => "\\r".into(),
                            '\x1b' => "\\e".into(),
                            '\t' => "⇥ ".into(),
                            c if (c as u32) < 0x20 => format!("{:02x}", c as u8),
                            _ => {
                                let s: String = ch.to_string();
                                if dw == 1 { format!("{} ", s) } else { s }
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
                        let base = if app.is_color256 {
                            let fg = if indexed_luminance(*ch as u8) > 128.0 { Color::Black } else { Color::White };
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
                            let ts = if app.is_color256 {
                                let fg = if indexed_luminance(tail_b) > 128.0 { Color::Black } else { Color::White };
                                resolve(app, cgo, Style::default().bg(Color::Indexed(tail_b)).fg(fg), mr)
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
                        let base = if app.is_color256 {
                            let fg = if indexed_luminance(b) > 128.0 { Color::Black } else { Color::White };
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
                    let non_cursor_style = if app.is_color256 {
                        let fg = if indexed_luminance(b) > 128.0 { Color::Black } else { Color::White };
                        Style::default().bg(Color::Indexed(b)).fg(fg)
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
                let base = if app.is_color256 {
                    let fg = if indexed_luminance(b) > 128.0 { Color::Black } else { Color::White };
                    Style::default().bg(Color::Indexed(b)).fg(fg)
                } else {
                    byte_style(b, app.mode)
                };
                let sty = resolve(app, go, base, mr);
                let final_sty = if app.is_color256 { sty } else if dim { dim_bg_10pct(sty) } else { sty };
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
/// - @address → 跳转到字节地址
/// - pack → 跳转到指定页
/// - Ctrl+H:help → 打开帮助窗口
fn draw_status(f: &mut ratatui::Frame, app: &App, data: &[u8], area: Rect) {
    f.render_widget(Clear, Rect::new(0, area.height - 1, area.width, 1));
    let text = match app.input_mode {
        InputMode::Edit => {
            let byte_info = if app.mode != DisplayMode::Hex && app.cursor_byte < app.file_size {
                format!(" [{:02X}]", data[app.cursor_byte])
            } else {
                String::new()
            };
            return f.render_widget(
                Paragraph::new(Span::raw(format!(
                    "{}{}",
                    match app.mode {
                        DisplayMode::Ascii => "[EDIT ASCII]",
                        DisplayMode::Utf8 => "[EDIT UTF8]",
                        DisplayMode::Hex => "[EDIT HEX]",
                    },
                    byte_info
                ))),
                Rect::new(0, area.height - 1, area.width, 1),
            );
        }
        InputMode::SearchInput | InputMode::StringSearchInput | InputMode::GotoInput | InputMode::GotoByteInput => {
            return f.render_widget(
                Paragraph::new(Span::raw(format!(
                    "{} {}",
                    app.input_prompt, app.input_buf
                ))),
                Rect::new(0, area.height - 1, area.width, 1),
            );
        }
        InputMode::SaveConfirm => "Save changes? [Yes] [No]",
        InputMode::Help => "Press any key to close",
        InputMode::Normal => {
            if app.search_active {
                if let Some(ref s) = app.search {
                    let total = s.ranges.len();
                    let plus = if s.has_more() { "+" } else { "" };
                    let cur = app.global_match_idx.map_or(0, |i| i + 1);
                    let mut disp = s.label.clone();
                    if disp.len() > 24 {
                        disp.truncate(24);
                        disp.push_str("...");
                    }
                    let status = format!(
                        "Search: {} [{}/{}{}]  ↑↓:in-pack ←→:global ESC:clear",
                        disp, cur, total, plus
                    );
                    return f.render_widget(
                        Paragraph::new(Span::styled(status, sp(5))),
                        Rect::new(0, area.height - 1, area.width, 1),
                    );
                }
            }
            let dirty = if app.dirty { " [MODIFIED]" } else { "" };
            // hex width based on file size
            let hex_w = if app.file_size <= 0xff { 2 }
                else if app.file_size <= 0xffff { 4 }
                else if app.file_size <= 0xffffff { 6 }
                else { 8 };
            let offset_str = format!("@{:0width$x}", app.cursor_byte, width = hex_w);
            let max_rows = app.max_rows(area.height);
            let last_global_row = (app.global_scroll_top() + max_rows - 1).min(app.global_total_rows().saturating_sub(1));
            let last_pack = last_global_row / (app.pack_size / 16);
            let pack_str = format!("pack {:x}/{:x}", last_pack + 1, app.total_packs);
            let help_str = "  Ctrl+H:help";

            let mode_label = app.mode.label();
            let mut spans = if app.is_color256 {
                let grad = [
                    Color::Rgb(100, 149, 237),
                    Color::Rgb(123, 137, 231),
                    Color::Rgb(147, 125, 225),
                    Color::Rgb(171, 113, 219),
                    Color::Rgb(195, 101, 213),
                    Color::Rgb(219, 89, 207),
                    Color::Rgb(219, 112, 147),
                ];
                mode_label.chars().enumerate().map(|(i, c)| {
                    Span::styled(c.to_string(), Style::default().fg(Color::White).bg(grad[i]))
                }).collect::<Vec<_>>()
            } else {
                vec![Span::styled(mode_label, sp(5))]
            };
            spans.push(Span::styled(dirty, sp(5)));
            spans.push(Span::styled(format!("  {}  {}{}", offset_str, pack_str, help_str), sp(5)));
            return f.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(0, area.height - 1, area.width, 1),
            );
        }
        InputMode::ModeSelect => {
            return f.render_widget(
                Paragraph::new(Span::styled("↑↓:select Enter:confirm Esc:cancel 1/2/3:mode 4:256", sp(5))),
                Rect::new(0, area.height - 1, area.width, 1),
            );
        }
    };
    f.render_widget(
        Paragraph::new(Span::styled(text, sp(5))),
        Rect::new(0, area.height - 1, area.width, 1),
    );
}

/// 绘制帮助弹窗（自适应大小，带滚动条）
fn draw_help(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let ver = env!("CARGO_PKG_VERSION");
    let lines_text = [
        &*format!("=== read-bin v{} by Saisui ===", ver),
        "",
        "Navigation:",
        "  ↑↓ / jk    Scroll one line",
        "  ←→ / hl    Prev/Next pack",
        "  J / K       Scroll one screen",
        "  H / L       Jump ±16 packs",
        "  PGUP/PGDN   Scroll half screen",
        "  HOME        First pack",
        "  O / P       ±1MB area",
        "  g/Ctrl+G    Go to offset (hex)",
        "",
        "Search:",
        "  f           Hex / nibble pattern search",
        "  F           Plain string search",
        "  ↑↓ / ←→    Navigate matches",
        "  ESC         Clear search",
        "",
        "  Search syntax:",
        "    4f2a        Exact hex bytes",
        "    4x          Hi nibble=4, lo any (x=any nibble)",
        "    [0-3]f      Hi nibble in 0-3, lo=f",
        "    [A-F][0-3]  Both nibbles in range",
        "    z            Any single byte (z = xx)",
        "",
        "Edit:",
        "  i           Enter edit mode",
        "  ESC         Exit edit mode",
        "  ←→↑↓       Move cursor",
        "  PGUP/PGDN   Scroll page",
        "  0-9a-f      Edit nibble (hex)",
        "  any char    Edit byte (ascii/utf8)",
        "",
        "Selection:",
        "  Alt+J       Selection start (bright)",
        "  Alt+K       Selection end",
        "",
        "Other:",
        "  m / Alt+M   Toggle mode (ASCII/HEX/UTF8)",
        "  n           Toggle 256-color background",
        "  Ctrl+H / ?  This help",
        "  Ctrl+Z / Y  Undo / Redo",
        "  Ctrl+Q / q  Quit",
        "  Mouse click Move cursor",
        "  Scroll wheel Scroll page",
    ];
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
        .map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(Color::White))))
        .collect();
    f.render_widget(
        Paragraph::new(help).wrap(Wrap { trim: false }),
        inner,
    );
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
                Style::default().fg(Color::DarkGray).bg(Color::Rgb(20, 20, 40))
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
    let focus_style = COLOR_CFG.get()
        .map(|c| c.sp_focused_button)
        .unwrap_or_else(|| Style::default().fg(Color::Black).bg(Color::White));
    let ys = if app.save_selected { focus_style } else { Style::default() };
    let ns = if !app.save_selected { focus_style } else { Style::default() };
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
/// 包含三个模式选项（ASCII/HEX/UTF8）和一个 256 色勾选框。
/// 点击选项切换模式，点击 256 切换勾选状态。
fn draw_mode_dropdown(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let modes = [
        (DisplayMode::Ascii, "[ASCII]"),
        (DisplayMode::Hex, "[HEX]  "),
        (DisplayMode::Utf8, "[UTF8] "),
    ];
    let dw = 10u16;
    let dh = 5u16;
    let dy = area.height.saturating_sub(1) - dh;
    let dx = 0u16;
    let dialog = Rect::new(dx, dy, dw, dh);
    f.render_widget(Clear, dialog);
    for (i, (mode, label)) in modes.iter().enumerate() {
        let sty = if app.mode == *mode { sp(16) } else { Style::default() };
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {} ", label), sty)),
            Rect::new(dx, dy + i as u16, dw, 1),
        );
    }
    let checkbox = if app.is_color256 { " [x] 256 " } else { " [ ] 256 " };
    let cb_style = if app.is_color256 {
        Style::default().fg(Color::White).bg(Color::Rgb(147, 112, 219))
    } else {
        Style::default()
    };
    f.render_widget(
        Paragraph::new(Span::styled(checkbox, cb_style)),
        Rect::new(dx, dy + 3, dw, 1),
    );
}
