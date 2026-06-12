pub const FIND_CHUNK: usize = 1024 * 1024;
const CHUNK: usize = 1024 * 1024;
const L1_SIZE: usize = 1024 * 1024;
const L2_SIZE: usize = 1024 * 1024 * 1024;

pub struct Search {
    needle: Needle,
    pack_size: usize,
    file_size: usize,
    pub ranges: Vec<(usize, usize)>,
    has_match_l0: Vec<u8>,
    has_match_l1: Vec<u8>,
    has_match_l2: Vec<u8>,
    pub match_count: usize,
    scanned: usize,
    pub label: String,
}

enum Needle {
    Lit(Vec<u8>),
    Pat(Vec<NibAtom>),
}

/// Per-nibble matcher. Two NibAtoms = one byte constraint.
#[derive(Clone)]
pub enum NibAtom {
    Exact(u8),       // 0x0..0xF
    Range(u8, u8),   // inclusive nibble range
    Any,             // x
}

impl Search {
    pub fn new_hex(data: Vec<u8>, ps: usize, fs: usize, label: String) -> Self {
        let l0 = (fs + ps - 1) / ps;
        let l1 = (fs + L1_SIZE - 1) / L1_SIZE;
        let l2 = (fs + L2_SIZE - 1) / L2_SIZE;
        Self { needle: Needle::Lit(data), pack_size: ps, file_size: fs, ranges: Vec::new(),
            has_match_l0: vec![0u8; (l0 + 7) / 8],
            has_match_l1: vec![0u8; (l1 + 7) / 8],
            has_match_l2: vec![0u8; (l2 + 7) / 8],
            match_count: 0, scanned: 0, label }
    }
    pub fn new_pat(pat: Vec<NibAtom>, ps: usize, fs: usize, label: String) -> Self {
        let l0 = (fs + ps - 1) / ps;
        let l1 = (fs + L1_SIZE - 1) / L1_SIZE;
        let l2 = (fs + L2_SIZE - 1) / L2_SIZE;
        Self { needle: Needle::Pat(pat), pack_size: ps, file_size: fs, ranges: Vec::new(),
            has_match_l0: vec![0u8; (l0 + 7) / 8],
            has_match_l1: vec![0u8; (l1 + 7) / 8],
            has_match_l2: vec![0u8; (l2 + 7) / 8],
            match_count: 0, scanned: 0, label }
    }
    pub fn has_more(&self) -> bool { self.scanned < self.file_size }
    pub fn extend(&mut self, mmap: &[u8], min_off: usize) -> bool {
        if min_off < self.scanned { return false; }
        let mut start = (self.scanned / CHUNK) * CHUNK;
        let mut found = false;
        while start < self.file_size && (start < min_off + CHUNK || self.ranges.is_empty()) {
            let end = (start + CHUNK).min(self.file_size);
            let new_matches: Vec<(usize, usize)> = match &self.needle {
                Needle::Lit(needle) => {
                    let nlen = needle.len();
                    if nlen == 0 { break; }
                    let mut out = Vec::new();
                    let mut pos = start;
                    loop {
                        pos = match mmap[pos..end].windows(nlen).position(|w| w == needle.as_slice()) {
                            Some(p) => pos + p, None => break,
                        };
                        let me = pos + nlen;
                        if self.ranges.last().map_or(true, |(_, e)| *e <= pos) {
                            out.push((pos, me));
                        }
                        pos += 1;
                    }
                    out
                }
                Needle::Pat(atoms) => {
                    let nbytes = atoms.len() / 2;
                    if nbytes == 0 { break; }
                    let mut out = Vec::new();
                    let mut pos = start;
                    while pos + nbytes <= end {
                        if atoms_match(atoms, mmap, pos) {
                            let me = pos + nbytes;
                            if self.ranges.last().map_or(true, |(_, e)| *e <= pos) {
                                out.push((pos, me));
                            }
                        }
                        pos += 1;
                    }
                    out
                }
            };
            for &(s, e) in &new_matches {
                self.ranges.push((s, e));
                self.match_count += 1;
                self.mark_all(s, e);
            }
            if !self.ranges.is_empty() && !found { found = true; }
            self.scanned = end;
            start = end;
        }
        found
    }
    pub fn find_after(&mut self, mmap: &[u8], min: usize) -> Option<usize> {
        if let Some(i) = self.ranges.iter().position(|(s, _)| *s >= min) { return Some(i); }
        self.extend(mmap, min);
        self.ranges.iter().position(|(s, _)| *s >= min)
    }
    pub fn pack_matches(&self, pi: usize) -> Vec<(usize, usize)> {
        let base = pi * self.pack_size;
        let end = (base + self.pack_size).min(self.file_size);
        let mut ranges = Vec::new();
        for &(s, e) in &self.ranges {
            if s >= end { break; }
            if e > base {
                ranges.push((s.max(base), e.min(end)));
            }
        }
        ranges
    }
    pub fn idx_for_offset(&self, off: usize) -> Option<usize> {
        self.ranges.iter().position(|(s, e)| *s <= off && off < *e)
    }

