/// 四级位图搜索引擎
///
/// 固定 804 字节内存，替代原来的 ranges Vec + 后台线程。
/// 按需扫描，不预加载全文件。
///
/// 层级：
///   L3: 128B — 全文件 1024 个 1GB 块的存在性
///   L2: 128B — 当前 1GB 内 1024 个 1MB 块的存在性
///   L1:  32B — 当前 1MB 内 256 个 4K 页的存在性
///   L0: 512B — 当前 4K 页内 4096 字节的存在性
use crate::search::{atoms_match, Needle};

const L0_BITS: usize = 4096;
const L1_BITS: usize = 256;
const L2_BITS: usize = 1024;
const L3_BITS: usize = 1024;
const PAGE_SIZE: usize = 4096;
const MB_SIZE: usize = 1024 * 1024;
const GB_SIZE: usize = 1024 * 1024 * 1024;
const SCAN_CHUNK: usize = 1024 * 1024;

pub struct BitSearch {
    l0: [u8; 512],
    l1: [u8; 32],
    l2: [u8; 128],
    l3: [u8; 128],
    pub count: usize,
    scanned: usize,
    needle: Needle,
    needle_len: usize,
    pub label: String,
    cached_page: usize,
    cached_mb: usize,
    file_size: usize,
}

// ─── 位操作 ──────────────────────────────────────────────

fn set_bit(bm: &mut [u8], bit: usize) {
    bm[bit / 8] |= 1 << (bit % 8);
}

fn next_set(bm: &[u8], total_bits: usize, pos: usize) -> Option<usize> {
    if pos >= total_bits {
        return None;
    }
    let mut byte_idx = pos / 8;
    let bit_idx = pos % 8;
    let mask = !((1u8 << bit_idx) - 1);
    let b = bm[byte_idx] & mask;
    if b != 0 {
        return Some(byte_idx * 8 + b.trailing_zeros() as usize);
    }
    byte_idx += 1;
    while byte_idx * 8 < total_bits {
        if bm[byte_idx] != 0 {
            let bit = byte_idx * 8 + bm[byte_idx].trailing_zeros() as usize;
            if bit < total_bits {
                return Some(bit);
            }
        }
        byte_idx += 1;
    }
    None
}

fn prev_set(bm: &[u8], pos: usize) -> Option<usize> {
    let mut byte_idx = pos / 8;
    let bit_idx = pos % 8;
    if byte_idx < bm.len() {
        let mask = (1u8 << (bit_idx + 1)) - 1;
        let b = bm[byte_idx] & mask;
        if b != 0 {
            return Some(byte_idx * 8 + 7 - b.leading_zeros() as usize);
        }
    }
    while byte_idx > 0 {
        byte_idx -= 1;
        if bm[byte_idx] != 0 {
            return Some(byte_idx * 8 + 7 - bm[byte_idx].leading_zeros() as usize);
        }
    }
    None
}

// ─── 地址编解码 ──────────────────────────────────────────

fn decode(pos: usize) -> (usize, usize, usize, usize) {
    (
        pos / GB_SIZE,
        pos % GB_SIZE / MB_SIZE,
        pos % MB_SIZE / PAGE_SIZE,
        pos % PAGE_SIZE,
    )
}

fn encode(g3: usize, g2: usize, g1: usize, g0: usize) -> usize {
    g3 * GB_SIZE + g2 * MB_SIZE + g1 * PAGE_SIZE + g0
}

// ─── BitSearch ───────────────────────────────────────────

impl BitSearch {
    pub fn new(needle: Needle, needle_len: usize, label: String, file_size: usize) -> Self {
        Self {
            l0: [0; 512],
            l1: [0; 32],
            l2: [0; 128],
            l3: [0; 128],
            count: 0,
            scanned: 0,
            needle,
            needle_len,
            label,
            cached_page: usize::MAX,
            cached_mb: usize::MAX,
            file_size,
        }
    }

    pub fn has_more(&self) -> bool {
        self.scanned < self.file_size
    }

    fn mark(&mut self, pos: usize) {
        let (g3, g2, g1, g0) = decode(pos);
        set_bit(&mut self.l0, g0);
        set_bit(&mut self.l1, g1);
        set_bit(&mut self.l2, g2);
        set_bit(&mut self.l3, g3);
        self.count += 1;
    }

