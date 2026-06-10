use std::collections::HashSet;

use crate::search::SearchAccumulator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    Ascii,
    Hex,
    Utf8,
}

impl DisplayMode {
    pub fn next(self) -> Self {
        match self {
            Self::Ascii => Self::Hex,
            Self::Hex => Self::Utf8,
            Self::Utf8 => Self::Ascii,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ascii => "[ASCII]",
            Self::Hex => "[HEX]",
            Self::Utf8 => "[UTF8]",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Hex,
    Ascii,
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

    // Edit state
    pub edit_kind: EditKind,
    pub cursor_byte: usize,
    pub cursor_nibble: usize,
    pub dirty: bool,

    // Undo/redo
    pub undo_stack: Vec<UndoEntry>,
    pub redo_stack: Vec<UndoEntry>,

    // Search state
    pub search: Option<SearchAccumulator>,
    pub search_active: bool,
    pub pack_ranges: Vec<(usize, usize)>,
    pub pack_set: HashSet<usize>,
    pub pack_match_idx: Option<usize>,
    pub global_match_idx: Option<usize>,

    // Input buffer
    pub input_buf: String,
    pub input_prompt: String,

    // Save confirm
    pub save_selected: bool,
}

impl App {
    pub fn new(file_size: usize, filename: String) -> Self {
        let pack_size = 4096;
        let total_packs = (file_size + pack_size - 1) / pack_size;
        Self {
            file_size,
            pack_size,
            total_packs,
            current_pack: 0,
            scroll_top: 0,
            mode: DisplayMode::Ascii,
            input_mode: InputMode::Normal,
            filename,
            edit_kind: EditKind::Hex,
            cursor_byte: 0,
            cursor_nibble: 0,
            dirty: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            search: None,
            search_active: false,
            pack_ranges: Vec::new(),
            pack_set: HashSet::new(),
            pack_match_idx: None,
            global_match_idx: None,
            input_buf: String::new(),
            input_prompt: String::new(),
            save_selected: false,
        }
    }

    pub fn max_data_rows(&self, terminal_height: u16) -> usize {
        (terminal_height as usize).saturating_sub(4)
    }

    pub fn data_len(&self) -> usize {
        std::cmp::min(
            self.pack_size,
            self.file_size - self.current_pack * self.pack_size,
        )
    }

    pub fn total_rows(&self) -> usize {
        (self.data_len() + 15) / 16
    }

    pub fn refresh_pack_display(&mut self) {
        if let Some(ref search) = self.search {
            let (ranges, set) = search.get_current_pack_matches(self.current_pack);
            self.pack_ranges = ranges;
            self.pack_set = set;
            if let Some(gidx) = self.global_match_idx {
                if gidx < search.match_ranges.len() {
                    let (global_start, _) = search.match_ranges[gidx];
                    self.pack_match_idx = self
                        .pack_ranges
                        .iter()
                        .position(|(s, e)| *s <= global_start && global_start < *e);
                    return;
                }
            }
            self.pack_match_idx = None;
        }
    }

    pub fn jump_to_global_match(&mut self, idx: usize) -> bool {
        let search = match self.search.as_ref() {
            Some(s) => s,
            None => return false,
        };
        if idx >= search.match_ranges.len() {
            return false;
        }
        let (start, _) = search.match_ranges[idx];
        self.global_match_idx = Some(idx);
        self.current_pack = start / self.pack_size;
        let offset_in_pack = start % self.pack_size;
        let row = offset_in_pack / 16;
        let total = self.total_rows();
        let max_rows = 25;
        self.scroll_top = row
            .saturating_sub(max_rows / 2)
            .min(total.saturating_sub(max_rows));
        self.refresh_pack_display();
        true
    }

    pub fn jump_to_next_global_match(&mut self, mmap: &[u8]) -> bool {
        let new_idx = self.global_match_idx.map_or(0, |i| i + 1);
        let len = self.search.as_ref().map_or(0, |s| s.match_ranges.len());
        if new_idx < len {
            return self.jump_to_global_match(new_idx);
        }
        let last_end = self
            .search
            .as_ref()
            .and_then(|s| s.match_ranges.last().map(|(_, e)| *e));
        if let Some(e) = last_end {
            let search = self.search.as_mut().unwrap();
            if search.extend_scan(mmap, e + 1) {
                if new_idx < search.match_ranges.len() {
                    return self.jump_to_global_match(new_idx);
                }
            }
        }
        false
    }

    pub fn jump_to_prev_global_match(&mut self) -> bool {
        let cur = match self.global_match_idx {
            Some(i) => i,
            None => return false,
        };
        if cur == 0 {
            return false;
        }
        self.jump_to_global_match(cur - 1)
    }

    pub fn ensure_cursor_visible(&mut self, terminal_height: u16) {
        let pack_of_cursor = self.cursor_byte / self.pack_size;
        if pack_of_cursor != self.current_pack {
            self.current_pack = pack_of_cursor;
        }
        let offset_in_pack = self.cursor_byte % self.pack_size;
        let row_of_cursor = offset_in_pack / 16;
        let max_rows = self.max_data_rows(terminal_height);
        if row_of_cursor < self.scroll_top {
            self.scroll_top = row_of_cursor;
        } else if row_of_cursor >= self.scroll_top + max_rows {
            self.scroll_top = row_of_cursor - max_rows + 1;
        }
        let total = self.total_rows();
        self.scroll_top = self
            .scroll_top
            .min(total.saturating_sub(max_rows));
    }

    pub fn modify_byte(&mut self, mmap: &mut [u8], offset: usize, value: u8) {
        if offset < self.file_size && mmap[offset] != value {
            self.undo_stack.push(UndoEntry {
                offset,
                old: mmap[offset],
                new: value,
            });
            self.redo_stack.clear();
            mmap[offset] = value;
            self.dirty = true;
        }
    }

    pub fn undo(&mut self, mmap: &mut [u8]) {
        if let Some(entry) = self.undo_stack.pop() {
            if entry.offset < self.file_size {
                mmap[entry.offset] = entry.old;
                self.redo_stack.push(entry);
                self.dirty = !self.undo_stack.is_empty();
            }
        }
    }

    pub fn redo(&mut self, mmap: &mut [u8]) {
        if let Some(entry) = self.redo_stack.pop() {
            if entry.offset < self.file_size {
                mmap[entry.offset] = entry.new;
                self.undo_stack.push(entry);
                self.dirty = true;
            }
        }
    }

    pub fn edit_hex_input(&mut self, mmap: &mut [u8], ch: char) {
        let nib = match ch {
            '0'..='9' => (ch as u8 - b'0') as u8,
            'a'..='f' => (ch as u8 - b'a' + 10) as u8,
            'A'..='F' => (ch as u8 - b'A' + 10) as u8,
            _ => return,
        };
        if self.cursor_byte >= self.file_size {
            return;
        }
        let cur = mmap[self.cursor_byte];
        let new_byte = if self.cursor_nibble == 0 {
            (cur & 0x0f) | (nib << 4)
        } else {
            (cur & 0xf0) | nib
        };
        self.modify_byte(mmap, self.cursor_byte, new_byte);
        if self.cursor_nibble == 0 {
            self.cursor_nibble = 1;
        } else {
            self.cursor_nibble = 0;
            self.cursor_byte += 1;
            if self.cursor_byte >= self.file_size {
                self.cursor_byte = self.file_size - 1;
                self.cursor_nibble = 1;
            }
        }
    }

    pub fn edit_ascii_input(&mut self, mmap: &mut [u8], ch: char) {
        if self.cursor_byte >= self.file_size {
            return;
        }
        let b = (ch as u32 & 0xff) as u8;
        self.modify_byte(mmap, self.cursor_byte, b);
        self.cursor_byte += 1;
        if self.cursor_byte >= self.file_size {
            self.cursor_byte = self.file_size - 1;
        }
    }

    pub fn format_size(size: usize) -> String {
        let units = ["B", "KB", "MB", "GB"];
        let mut s = size as f64;
        for unit in &units {
            if s < 1024.0 {
                return if *unit == "B" {
                    format!("{}{}", s as usize, unit)
                } else {
                    format!("{:.1}{}", s, unit)
                };
            }
            s /= 1024.0;
        }
        format!("{:.1}TB", s)
    }

    pub fn current_match_range(&self) -> Option<(usize, usize)> {
        if !self.search_active {
            return None;
        }
        let idx = self.global_match_idx?;
        let search = self.search.as_ref()?;
        search.match_ranges.get(idx).copied()
    }

    pub fn clear_search(&mut self) {
        self.search_active = false;
        self.search = None;
        self.pack_ranges.clear();
        self.pack_set.clear();
        self.pack_match_idx = None;
        self.global_match_idx = None;
    }

    pub fn do_search(
        &mut self,
        mmap: &[u8],
        is_regex: bool,
        needle_bytes: Vec<u8>,
        regex: Option<regex::bytes::Regex>,
        label: String,
        terminal_height: u16,
    ) -> bool {
        let mut acc = if is_regex {
            SearchAccumulator::new_regex(regex.unwrap(), self.pack_size, self.file_size, label)
        } else {
            SearchAccumulator::new_hex(needle_bytes, self.pack_size, self.file_size, label)
        };
        let start_offset = self.current_pack * self.pack_size + self.scroll_top * 16;
        acc.extend_scan(mmap, start_offset);
        if !acc.match_ranges.is_empty() {
            self.search_active = true;
            self.global_match_idx = Some(0);
            let (start, _) = acc.match_ranges[0];
            self.current_pack = start / self.pack_size;
            let offset_in_pack = start % self.pack_size;
            let row = offset_in_pack / 16;
            let max_rows = self.max_data_rows(terminal_height);
            let total = self.total_rows();
            self.scroll_top = row
                .saturating_sub(max_rows / 2)
                .min(total.saturating_sub(max_rows));
            self.search = Some(acc);
            self.refresh_pack_display();
            true
        } else {
            false
        }
    }

    pub fn pack_and_row_for_offset(offset: usize, pack_size: usize) -> (usize, usize) {
        let pack = offset / pack_size;
        let row = (offset % pack_size) / 16;
        (pack, row)
    }
}
