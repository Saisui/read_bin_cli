use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};

use crate::app::{App, DisplayMode, InputMode};
use crate::utf8;

fn style_pair(n: u8) -> Style {
    match n {
        1 => Style::default().fg(Color::Red),
        2 => Style::default().fg(Color::Black).bg(Color::Blue),
        3 => Style::default().fg(Color::Black).bg(Color::Yellow),
        4 => Style::default().fg(Color::Black).bg(Color::Red),
        5 => Style::default().fg(Color::White),
        6 => Style::default().fg(Color::Black).bg(Color::Cyan),
        7 => Style::default().fg(Color::Black).bg(Color::Blue),
        8 => Style::default().fg(Color::Black).bg(Color::Magenta),
        10 => Style::default().fg(Color::Black).bg(Color::Green),
        12 => Style::default().fg(Color::Black).bg(Color::Yellow),
        13 => Style::default().fg(Color::Red).bg(Color::Yellow),
        14 => Style::default().fg(Color::White).bg(Color::Red),
        _ => Style::default(),
    }
}

fn utf8_class_style(cls: utf8::ByteClass) -> Style {
    match cls {
        utf8::ByteClass::Ascii => style_pair(5),
        utf8::ByteClass::Duo => style_pair(2),
        utf8::ByteClass::Trio => style_pair(6),
        utf8::ByteClass::Quo => style_pair(8),
        utf8::ByteClass::Tail => style_pair(3),
        utf8::ByteClass::Invalid => style_pair(14),
    }
}

fn gradient_color(index: usize, total: usize) -> Color {
    let t = if total > 1 {
        index as f64 / (total - 1) as f64
    } else {
        0.0
    };
    let r = ((400.0 + 0.0 * t).min(1000.0) * 255.0 / 1000.0) as u8;
    let g = ((400.0 + 600.0 * t).min(1000.0) * 255.0 / 1000.0) as u8;
    let b = ((1000.0 - 0.0 * t).max(0.0) * 255.0 / 1000.0) as u8;
    Color::Rgb(r, g, b)
}