    pub fn scan_chunk(&mut self, data: &[u8]) -> usize {
        let start = self.scanned;
        let end = (start + SCAN_CHUNK).min(self.file_size);
        if start + self.needle_len > self.file_size {
            self.scanned = self.file_size;
            return 0;
        }
        let mut found = 0;
        let mut pos = start;
        while pos + self.needle_len <= end {
            if self.matches_at(data, pos) {
                self.mark(pos);
                found += 1;
            }
            pos += 1;
        }
        self.scanned = end;
        found
    }

    fn matches_at(&self, data: &[u8], pos: usize) -> bool {
        if pos + self.needle_len > data.len() {
            return false;
        }
        match &self.needle {
            Needle::Lit(bytes) => &data[pos..pos + bytes.len()] == bytes.as_slice(),
            Needle::Pat(atoms) => atoms_match(atoms, data, pos),
        }
    }

    fn ensure_cached(&mut self, data: &[u8], pos: usize) {
        let page_start = pos - (pos % PAGE_SIZE);
        if self.cached_page == page_start {
            return;
        }
        self.l0 = [0u8; 512];
        self.cached_page = page_start;
        let end = (page_start + PAGE_SIZE).min(self.file_size);
        let scan_end = end.min(self.scanned);
        let mut p = page_start;
        while p + self.needle_len <= scan_end {
            if self.matches_at(data, p) {
                set_bit(&mut self.l0, p % PAGE_SIZE);
            }
            p += 1;
        }
    }

    fn ensure_mb_cached(&mut self, data: &[u8], pos: usize) {
        let mb_key = pos / MB_SIZE;
        if self.cached_mb == mb_key {
            return;
        }
        self.l1 = [0u8; 32];
        self.l0 = [0u8; 512];
        self.cached_mb = mb_key;
        self.cached_page = usize::MAX;
        let mb_start = mb_key * MB_SIZE;
        let mb_end = (mb_start + MB_SIZE).min(self.file_size);
        let target_page = pos / PAGE_SIZE;
        let scan_end = mb_end.min(self.scanned);
        let mut p = mb_start;
        while p + self.needle_len <= scan_end {
            if self.matches_at(data, p) {
                let (_, _, sg1, sg0) = decode(p);
                set_bit(&mut self.l1, sg1);
                if p / PAGE_SIZE == target_page {
                    set_bit(&mut self.l0, sg0);
                }
            }
            p += 1;
        }
    }

    fn rebuild_l2_for_gb(&mut self, data: &[u8], g3: usize) {
        self.l2 = [0u8; 128];
        let gb_start = g3 * GB_SIZE;
        let gb_end = (gb_start + GB_SIZE).min(self.file_size);
        let scan_end = gb_end.min(self.scanned);
        let mut p = gb_start;
        while p + self.needle_len <= scan_end {
            if self.matches_at(data, p) {
                let (_, sg2, _, _) = decode(p);
                set_bit(&mut self.l2, sg2);
            }
            p += 1;
        }
    }

    // ─── 查询 ──────────────────────────────────────────

