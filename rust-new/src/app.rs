use std::sync::mpsc;
use crate::search::{Search, SearchEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode { Ascii, Hex, Utf8 }

impl DisplayMode {
    pub fn next(self) -> Self {
        match self {
            Self::Ascii => Self::Hex,
            Self::Hex => Self::Utf8,
            Self::Utf8 => Self::Ascii,
        }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Ascii => "[ASCII]", Self::Hex => "[HEX]", Self::Utf8 => "[UTF8]" }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Edit,
    SearchInput,
    GotoInput,
    StringSearchInput,
    SaveConfirm,
    Help,
}

pub struct UndoEntry {
    pub offset: usize,
    pub old: u8,
    pub new: u8,
}

pub struct App {
    pub file_size: usize,
    pub pack_size: usize,
    pub total_packs: usize,
    pub current_pack: usize,
    pub scroll_top: usize,
    pub mode: DisplayMode,
    pub input_mode: InputMode,
    pub filename: String,
    pub cursor_byte: usize,
    pub cursor_nibble: usize,
    pub dirty: bool,
    pub undo_stack: Vec<UndoEntry>,
    pub redo_stack: Vec<UndoEntry>,
    pub search: Option<Search>,
    pub search_active: bool,
    pub pack_ranges: Vec<(usize, usize)>,
    pub pack_match_idx: Option<usize>,
    pub global_match_idx: Option<usize>,
    pub input_buf: String,
    pub input_prompt: String,
    pub save_selected: bool,
    pub sel_start: Option<usize>,
    pub sel_end: Option<usize>,
    pub help_scroll: usize,
    pub help_rect: Option<(u16, u16, u16, u16)>, // x, y, w, h
    pub cursor_focused: bool,
    pub search_rx: Option<mpsc::Receiver<SearchEvent>>,
}

impl App {
    pub fn new(file_size: usize, filename: String) -> Self {
        let pack_size = 4096;
        Self {
            file_size,
            pack_size,
            total_packs: (file_size + pack_size - 1) / pack_size,
            current_pack: 0,
            scroll_top: 0,
            mode: DisplayMode::Ascii,
            input_mode: InputMode::Normal,
            filename,
            cursor_byte: 0,
            cursor_nibble: 0,
            dirty: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            search: None,
            search_active: false,
            pack_ranges: Vec::new(),
            pack_match_idx: None,
            global_match_idx: None,
            input_buf: String::new(),
            input_prompt: String::new(),
            save_selected: false,
            sel_start: None,
            sel_end: None,
            help_scroll: 0,
            help_rect: None,
            cursor_focused: true,
            search_rx: None,
        }
    }

    pub fn max_rows(&self, h: u16) -> usize {
        (h as usize).saturating_sub(4)
    }

    pub fn data_len(&self) -> usize {
        self.pack_size.min(self.file_size - self.current_pack * self.pack_size)
    }

    pub fn total_rows(&self) -> usize {
        (self.data_len() + 15) / 16
    }

    pub fn format_size(size: usize) -> String {
        let mut s = size as f64;
        for u in &["B", "KB", "MB", "GB"] {
            if s < 1024.0 {
                return if *u == "B" { format!("{}B", s as usize) } else { format!("{:.1}{}", s, u) };
            }
            s /= 1024.0;
        }
        format!("{:.1}TB", s)
    }

    pub fn current_match_range(&self) -> Option<(usize, usize)> {
        if !self.search_active { return None; }
        let idx = self.global_match_idx?;
        self.search.as_ref()?.ranges.get(idx).copied()
    }

    pub fn clear_search(&mut self) {
        self.search_active = false;
        self.search = None;
        self.pack_ranges.clear();
        self.pack_match_idx = None;
        self.global_match_idx = None;
    }

    pub fn refresh_pack(&mut self) {
        if let Some(ref s) = self.search {
            self.pack_ranges = s.pack_matches(self.current_pack);
            self.pack_match_idx = self.global_match_idx
                .and_then(|gi| s.ranges.get(gi).map(|&(st, _)| st))
                .and_then(|st| self.pack_ranges.iter().position(|(s, e)| *s <= st && st < *e));
        }
    }

    pub fn jump_global(&mut self, idx: usize) -> bool {
        let s = match self.search.as_ref() { Some(s) => s, None => return false };
        if idx >= s.ranges.len() { return false; }
        let (start, _) = s.ranges[idx];
        self.global_match_idx = Some(idx);
        self.current_pack = start / self.pack_size;
        let row = (start % self.pack_size) / 16;
        self.scroll_top = row.saturating_sub(12);
        self.refresh_pack();
        true
    }

    pub fn next_global(&mut self, mmap: &[u8]) -> bool {
        let ni = self.global_match_idx.map_or(0, |i| i + 1);
        let len = self.search.as_ref().map_or(0, |s| s.ranges.len());
        if ni < len { return self.jump_global(ni); }
        let last = self.search.as_ref().and_then(|s| s.ranges.last().map(|(_, e)| *e));
        if let Some(e) = last {
            let s = self.search.as_mut().unwrap();
            if s.extend(mmap, e + 1) && ni < s.ranges.len() {
                return self.jump_global(ni);
            }
        }
        false
    }

    pub fn prev_global(&mut self) -> bool {
        let cur = match self.global_match_idx { Some(i) => i, None => return false };
        if cur == 0 { return false; }
        self.jump_global(cur - 1)
    }

    pub fn next_page_match(&mut self) -> bool {
        let cur_pack = self.current_pack;
        if let Some(s) = self.search.as_ref() {
            for idx in 0..s.ranges.len() {
                let (start, _) = s.ranges[idx];
                if start / self.pack_size > cur_pack {
                    return self.jump_global(idx);
                }
            }
        }
        false
    }

    pub fn prev_page_match(&mut self) -> bool {
        let cur_pack = self.current_pack;
        if let Some(s) = self.search.as_ref() {
            for idx in (0..s.ranges.len()).rev() {
                let (start, _) = s.ranges[idx];
                if start / self.pack_size < cur_pack {
                    return self.jump_global(idx);
                }
            }
        }
        false
    }

    pub fn ensure_cursor_visible(&mut self, h: u16) {
        let pk = self.cursor_byte / self.pack_size;
        if pk != self.current_pack { self.current_pack = pk; }
        let row = (self.cursor_byte % self.pack_size) / 16;
        let mr = self.max_rows(h);
        if row < self.scroll_top { self.scroll_top = row; }
        else if row >= self.scroll_top + mr { self.scroll_top = row - mr + 1; }
        let tr = self.total_rows();
        self.scroll_top = self.scroll_top.min(tr.saturating_sub(mr));
    }

    pub fn modify(&mut self, mmap: &mut [u8], off: usize, val: u8) {
        if off < self.file_size && mmap[off] != val {
            self.undo_stack.push(UndoEntry { offset: off, old: mmap[off], new: val });
            self.redo_stack.clear();
            mmap[off] = val;
            self.dirty = true;
        }
    }

    pub fn undo(&mut self, mmap: &mut [u8]) {
        if let Some(e) = self.undo_stack.pop() {
            if e.offset < self.file_size {
                mmap[e.offset] = e.old;
                self.redo_stack.push(e);
                self.dirty = !self.undo_stack.is_empty();
            }
        }
    }

    pub fn redo(&mut self, mmap: &mut [u8]) {
        if let Some(e) = self.redo_stack.pop() {
            if e.offset < self.file_size {
                mmap[e.offset] = e.new;
                self.undo_stack.push(e);
                self.dirty = true;
            }
        }
    }

    pub fn edit_hex(&mut self, mmap: &mut [u8], ch: char) {
        let nib = match ch {
            '0'..='9' => ch as u8 - b'0',
            'a'..='f' => ch as u8 - b'a' + 10,
            'A'..='F' => ch as u8 - b'A' + 10,
            _ => return,
        };
        if self.cursor_byte >= self.file_size { return; }
        let cur = mmap[self.cursor_byte];
        let new = if self.cursor_nibble == 0 { (cur & 0x0f) | (nib << 4) } else { (cur & 0xf0) | nib };
        self.modify(mmap, self.cursor_byte, new);
        if self.cursor_nibble == 0 { self.cursor_nibble = 1; }
        else {
            self.cursor_nibble = 0;
            self.cursor_byte += 1;
            if self.cursor_byte >= self.file_size {
                self.cursor_byte = self.file_size - 1;
                self.cursor_nibble = 1;
            }
        }
    }

    pub fn edit_char(&mut self, mmap: &mut [u8], ch: char) {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        for &b in s.as_bytes() {
            if self.cursor_byte >= self.file_size { return; }
            self.modify(mmap, self.cursor_byte, b);
            self.cursor_byte += 1;
        }
        if self.cursor_byte >= self.file_size { self.cursor_byte = self.file_size - 1; }
    }

    /// Accept a pre-built Search, run it, position to first match
    pub fn apply_search(&mut self, mut acc: Search, mmap: &[u8], h: u16) -> bool {
        let off = self.current_pack * self.pack_size + self.scroll_top * 16;
        acc.extend(mmap, off);
        self.search_active = true;
        self.search = Some(acc);
        if !self.search.as_ref().unwrap().ranges.is_empty() {
            self.global_match_idx = Some(0);
            let (start, _) = self.search.as_ref().unwrap().ranges[0];
            self.current_pack = start / self.pack_size;
            let row = (start % self.pack_size) / 16;
            let mr = self.max_rows(h);
            let tr = self.total_rows();
            self.scroll_top = row.saturating_sub(mr / 2).min(tr.saturating_sub(mr));
        }
        self.refresh_pack();
        true
    }

    pub fn start_bg_search(&mut self, needle: Vec<u8>, data: Vec<u8>) {
        let file_size = self.file_size;
        let rx = crate::search::start_bg_search(needle, file_size, data);
        self.search_rx = Some(rx);
    }

    pub fn drain_search_rx(&mut self) {
        let events: Vec<SearchEvent> = {
            let rx = match self.search_rx { Some(ref rx) => rx, None => return };
            let mut events = Vec::new();
            loop {
                match rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(_) => break,
                }
            }
            events
        };
        for event in events {
            match event {
                SearchEvent::Chunk { matches } => {
                    if let Some(ref mut s) = self.search {
                        for &(start, end) in &matches {
                            if s.ranges.last().map_or(true, |(_, e)| *e <= start) {
                                s.ranges.push((start, end));
                                s.match_count += 1;
                                s.mark_all(start, end);
                            }
                        }
                    }
                }
                SearchEvent::Done => {
                    self.search_rx = None;
                }
            }
        }
        self.refresh_pack();
    }
}