fn byte_display(b: u8, mode: DisplayMode) -> String {
    match mode {
        DisplayMode::Ascii => {
            if b == 0 {
                ". ".into()
            } else if b == 0x0d {
                "\\r".into()
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
        DisplayMode::Hex | DisplayMode::Utf8 => format!("{:02x}", b),
    }
}

fn byte_normal_style(b: u8, mode: DisplayMode) -> Style {
    match mode {
        DisplayMode::Ascii => {
            if b == 0 {
                style_pair(1)
            } else if b == 0x0d {
                style_pair(2)
            } else if b == 10 || b == 0x20 {
                style_pair(5)
            } else if b == 0x1b || (0x01..=0x1f).contains(&b) {
                style_pair(4)
            } else if (0x21..=0x7e).contains(&b) {
                style_pair(5)
            } else if (0x80..=0xbf).contains(&b) {
                style_pair(6)
            } else {
                style_pair(8)
            }
        }
        DisplayMode::Hex => {
            if (0x20..=0x7e).contains(&b) {
                style_pair(10)
            } else if b == 0 {
                style_pair(1)
            } else if b == 0x0d {
                style_pair(2)
            } else if b == 10 {
                style_pair(5)
            } else if b == 0x1b || (0x01..=0x1f).contains(&b) {
                style_pair(4)
            } else if (0x80..=0xbf).contains(&b) {
                style_pair(6)
            } else {
                style_pair(8)
            }
        }
        DisplayMode::Utf8 => style_pair(5),
    }
}

fn resolve_style(
    app: &App,
    global_off: usize,
    base_style: Style,
    match_range: Option<(usize, usize)>,
) -> Style {
    if app.input_mode == InputMode::Edit && app.cursor_byte == global_off {
        return style_pair(14);
    }
    if let Some((ms, me)) = match_range {
        if ms <= global_off && global_off < me {
            return style_pair(13);
        }
    }
    if app.search_active && app.pack_set.contains(&global_off) {
        return style_pair(12);
    }
    base_style
}

pub fn build_lines(app: &App, data: &[u8], area: Rect) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Line 0: filename + size
    let size_str = App::format_size(app.file_size);
    lines.push(Line::from(Span::raw(format!(
        "{}  ({})",
        app.filename, size_str
    ))));

    // Line 1: pack info + mode
    let pack_str = format!("{:x} / {:x}", app.current_pack + 1, app.total_packs);
    let mut mode_str = app.mode.label().to_string();
    if app.input_mode == InputMode::Edit {
        mode_str.push_str(" [EDIT]");
    }
    lines.push(Line::from(Span::raw(format!(
        "pack: {}  {}",
        pack_str, mode_str
    ))));

    // Line 2: header (gradient)
    let header_text = "    0 1 2 3 4 5 6 7 8 9 a b c d e f ";
    let leading = 4;
    let grad_len = header_text.len().saturating_sub(leading);
    let mut header_spans: Vec<Span<'static>> = Vec::new();
    for (i, ch) in header_text.chars().enumerate() {
        let col = if i < leading {
            Color::White
        } else {
            gradient_color(i - leading, grad_len)
        };
        header_spans.push(Span::styled(ch.to_string(), Style::default().fg(col)));
    }
    lines.push(Line::from(header_spans));

    // Data rows
    let total_rows = app.total_rows();
    let max_rows = app.max_data_rows(area.height);
    let start_row = app.scroll_top;
    let end_row = (start_row + max_rows).min(total_rows);
    let match_range = app.current_match_range();
    let base_off = app.current_pack * app.pack_size;

    for r in start_row..end_row {
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::raw(format!("{:02x}  ", r)));

        let offset = r * 16;
        let remaining = 16.min(data.len().saturating_sub(offset));

        if app.mode == DisplayMode::Utf8 {
            let segments = utf8::decode_row(data, offset, remaining);
            for seg in &segments {
                match seg {
                    utf8::Utf8Segment::Char { pos, ch, len } => {
                        let byte_off = offset + pos;
                        let global_off = base_off + byte_off;
                        let head_byte = data[byte_off];
                        let cls = utf8::byte_class(head_byte);
                        let base = utf8_class_style(cls);
                        let style = resolve_style(app, global_off, base, match_range);

                        let dw = utf8::display_width(*ch);
                        let display_ch = match *ch {
                            '\n' => "⏎".to_string(),
                            '\r' => "↵".to_string(),
                            '\t' => "⇥".to_string(),
                            _ => {
                                let s: String = ch.to_string();
                                if dw == 1 {
                                    format!("{} ", s)
                                } else {
                                    s
                                }
                            }
                        };
                        spans.push(Span::styled(display_ch, style));

                        for ci in 1..*len {
                            let cb_global = base_off + offset + pos + ci;
                            let tail_style =
                                resolve_style(app, cb_global, style_pair(3), match_range);
                            spans.push(Span::styled("··".to_string(), tail_style));
                        }
                    }
                    utf8::Utf8Segment::Invalid { pos } => {
                        let byte_off = offset + pos;
                        let global_off = base_off + byte_off;
                        let b = data[byte_off];
                        let cls = utf8::byte_class(b);
                        let base = utf8_class_style(cls);
                        let style = resolve_style(app, global_off, base, match_range);
                        spans.push(Span::styled(format!("{:02x}", b), style));
                    }
                }
            }
        } else {
            for i in 0..16 {
                if offset + i >= data.len() {
                    spans.push(Span::raw("  ".to_string()));
                    continue;
                }
                let b = data[offset + i];
                let global_off = base_off + offset + i;

                if app.input_mode == InputMode::Edit && app.cursor_byte == global_off {
                    let disp = byte_display(b, app.mode);
                    match (app.mode, app.cursor_nibble) {
                        (DisplayMode::Hex, 0) => {
                            let c0: String = disp.chars().take(1).collect();
                            let c1: String = disp.chars().skip(1).take(1).collect();
                            spans.push(Span::styled(c0, style_pair(14)));
                            spans.push(Span::styled(c1, style_pair(5)));
                        }
                        (DisplayMode::Hex, 1) => {
                            let c0: String = disp.chars().take(1).collect();
                            let c1: String = disp.chars().skip(1).take(1).collect();
                            spans.push(Span::styled(c0, style_pair(5)));
                            spans.push(Span::styled(c1, style_pair(14)));
                        }
                        _ => {
                            spans.push(Span::styled(disp, style_pair(14)));
                        }
                    }
                    continue;
                }

                let base = byte_normal_style(b, app.mode);
                let style = resolve_style(app, global_off, base, match_range);
                spans.push(Span::styled(byte_display(b, app.mode), style));
            }
        }

        lines.push(Line::from(spans));
    }

    lines
}

