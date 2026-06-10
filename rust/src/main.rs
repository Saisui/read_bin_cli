mod app;
mod display;
mod search;
mod utf8;

use std::fs::OpenOptions;
use std::io;

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use memmap2::{Mmap, MmapMut};
use std::fs::File;
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};

use app::{App, DisplayMode, InputMode};

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <filename> [--dump]", args[0]);
        std::process::exit(1);
    }
    let filename = args[1].clone();
    let dump_mode = args.get(2).map(|s| s.as_str()) == Some("--dump");

    let file = if dump_mode {
        File::open(&filename)?
    } else {
        OpenOptions::new().read(true).write(true).open(&filename)?
    };
    let file_size = file.metadata()?.len() as usize;
    if file_size == 0 {
        eprintln!("Empty file");
        return Ok(());
    }

    if dump_mode {
        // Non-TUI mode: just print hex dump
        let mmap = unsafe { Mmap::map(&file)? };
        let data: &[u8] = &mmap;
        let base_name = std::path::Path::new(&filename)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| filename.clone());
        println!("{}  ({})", base_name, App::format_size(file_size));
        println!("    0 1 2 3 4 5 6 7 8 9 a b c d e f ");
        for r in 0..(data.len() + 15) / 16 {
            print!("{:04x}  ", r);
            let offset = r * 16;
            for i in 0..16 {
                if offset + i < data.len() {
                    print!("{:02x} ", data[offset + i]);
                } else {
                    print!("   ");
                }
            }
            print!(" |");
            for i in 0..16 {
                if offset + i < data.len() {
                    let b = data[offset + i];
                    if (0x20..=0x7e).contains(&b) {
                        print!("{}", b as char);
                    } else {
                        print!(".");
                    }
                }
            }
            println!("|");
        }
        return Ok(());
    }

    let mut mmap = unsafe { MmapMut::map_mut(&file)? };

    enable_raw_mode().map_err(|e| io::Error::new(e.kind(), format!("enable_raw_mode failed: {}", e)))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .map_err(|e| io::Error::new(e.kind(), format!("EnterAlternateScreen failed: {}", e)))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)
        .map_err(|e| io::Error::new(e.kind(), format!("Terminal::new failed: {}", e)))?;
    terminal.clear()?;

    let base_name = std::path::Path::new(&filename)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.clone());

    let mut app = App::new(file_size, base_name);

    let result = run(&mut terminal, &mut app, &mut mmap);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Drop mmap before potentially flushing
    drop(mmap);

    if let Err(e) = result {
        eprintln!("Error: {}", e);
    }

    Ok(())
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    mmap: &mut MmapMut,
) -> io::Result<()> {
    loop {
        let area = terminal.size()?;
        let term_h = area.height;
        let _term_w = area.width;

        // Clamp scroll
        let total_rows = app.total_rows();
        let max_rows = app.max_data_rows(term_h);
        if app.scroll_top > total_rows.saturating_sub(max_rows) {
            app.scroll_top = total_rows.saturating_sub(max_rows);
        }

        // Build frame
        terminal.draw(|f| {
            let area = f.area();

            match app.input_mode {
                InputMode::Help => {
                    let lines = display::help_lines();
                    let h = (lines.len() + 2).min(area.height as usize - 2);
                    let w = lines.iter().map(|l| l.len()).max().unwrap_or(40) + 4;
                    let w = w.min(area.width as usize - 2);
                    let y = ((area.height as usize).saturating_sub(h)) / 2;
                    let x = ((area.width as usize).saturating_sub(w)) / 2;
                    let popup = Rect::new(x as u16, y as u16, w as u16, h as u16);
                    f.render_widget(Clear, popup);
                    let help_text: Vec<Line> = lines
                        .iter()
                        .map(|l| Line::from(Span::raw(l.to_string())))
                        .collect();
                    let block = Block::default().borders(Borders::ALL).title("Help");
                    let p = Paragraph::new(help_text).block(block);
                    f.render_widget(p, popup);
                }
                InputMode::SaveConfirm => {
                    let hex_lines = display::build_lines(app, &*mmap, area);
                    let p = Paragraph::new(hex_lines);
                    f.render_widget(p, area);

                    // Draw save dialog overlay
                    let dw = 50u16;
                    let dh = 5u16;
                    let dy = (area.height.saturating_sub(dh)) / 2;
                    let dx = (area.width.saturating_sub(dw)) / 2;
                    let dialog = Rect::new(dx, dy, dw, dh);
                    f.render_widget(Clear, dialog);
                    let block = Block::default().borders(Borders::ALL);
                    f.render_widget(block, dialog);

                    let msg = "Save changes before quitting?";
                    f.render_widget(
                        Paragraph::new(Span::raw(msg)),
                        Rect::new(dx + 2, dy + 1, dw - 4, 1),
                    );

                    let yes_style = if app.save_selected {
                        Style::default().fg(Color::Black).bg(Color::White)
                    } else {
                        Style::default()
                    };
                    let no_style = if !app.save_selected {
                        Style::default().fg(Color::Black).bg(Color::White)
                    } else {
                        Style::default()
                    };
                    let btns = Line::from(vec![
                        Span::styled(" Yes ", yes_style),
                        Span::raw("   "),
                        Span::styled(" No ", no_style),
                    ]);
                    f.render_widget(
                        Paragraph::new(btns),
                        Rect::new(dx + 2, dy + 2, dw - 4, 1),
                    );
                }
                _ => {
                    // Normal / Edit / SearchInput etc
                    let data_start = app.current_pack * app.pack_size;
                    let data_end = (data_start + app.pack_size).min(app.file_size);
                    let data = &mmap[data_start..data_end];

                    let hex_lines = display::build_lines(app, data, area);
                    let p = Paragraph::new(hex_lines);
                    f.render_widget(p, area);

                    // Status bar
                    let status = display::build_status(app);
                    let status_y = area.height.saturating_sub(1);
                    let status_style = Style::default().fg(Color::White);
                    f.render_widget(
                        Paragraph::new(Span::styled(status, status_style)),
                        Rect::new(0, status_y, area.width, 1),
                    );

                    // Input line
                    if matches!(
                        app.input_mode,
                        InputMode::SearchInput | InputMode::StringSearchInput | InputMode::GotoInput
                    ) {
                        let input_y = area.height.saturating_sub(1);
                        let input_text = format!("{} {}", app.input_prompt, app.input_buf);
                        f.render_widget(
                            Paragraph::new(Span::raw(input_text)),
                            Rect::new(0, input_y, area.width, 1),
                        );
                    }
                }
            }
        })?;

        // Handle input
        if let Event::Key(key) = event::read()? {
            match app.input_mode {
                InputMode::Help => {
                    app.input_mode = InputMode::Normal;
                }
                InputMode::SaveConfirm => {
                    match key.code {
                        KeyCode::Left | KeyCode::Char('h') => app.save_selected = true,
                        KeyCode::Right | KeyCode::Char('l') => app.save_selected = false,
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            app.save_selected = true;
                            break;
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') => {
                            app.save_selected = false;
                            break;
                        }
                        KeyCode::Enter | KeyCode::Char(' ') => break,
                        KeyCode::Esc => {
                            app.save_selected = false;
                            break;
                        }
                        _ => {}
                    }
                }
                InputMode::SearchInput | InputMode::StringSearchInput | InputMode::GotoInput => {
                    match key.code {
                        KeyCode::Esc => {
                            app.input_mode = InputMode::Normal;
                            app.input_buf.clear();
                        }
                        KeyCode::Enter => {
                            let buf = app.input_buf.clone();
                            let _prompt = app.input_prompt.clone();
                            let mode = app.input_mode;
                            app.input_mode = InputMode::Normal;
                            app.input_buf.clear();

                            if mode == InputMode::GotoInput {
                                if let Some(val) = parse_hex_input(&buf) {
                                    let target = val.saturating_sub(1);
                                    if target < app.total_packs {
                                        if app.search_active {
                                            let offset = target * app.pack_size;
                                            if let Some(ref mut search) = app.search {
                                                if let Some(idx) =
                                                    search.find_next_match_after_offset(&*mmap, offset)
                                                {
                                                    app.jump_to_global_match(idx);
                                                }
                                            }
                                        } else {
                                            app.current_pack = target;
                                            app.scroll_top = 0;
                                        }
                                    }
                                }
                            } else if mode == InputMode::StringSearchInput {
                                if let Some((label, bytes)) = search::parse_string_search(&buf) {
                                    app.do_search(
                                        &*mmap,
                                        false,
                                        bytes,
                                        None,
                                        label,
                                        term_h,
                                    );
                                }
                            } else {
                                // SearchInput
                                if let Some((label, is_regex, bytes, regex)) =
                                    search::parse_search_input(&buf)
                                {
                                    app.do_search(
                                        &*mmap,
                                        is_regex,
                                        bytes,
                                        regex,
                                        label,
                                        term_h,
                                    );
                                }
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
                InputMode::Edit => {
                    match key.code {
                        KeyCode::Esc => {
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Left | KeyCode::Char('h') => {
                            if app.mode == DisplayMode::Ascii || app.mode == DisplayMode::Utf8 {
                                if app.cursor_byte > 0 {
                                    app.cursor_byte -= 1;
                                }
                            } else if app.cursor_nibble == 0 {
                                if app.cursor_byte > 0 {
                                    app.cursor_byte -= 1;
                                    app.cursor_nibble = 1;
                                }
                            } else {
                                app.cursor_nibble = 0;
                            }
                            app.ensure_cursor_visible(term_h);
                        }
                        KeyCode::Right | KeyCode::Char('l') => {
                            if app.mode == DisplayMode::Ascii || app.mode == DisplayMode::Utf8 {
                                if app.cursor_byte + 1 < app.file_size {
                                    app.cursor_byte += 1;
                                }
                            } else if app.cursor_nibble == 0 {
                                app.cursor_nibble = 1;
                            } else {
                                if app.cursor_byte + 1 < app.file_size {
                                    app.cursor_byte += 1;
                                    app.cursor_nibble = 0;
                                }
                            }
                            app.ensure_cursor_visible(term_h);
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            let new = app.cursor_byte.saturating_sub(16);
                            app.cursor_byte = new;
                            app.ensure_cursor_visible(term_h);
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            let new = app.cursor_byte + 16;
                            if new < app.file_size {
                                app.cursor_byte = new;
                            }
                            app.ensure_cursor_visible(term_h);
                        }
                        KeyCode::Enter => {
                            if app.mode == DisplayMode::Ascii {
                                app.edit_ascii_input(&mut *mmap, '\n');
                            }
                            app.ensure_cursor_visible(term_h);
                        }
                        KeyCode::Tab => {
                            if app.mode == DisplayMode::Ascii {
                                app.edit_ascii_input(&mut *mmap, '\t');
                            }
                            app.ensure_cursor_visible(term_h);
                        }
                        KeyCode::Char(c) => {
                            if app.mode == DisplayMode::Hex {
                                if c.is_ascii_hexdigit() {
                                    app.edit_hex_input(&mut *mmap, c);
                                }
                            } else {
                                app.edit_ascii_input(&mut *mmap, c);
                            }
                            app.ensure_cursor_visible(term_h);
                        }
                        _ => {}
                    }
                }
                InputMode::Normal => {
                    // ESC: clear search
                    if key.code == KeyCode::Esc {
                        if app.search_active {
                            app.clear_search();
                        }
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') => {
                            if app.dirty {
                                app.input_mode = InputMode::SaveConfirm;
                                app.save_selected = true;
                            } else {
                                break;
                            }
                        }
                        KeyCode::Char('?') => {
                            app.input_mode = InputMode::Help;
                        }
                        KeyCode::Char('m') => {
                            app.mode = app.mode.next();
                        }
                        KeyCode::Char('i') => {
                            app.input_mode = InputMode::Edit;
                            if app.cursor_byte == 0 && !app.dirty {
                                app.cursor_byte =
                                    app.current_pack * app.pack_size + app.scroll_top * 16;
                            }
                            app.ensure_cursor_visible(term_h);
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
                                navigate_pack_match(app, -1);
                            } else {
                                app.scroll_top = app.scroll_top.saturating_sub(1);
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if app.search_active {
                                navigate_pack_match(app, 1);
                            } else {
                                let total = app.total_rows();
                                if app.scroll_top + max_rows < total {
                                    app.scroll_top += 1;
                                }
                            }
                        }
                        KeyCode::Left | KeyCode::Char('h') => {
                            if app.search_active {
                                if !app.jump_to_prev_global_match() {
                                    // show message via status
                                }
                            } else if app.current_pack > 0 {
                                app.current_pack -= 1;
                                app.scroll_top = 0;
                            }
                        }
                        KeyCode::Right | KeyCode::Char('l') => {
                            if app.search_active {
                                if !app.jump_to_next_global_match(&*mmap) {
                                    // show message via status
                                }
                            } else if app.current_pack + 1 < app.total_packs {
                                app.current_pack += 1;
                                app.scroll_top = 0;
                            }
                        }
                        KeyCode::Char('K') => {
                            app.scroll_top = app.scroll_top.saturating_sub(max_rows);
                        }
                        KeyCode::Char('J') => {
                            let total = app.total_rows();
                            app.scroll_top = (app.scroll_top + max_rows).min(total.saturating_sub(max_rows));
                        }
                        KeyCode::Char('H') => {
                            let target = app.current_pack.saturating_sub(16);
                            if app.search_active {
                                let offset = target * app.pack_size;
                                if let Some(ref mut search) = app.search {
                                    if let Some(idx) =
                                        search.find_next_match_after_offset(&*mmap, offset)
                                    {
                                        app.jump_to_global_match(idx);
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
                                let offset = target * app.pack_size;
                                if let Some(ref mut search) = app.search {
                                    if let Some(idx) =
                                        search.find_next_match_after_offset(&*mmap, offset)
                                    {
                                        app.jump_to_global_match(idx);
                                    }
                                }
                            } else {
                                app.current_pack = target;
                                app.scroll_top = 0;
                            }
                        }
                        KeyCode::PageUp => {
                            let step = max_rows / 2;
                            app.scroll_top = app.scroll_top.saturating_sub(step.max(1));
                        }
                        KeyCode::PageDown => {
                            let step = max_rows / 2;
                            let total = app.total_rows();
                            app.scroll_top =
                                (app.scroll_top + step.max(1)).min(total.saturating_sub(max_rows));
                        }
                        KeyCode::Home => {
                            if app.search_active {
                                if let Some(ref mut search) = app.search {
                                    if let Some(idx) =
                                        search.find_next_match_after_offset(&*mmap, 0)
                                    {
                                        app.jump_to_global_match(idx);
                                    }
                                }
                            } else {
                                app.current_pack = 0;
                                app.scroll_top = 0;
                            }
                        }
                        KeyCode::Char('O') | KeyCode::Char('o') => {
                            if app.search_active {
                                let current_offset =
                                    app.current_pack * app.pack_size + app.scroll_top * 16;
                                let new_min = current_offset.saturating_sub(search::FIND_CHUNK_SIZE);
                                if let Some(ref mut search) = app.search {
                                    if let Some(idx) =
                                        search.find_next_match_after_offset(&*mmap, new_min)
                                    {
                                        if idx != app.global_match_idx.unwrap_or(usize::MAX)
                                            || new_min <= search.match_ranges[idx].0
                                        {
                                            app.jump_to_global_match(idx);
                                        }
                                    }
                                }
                            } else {
                                app.current_pack = app.current_pack.saturating_sub(256);
                                app.scroll_top = 0;
                            }
                        }
                        KeyCode::Char('P') | KeyCode::Char('p') => {
                            if app.search_active {
                                let current_offset =
                                    app.current_pack * app.pack_size + app.scroll_top * 16;
                                let new_min = current_offset + search::FIND_CHUNK_SIZE;
                                if let Some(ref mut search) = app.search {
                                    if let Some(idx) =
                                        search.find_next_match_after_offset(&*mmap, new_min)
                                    {
                                        app.jump_to_global_match(idx);
                                    }
                                }
                            } else {
                                app.current_pack = (app.current_pack + 256).min(app.total_packs - 1);
                                app.scroll_top = 0;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Handle save on quit
    if app.save_selected && app.dirty {
        mmap.flush()?;
    }

    Ok(())
}

fn navigate_pack_match(app: &mut App, delta: i32) {
    if app.pack_ranges.is_empty() {
        return;
    }
    let len = app.pack_ranges.len();
    let cur = app.pack_match_idx.unwrap_or(0);
    let new_idx = if delta > 0 {
        (cur + 1) % len
    } else {
        (cur + len - 1) % len
    };
    app.pack_match_idx = Some(new_idx);
    let (match_start, _) = app.pack_ranges[new_idx];
    if let Some(ref search) = app.search {
        if let Some(gidx) = search.get_match_index_for_offset(match_start) {
            app.global_match_idx = Some(gidx);
        }
    }
    let offset_in_pack = match_start % app.pack_size;
    let row = offset_in_pack / 16;
    let total = app.total_rows();
    let max_rows = 25; // approximate
    app.scroll_top = row
        .saturating_sub(max_rows / 2)
        .min(total.saturating_sub(max_rows));
}

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
