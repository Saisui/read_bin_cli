/// Sparse Hierarchical Bitmap — 稀疏层级位图
///
/// 作者：Saisui（本项目的发明设计）
///
/// 用于高效追踪文件中被编辑过的字节。灵感来源于四级位图搜索引擎（BitSearch），
/// 但采用按需分配（sparse）策略，而非预分配全量位图。
///
/// ## 设计思想
///
/// 将文件偏移空间按 4K → 1MB → 1GB → 1TB 分层，每层用位图标记"该区域是否有编辑"。
/// 查询时从顶层向下逐级缩小范围，任意一层"无"则跳过整棵子树。
/// 只为实际有编辑的区域分配下级位图，未编辑区域零开销。
///
/// ## 结构
///
/// ```text
/// L3: [u8; 128]                     128B 固定（1024 个 1GB 块）
/// L2: HashMap<usize, [u8; 128]>     每个有编辑的 1GB 块分配 128B
/// L1: HashMap<usize, [u8; 32]>      每个有编辑的 1MB 块分配 32B
/// L0: HashMap<usize, [u8; 512]>     每个有编辑的 4K 页分配 512B
/// ```
///
/// ## 复杂度
///
/// - 标记（mark）：O(1) 位操作，按需分配
/// - 查询（is_modified）：O(1) 最多 4 次位操作
/// - 内存：与编辑数量成正比，与文件大小无关
///
/// ## 与 Multi-level Page Table 的类比
///
/// 操作系统页表：PGD → PUD → PMD → PTE → 物理页
/// 本结构：      L3  → L2  → L1  → L0  → 字节
///
/// 都是逐级缩小范围、按需分配下级的层级索引。
use std::collections::HashMap;

const L0_BYTES: usize = 512; // 4096 bits / 8
const L1_BYTES: usize = 32; // 256 bits / 8
const L2_BYTES: usize = 128; // 1024 bits / 8
const L3_BYTES: usize = 128; // 1024 bits / 8

/// 稀疏层级位图
///
/// 固定开销：L3 (128B) + 3 个 HashMap 头（约 200B）≈ 328B
/// 每个有编辑的区域按需分配子位图。
pub struct ModifiedMap {
    /// L3: 全文件 1024 个 1GB 块的存在性（固定 128B）
    l3: [u8; L3_BYTES],
    /// L2: 每个有编辑的 1GB 块 → 该块内 1024 个 1MB 块的存在性
    l2: HashMap<usize, [u8; L2_BYTES]>,
    /// L1: 每个有编辑的 1MB 块 → 该块内 256 个 4K 页的存在性
    l1: HashMap<usize, [u8; L1_BYTES]>,
    /// L0: 每个有编辑的 4K 页 → 该页内 4096 字节的存在性
    l0: HashMap<usize, [u8; L0_BYTES]>,
}

impl ModifiedMap {
    /// 创建空的层级位图
    pub fn new() -> Self {
        Self {
            l3: [0; L3_BYTES],
            l2: HashMap::new(),
            l1: HashMap::new(),
            l0: HashMap::new(),
        }
    }

    /// 标记一个字节偏移为"已编辑"
    ///
    /// 按需分配 L0/L1/L2 子位图，未触及的区域零开销。
    pub fn mark(&mut self, offset: usize) {
        let l3_idx = offset >> 30; // / 1GB
        let l2_idx = (offset >> 20) & 0x3FF; // / 1MB % 1024
        let l1_idx = (offset >> 12) & 0xFF; // / 4K % 256
        let l0_idx = offset & 0xFFF; // % 4K

        // L3
        self.l3[l3_idx / 8] |= 1 << (l3_idx % 8);

        // L2: 按需分配
        self.l2.entry(l3_idx).or_insert([0; L2_BYTES])[l2_idx / 8] |= 1 << (l2_idx % 8);

        // L1: 按需分配
        let l1_key = l3_idx * 1024 + l2_idx;
        self.l1.entry(l1_key).or_insert([0; L1_BYTES])[l1_idx / 8] |= 1 << (l1_idx % 8);

        // L0: 按需分配
        let l0_key = l1_key * 256 + l1_idx;
        self.l0.entry(l0_key).or_insert([0; L0_BYTES])[l0_idx / 8] |= 1 << (l0_idx % 8);
    }

    /// 查询一个字节偏移是否被编辑过
    ///
    /// 逐级下降，任意一层"无"则立即返回 false。
    /// 最多 4 次位操作，绝大多数在 L3/L2 就跳过了。
    pub fn is_modified(&self, offset: usize) -> bool {
        let l3_idx = offset >> 30;
        let l2_idx = (offset >> 20) & 0x3FF;
        let l1_idx = (offset >> 12) & 0xFF;
        let l0_idx = offset & 0xFFF;

        // L3: 这个 1GB 块有没有编辑？
        if self.l3[l3_idx / 8] & (1 << (l3_idx % 8)) == 0 {
            return false;
        }
        // L2: 这个 1MB 块有没有编辑？
        let l2 = match self.l2.get(&l3_idx) {
            Some(b) => b,
            None => return false,
        };
        if l2[l2_idx / 8] & (1 << (l2_idx % 8)) == 0 {
            return false;
        }
        // L1: 这个 4K 页有没有编辑？
        let l1_key = l3_idx * 1024 + l2_idx;
        let l1 = match self.l1.get(&l1_key) {
            Some(b) => b,
            None => return false,
        };
        if l1[l1_idx / 8] & (1 << (l1_idx % 8)) == 0 {
            return false;
        }
        // L0: 这个字节有没有编辑？
        let l0_key = l1_key * 256 + l1_idx;
        let l0 = match self.l0.get(&l0_key) {
            Some(b) => b,
            None => return false,
        };
        l0[l0_idx / 8] & (1 << (l0_idx % 8)) != 0
    }
}