pub fn build_status(app: &App) -> String {
    match app.input_mode {
        InputMode::Edit => {
            if app.mode == DisplayMode::Ascii {
                "[EDIT ASCII] Move: arrows, type char/Enter/Tab to edit, ESC to exit".into()
            } else if app.mode == DisplayMode::Utf8 {
                "[EDIT UTF8] Move: arrows, 0-9a-f to edit nibble, ESC to exit".into()
            } else {
                "[EDIT HEX] Move: arrows, 0-9a-f to edit nibble, ESC to exit".into()
            }
        }
        InputMode::SearchInput
        | InputMode::StringSearchInput
        | InputMode::GotoInput => {
            format!("{} {}", app.input_prompt, app.input_buf)
        }
        InputMode::SaveConfirm => {
            "Save changes before quitting? [Yes] [No]".into()
        }
        InputMode::Help => "Press any key to close".into(),
        InputMode::Normal => {
            if app.search_active {
                if let Some(ref search) = app.search {
                    let total = search.match_ranges.len();
                    let plus = if search.has_more() { "+" } else { "" };
                    let cur = app.global_match_idx.map_or(0, |i| i + 1);
                    let mut display = search.user_pattern.clone();
                    if display.len() > 24 {
                        display.truncate(24);
                        display.push_str("...");
                    }
                    return format!(
                        "Search: {} [{}/{}{}]  ↑↓: in-pack | ←→: global | ESC clear",
                        display, cur, total, plus
                    );
                }
            }
            let mut s = "hjkl/←→↑↓: move | H/L: ±16 packs | J/K: ±1 screen | PGUP/PGDN: scroll half | O/P: ±1MB | HOME: first | g: goto pack | f: search | F: str | i: edit | m: mode | ?: help | q: quit".to_string();
            if app.dirty {
                s = format!("[MODIFIED] {}", s);
            }
            s
        }
    }
}

pub fn help_lines() -> Vec<&'static str> {
    vec![
        "=== READ_BIN HELP ===",
        "",
        "Navigation (non-search mode):",
        "  hjkl / ←→↑↓            Move cursor / scroll",
        "  H / L                   Jump ±16 packs",
        "  J / K                   Scroll one screen",
        "  PGUP / PGDN             Scroll half screen",
        "  HOME                    Go to first pack",
        "  g                       Go to pack (hex input)",
        "",
        "Search mode (after pressing f/F):",
        "  ↑ / ↓ (or j/k)          Navigate matches within current pack",
        "  ← / → (or h/l)          Jump to global next/prev match",
        "  O / P                   Jump ±1MB block",
        "  H / L                   Jump ±16 packs",
        "  HOME                    Jump to first match in file",
        "  ESC                     Clear search highlight",
        "",
        "Search input:",
        "  f                       Search hex / regex / advanced hex (x/z)",
        "  F                       Search plain UTF-8 string",
        "",
        "Edit mode (press i):",
        "  ESC                     Exit edit mode",
        "  ←→↑↓                    Move cursor",
        "  0-9a-fA-F               Edit nibble (hex mode)",
        "  Enter                   Insert newline (\\n)",
        "  Tab                     Insert tab (\\t)",
        "  any character           Insert byte (ASCII mode)",
        "",
        "Other:",
        "  m                       Toggle display mode (ASCII / HEX / UTF8)",
        "  q                       Quit (with save prompt if modified)",
        "  ?                       Show this help",
        "",
        "Press any key to close",
    ]
}