    pub fn next_match_after(&mut self, data: &[u8], from: usize) -> Option<usize> {
        if self.needle_len == 0 {
            return None;
        }
        loop {
            let probe = from + 1;
            if probe >= self.file_size {
                return None;
            }
            let (g3, g2, g1, g0) = decode(probe);

            if probe < self.scanned {
                // 在当前 4K 页找
                self.ensure_cached(data, probe);
                if let Some(bit) = next_set(&self.l0, L0_BITS, g0) {
                    let pos = encode(g3, g2, g1, bit);
                    if pos + self.needle_len <= self.file_size {
                        return Some(pos);
                    }
                }
                // 下一个 4K 页
                if let Some(bit) = next_set(&self.l1, L1_BITS, g1 + 1) {
                    let pos = encode(g3, g2, bit, 0);
                    self.ensure_cached(data, pos);
                    if let Some(g0b) = next_set(&self.l0, L0_BITS, 0) {
                        return Some(encode(g3, g2, bit, g0b));
                    }
                }
                // 下一个 1MB
                if let Some(bit) = next_set(&self.l2, L2_BITS, g2 + 1) {
                    let pos = encode(g3, bit, 0, 0);
                    self.ensure_mb_cached(data, pos);
                    if let Some(g1b) = next_set(&self.l1, L1_BITS, 0) {
                        let p2 = encode(g3, bit, g1b, 0);
                        self.ensure_cached(data, p2);
                        if let Some(g0b) = next_set(&self.l0, L0_BITS, 0) {
                            return Some(encode(g3, bit, g1b, g0b));
                        }
                    }
                }
                // 下一个 1GB
                if let Some(bit) = next_set(&self.l3, L3_BITS, g3 + 1) {
                    self.rebuild_l2_for_gb(data, bit);
                    if let Some(g2b) = next_set(&self.l2, L2_BITS, 0) {
                        let pos = encode(bit, g2b, 0, 0);
                        self.ensure_mb_cached(data, pos);
                        if let Some(g1b) = next_set(&self.l1, L1_BITS, 0) {
                            let p2 = encode(bit, g2b, g1b, 0);
                            self.ensure_cached(data, p2);
                            if let Some(g0b) = next_set(&self.l0, L0_BITS, 0) {
                                return Some(encode(bit, g2b, g1b, g0b));
                            }
                        }
                    }
                }
            }

            // 按需扫描
            if self.scanned < self.file_size {
                let old_count = self.count;
                self.scan_chunk(data);
                if self.count > old_count {
                    // 重新从 probe 开始查找（扫描可能在 probe 附近找到匹配）
                    continue;
                }
                // 扫描区域不在 probe 附近，继续扫
                continue;
            }
            return None;
        }
    }

    pub fn prev_match_before(&mut self, data: &[u8], from: usize) -> Option<usize> {
        if self.needle_len == 0 || from == 0 {
            return None;
        }
        let pos = from - 1;
        let (g3, g2, g1, g0) = decode(pos);

        // 当前 4K 页
        self.ensure_cached(data, pos);
        if let Some(bit) = prev_set(&self.l0, g0) {
            return Some(encode(g3, g2, g1, bit));
        }
        // 前面的 4K 页
        if g1 > 0 {
            if let Some(bit) = prev_set(&self.l1, g1 - 1) {
                let p = encode(g3, g2, bit, PAGE_SIZE - 1);
                self.ensure_cached(data, p);
                if let Some(g0b) = prev_set(&self.l0, L0_BITS - 1) {
                    return Some(encode(g3, g2, bit, g0b));
                }
            }
        }
        // 前面的 1MB
        if g2 > 0 {
            if let Some(bit) = prev_set(&self.l2, g2 - 1) {
                let p = encode(g3, bit, L1_BITS - 1, PAGE_SIZE - 1);
                self.ensure_mb_cached(data, p);
                if let Some(g1b) = prev_set(&self.l1, L1_BITS - 1) {
                    let p2 = encode(g3, bit, g1b, PAGE_SIZE - 1);
                    self.ensure_cached(data, p2);
                    if let Some(g0b) = prev_set(&self.l0, L0_BITS - 1) {
                        return Some(encode(g3, bit, g1b, g0b));
                    }
                }
            }
        }
        // 前面的 1GB
        if g3 > 0 {
            if let Some(bit) = prev_set(&self.l3, L3_BITS - 1) {
                self.rebuild_l2_for_gb(data, bit);
                if let Some(g2b) = prev_set(&self.l2, L2_BITS - 1) {
                    let p = encode(bit, g2b, L1_BITS - 1, PAGE_SIZE - 1);
                    self.ensure_mb_cached(data, p);
                    if let Some(g1b) = prev_set(&self.l1, L1_BITS - 1) {
                        let p2 = encode(bit, g2b, g1b, PAGE_SIZE - 1);
                        self.ensure_cached(data, p2);
                        if let Some(g0b) = prev_set(&self.l0, L0_BITS - 1) {
                            return Some(encode(bit, g2b, g1b, g0b));
                        }
                    }
                }
            }
        }
        None
    }

    /// 获取指定 pack 内的匹配范围（供渲染高亮用）
    pub fn pack_matches(
        &mut self,
        data: &[u8],
        pack_start: usize,
        pack_size: usize,
    ) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let pack_end = (pack_start + pack_size).min(self.file_size);
        let scan_end = pack_end.min(self.scanned);
        let mut pos = pack_start;
        while pos + self.needle_len <= scan_end {
            if self.matches_at(data, pos) {
                ranges.push((pos, pos + self.needle_len));
            }
            pos += 1;
        }
        ranges
    }
}