    fn mark_all(&mut self, start: usize, end: usize) {
        let ep = (end - 1).max(start);
        for p in (start / self.pack_size)..=(ep / self.pack_size) {
            self.has_match_l0[p / 8] |= 1 << (p % 8);
        }
        for p in (start / L1_SIZE)..=(ep / L1_SIZE) {
            self.has_match_l1[p / 8] |= 1 << (p % 8);
        }
        for p in (start / L2_SIZE)..=(ep / L2_SIZE) {
            self.has_match_l2[p / 8] |= 1 << (p % 8);
        }
    }
}

fn atoms_match(atoms: &[NibAtom], data: &[u8], pos: usize) -> bool {
    atoms.chunks(2).enumerate().all(|(i, pair)| {
        let b = data[pos + i];
        let hi_ok = nib_match(&pair[0], b >> 4);
        let lo_ok = if pair.len() > 1 { nib_match(&pair[1], b & 0x0f) } else { true };
        hi_ok && lo_ok
    })
}

fn nib_match(a: &NibAtom, val: u8) -> bool {
    match a {
        NibAtom::Exact(n) => val == *n,
        NibAtom::Range(lo, hi) => val >= *lo && val <= *hi,
        NibAtom::Any => true,
    }
}

pub enum SearchKind {
    Hex { bytes: Vec<u8>, label: String },
    Pat { pat: Vec<NibAtom>, label: String },
}

pub fn parse_input(s: &str) -> Option<SearchKind> {
    let t = s.trim();
    if t.is_empty() { return None; }
    // pure hex
    let hex: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if !hex.is_empty() && hex.len() % 2 == 0 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        if let Some(bytes) = decode_hex(&hex) {
            return Some(SearchKind::Hex { bytes, label: t.into() });
        }
    }
    // x/z/[] nibble pattern
    if let Some(pat) = compile_pat(t) {
        return Some(SearchKind::Pat { pat, label: t.into() });
    }
    None
}

pub fn parse_str_input(s: &str) -> Option<(String, Vec<u8>)> {
    let t = s.trim();
    if t.is_empty() { return None; }
    Some((format!("\"{}\"", t), t.as_bytes().to_vec()))
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i+2], 16).ok()).collect()
}

fn hv(c: char) -> Option<u8> { c.to_digit(16).map(|v| v as u8) }

fn tokenize(s: &str) -> Option<Vec<NibAtom>> {
    let ch: Vec<char> = s.to_lowercase().chars().collect();
    let mut atoms = Vec::new();
    let mut i = 0;
    while i < ch.len() {
        if ch[i].is_ascii_whitespace() { i += 1; continue; }
        if ch[i] == 'z' {
            atoms.push(NibAtom::Any);
            atoms.push(NibAtom::Any);
            i += 1;
        } else if ch[i] == 'x' {
            atoms.push(NibAtom::Any);
            i += 1;
        } else if ch[i] == '[' {
            let start = i + 1;
            let end = ch[start..].iter().position(|&c| c == ']')? + start;
            let inner: String = ch[start..end].iter().collect();
            let (lo_s, hi_s) = inner.split_once('-')?;
            let lo = hv(lo_s.trim().chars().next()?)?;
            let hi = hv(hi_s.trim().chars().next()?)?;
            atoms.push(NibAtom::Range(lo.min(hi), lo.max(hi)));
            i = end + 1;
        } else if ch[i].is_ascii_hexdigit() {
            atoms.push(NibAtom::Exact(hv(ch[i])?));
            i += 1;
        } else {
            return None;
        }
    }
    Some(atoms)
}

fn compile_pat(s: &str) -> Option<Vec<NibAtom>> {
    let atoms = tokenize(s)?;
    if atoms.is_empty() { return None; }
    if atoms.len() % 2 != 0 { return None; }
    Some(atoms)
}
