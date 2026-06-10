use regex::bytes::Regex;
use std::collections::HashSet;

pub const FIND_CHUNK_SIZE: usize = 1024 * 1024;

pub struct SearchAccumulator {
    needle: Needle,
    pack_size: usize,
    file_size: usize,
    pub match_ranges: Vec<(usize, usize)>,
    pub matches_set: HashSet<usize>,
    scanned_until: usize,
    pub user_pattern: String,
}

enum Needle {
    Literal(Vec<u8>),
    Regex(Regex),
}

impl SearchAccumulator {
    pub fn new_hex(data: Vec<u8>, pack_size: usize, file_size: usize, label: String) -> Self {
        Self {
            needle: Needle::Literal(data),
            pack_size,
            file_size,
            match_ranges: Vec::new(),
            matches_set: HashSet::new(),
            scanned_until: 0,
            user_pattern: label,
        }
    }

    pub fn new_regex(re: Regex, pack_size: usize, file_size: usize, label: String) -> Self {
        Self {
            needle: Needle::Regex(re),
            pack_size,
            file_size,
            match_ranges: Vec::new(),
            matches_set: HashSet::new(),
            scanned_until: 0,
            user_pattern: label,
        }
    }

    pub fn has_more(&self) -> bool {
        self.scanned_until < self.file_size
    }

    /// Extend scan at least until min_offset. Returns true if any new match found.
    pub fn extend_scan(&mut self, mmap: &[u8], min_offset: usize) -> bool {
        if min_offset < self.scanned_until {
            return false;
        }
        let mut start = (self.scanned_until / FIND_CHUNK_SIZE) * FIND_CHUNK_SIZE;
        let mut found = false;

        while start < self.file_size
            && (start < min_offset + FIND_CHUNK_SIZE || self.match_ranges.is_empty())
        {
            let end = std::cmp::min(start + FIND_CHUNK_SIZE, self.file_size);
            let chunk = &mmap[start..end];

            match &self.needle {
                Needle::Literal(needle) => {
                    let nlen = needle.len();
                    let mut pos = start;
                    loop {
                        pos = if let Some(p) = find_subsequence(&mmap[pos..end], needle) {
                            pos + p
                        } else {
                            break
                        };
                        let m_start = pos;
                        let m_end = pos + nlen;
                        if self.match_ranges.last().map_or(true, |(_, e)| *e <= m_start) {
                            self.match_ranges.push((m_start, m_end));
                            for off in m_start..m_end {
                                self.matches_set.insert(off);
                            }
                        }
                        pos += 1;
                    }
                }
                Needle::Regex(re) => {
                    let mut pos = 0;
                    while let Some(m) = re.find_at(chunk, pos) {
                        let m_start = start + m.start();
                        let m_end = start + m.end();
                        if self.match_ranges.last().map_or(true, |(_, e)| *e <= m_start) {
                            self.match_ranges.push((m_start, m_end));
                            for off in m_start..m_end {
                                self.matches_set.insert(off);
                            }
                        }
                        pos = m.end();
                    }
                }
            }

            if !self.match_ranges.is_empty() && !found {
                found = true;
            }
            self.scanned_until = end;
            start = end;
        }
        found
    }

    pub fn get_match_index_for_offset(&self, offset: usize) -> Option<usize> {
        self.match_ranges
            .iter()
            .position(|(s, e)| *s <= offset && offset < *e)
    }

    pub fn get_current_pack_matches(&self, pack_idx: usize) -> (Vec<(usize, usize)>, HashSet<usize>) {
        let base = pack_idx * self.pack_size;
        let end = std::cmp::min(base + self.pack_size, self.file_size);
        let mut pack_ranges = Vec::new();
        let mut pack_set = HashSet::new();
        for &(s, e) in &self.match_ranges {
            if s >= end {
                break;
            }
            if e > base {
                let rs = std::cmp::max(s, base);
                let re = std::cmp::min(e, end);
                pack_ranges.push((rs, re));
                for off in rs..re {
                    pack_set.insert(off);
                }
            }
        }
        (pack_ranges, pack_set)
    }

    pub fn find_next_match_after_offset(&mut self, mmap: &[u8], min_offset: usize) -> Option<usize> {
        if let Some(idx) = self.match_ranges.iter().position(|(s, _)| *s >= min_offset) {
            return Some(idx);
        }
        if self.extend_scan(mmap, min_offset) {
            self.match_ranges
                .iter()
                .position(|(s, _)| *s >= min_offset)
        } else {
            None
        }
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

/// Parse search input: returns (label, type, data).
/// Supports: hex string, advanced hex (x/z), regex (/pattern/), plain text.
pub fn parse_search_input(input: &str) -> Option<(String, bool, Vec<u8>, Option<Regex>)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Regex: /pattern/
    if trimmed.starts_with('/') && trimmed.ends_with('/') && trimmed.len() >= 2 {
        let pattern = &trimmed[1..trimmed.len() - 1];
        match Regex::new(pattern) {
            Ok(re) => return Some((trimmed.to_string(), true, vec![], Some(re))),
            Err(_) => return None,
        }
    }

    // Pure hex string
    let hex_clean: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    if !hex_clean.is_empty()
        && hex_clean.len() % 2 == 0
        && hex_clean.chars().all(|c| c.is_ascii_hexdigit())
    {
        if let Some(bytes) = decode_hex(&hex_clean) {
            return Some((trimmed.to_string(), false, bytes, None));
        }
    }

    // Advanced hex with x/z
    let lower = trimmed.to_lowercase();
    if lower.contains('x') || lower.contains('z') {
        if let Some(re) = compile_advanced_hex(trimmed) {
            return Some((trimmed.to_string(), true, vec![], Some(re)));
        } else {
            return None;
        }
    }

    // Fallback: treat as latin-1 regex
    let conv = convert_hex_in_pattern(trimmed);
    match Regex::new(&conv) {
        Ok(re) => Some((trimmed.to_string(), true, vec![], Some(re))),
        Err(_) => None,
    }
}

pub fn parse_string_search(input: &str) -> Option<(String, Vec<u8>)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes().to_vec();
    Some((format!("\"{}\"", trimmed), bytes))
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect::<Option<Vec<u8>>>()
}

fn compile_advanced_hex(s: &str) -> Option<Regex> {
    let clean: String = s
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_hexdigit() || *c == 'x' || *c == 'z')
        .collect();
    if clean.is_empty() {
        return None;
    }
    let mut pattern = String::new();
    let chars: Vec<char> = clean.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == 'z' {
            pattern.push_str("[\\x00-\\xff]");
            i += 1;
        } else {
            if i + 1 >= chars.len() {
                return None;
            }
            let a = chars[i];
            let b = chars[i + 1];
            if a != 'x' && b != 'x' {
                pattern.push_str(&format!("\\x{:02x}", hex_val(a)? * 16 + hex_val(b)?));
            } else if a != 'x' && b == 'x' {
                pattern.push_str(&format!("[\\x{:0}0-\\x{:0}f]", hex_val(a)?, hex_val(a)?));
            } else if a == 'x' && b != 'x' {
                let v = hex_val(b)?;
                let alts: Vec<String> = (0..16u8)
                    .map(|hi| format!("\\x{:02x}", hi * 16 + v))
                    .collect();
                pattern.push_str(&format!("({})", alts.join("|")));
            } else {
                pattern.push_str("[\\x00-\\xff]");
            }
            i += 2;
        }
    }
    Regex::new(&pattern).ok()
}

fn hex_val(c: char) -> Option<u8> {
    c.to_digit(16).map(|v| v as u8)
}

fn convert_hex_in_pattern(s: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() {
            let a = chars[i];
            let b = chars[i + 1];
            if a.is_ascii_hexdigit() && b.is_ascii_hexdigit() && (i == 0 || chars[i - 1] != '\\') {
                result.push_str(&format!("\\x{:02x}", hex_val(a).unwrap_or(0) * 16 + hex_val(b).unwrap_or(0)));
                i += 2;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}
