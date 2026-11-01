// =========================================================================
// 📦 桶存储抽象（BucketStore）
// =========================================================================
//
// 统一封装「按编码值 `code` 索引的桶集合」，对调用方屏蔽底层是密集 `Vec`
// 还是稀疏 `FxHashMap`（需求 1/5）。
//
// 选择依据：**Dense 的绝对内存占用是否可接受**，占用率只是第二道判断。
//
// 密集后端按容量一次性预分配，无论多稀疏都吃满 `capacity × sizeof(B)`；
// 稀疏后端的内存随非空桶数增长（非空桶数恒 ≤ n_chars），但每次桶访问要付哈希开销。
//
//   - `capacity > SPARSE_THRESHOLD`              → Sparse（绝对上限，防容量爆内存）
//   - `capacity × sizeof(B) ≤ DENSE_BUDGET_BYTES` → Dense（占用可接受，用数组换主循环速度）
//   - `capacity > n_chars × SPARSE_ENTRY_FACTOR`  → Sparse（超预算且槽数远超可能占用）
//   - 否则                                        → Dense（超预算但占用率高，稀疏省不了多少）
//
// 为什么先看绝对占用、而不是像旧版那样先看占用率：
//
// 「4 码定长方案」(`code_space = 32^4 = 1,048,576`，`FullBucket` 48 B) 一次性预分配
// 每评估器 48 MB，而实际非空桶仅约 1.1 万个（利用率 3%）；多线程各持一份，且
// `update_char` 每次都在巨型工作集里随机跳，远超 L3 —— 必须走 Sparse（约 0.7 MB）。
//
// 但反过来的场景同样真实：3 码方案 `code_space = 32^3 = 32,768`，Dense 只要 1.3 MB/线程，
// 走 Sparse 只省下 1 MB，却让每次桶访问多一次哈希与探测。实测（同一 `moling` 配置，
// release，本机）：
//
//   | 指标        | Sparse | Dense  |
//   |-------------|--------|--------|
//   | try_move    | 9.3 μs | 6.9 μs |
//   | try_swap    | 19.5 μs| 14.2 μs|
//   | SA 吞吐     | 7.0 万步/秒 | 11.0 万步/秒 |
//   | 评估器构造  | 0.2 ms | 1.2 ms |
//
// 即 Dense 的构造代价（预分配 + 清零）只在启动时付一次，而桶访问在 SA 主循环里
// 每步都要付，故小容量必须选 Dense。8 MiB 的预算对应「构造一次 ≈ 数毫秒、
// 每线程占用在 L3/内存带宽可忽略」的区间。
//
// 关键不变量（需求 2）：调用方在桶成员降为 0 后必须调用 `remove`，使稀疏后端
// 的条目集合恒等于「当前非空桶集合」，条目数 ≤ n_chars。

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

/// 桶成员存储类型（汉字下标 ci，u32）。
///
/// 用 `SmallVec<[u32; 2]>` 而非 `Vec<u32>`：内联容量 2，与 `Vec<u32>` 同为 24 字节
/// （`FullBucket`/`SimpleBucket` 结构体大小不变，内存估算不受影响），但成员数 ≤2 的桶
/// （全码场景下绝大多数桶仅 1 个成员、少量 2 个）完全不触碰堆，消除退火热路径上桶空↔非空
/// 转换时的 `malloc`/`free` 抖动（见 `BucketStore::remove` 与 `Bucket::reset`）。
///
/// 注：`SmallVec<[u32; N]>` 在本 smallvec 版本中 len/cap 各占一字（共 16 字节）+ 内联/堆
/// 联合体（≥8 字节）。`N=2` 时联合体 8 字节 ⟹ 共 24 字节（与 `Vec<u32>` 等大）；`N=4` 时
/// 联合体 16 字节 ⟹ 32 字节（会使 `FullBucket` 增至 56 字节）。故取 `N=2` 以零内存增长换取
/// 对绝大多数小桶的内联化。成员超过 2 的桶（罕见的重码大桶）溢出到堆，行为与 `Vec` 一致。
pub type Members = SmallVec<[u32; 2]>;

/// 后端选择阈值（需求 12.6：唯一允许的编译期规模常量）。
///
/// 取 `2^21 = 2,097,152`：作为**容量绝对上限**——超过它一律走稀疏后端，
/// 不参与「容量 vs 字数」的权衡（避免 n_chars 很大时 Dense 仍然爆内存）。
pub const SPARSE_THRESHOLD: usize = 1 << 21;

/// Dense 后端的每评估器内存预算（字节）。
///
/// 与容量无关，直接给「每评估器 Dense 占用」定一个预算：只要 `capacity × sizeof(B)`
/// 在这个预算内，就用 Dense 换数组访问速度（实测 SA 吞吐快约 1.6 倍，见文件头表格）。
/// 8 MiB 覆盖本项目 3 码方案（`32^3 × 48 B ≈ 1.5 MB`）与简码各级别桶，
/// 同时把 4 码方案（`32^4 × 48 B ≈ 48 MB`）挡在门外。
pub const DENSE_BUDGET_BYTES: usize = 8 << 20;

/// 稀疏后端的「每字条目开销」相对系数（判据用）。
///
/// 稀疏后端的非空桶数恒 ≤ n_chars，故其内存上界 ≈ `n_chars × 每条目开销`。
/// 每条目含哈希表 entry（key u32 + 桶结构）与负载因子（~0.85）的开销，
/// 折合约 `SPARSE_ENTRY_FACTOR` 个密集槽位。取 4 是保守估计：宁可多走 Dense，
/// 也不在占用率本就不低时误判为 Sparse（哈希查找比数组索引慢）。
pub const SPARSE_ENTRY_FACTOR: usize = 4;

/// 后端类型（用于日志与测试观测）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Dense,
    Sparse,
}

/// 后端选择的理由（日志用）。
///
/// 与 [`choose_backend_detailed`] 同源返回，使日志不必自己重算判据——
/// 否则判据一改、日志就容易说着旧理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendReason {
    /// 容量超绝对上限 `SPARSE_THRESHOLD`
    OverAbsoluteLimit,
    /// `capacity × elem_size` 在 [`DENSE_BUDGET_BYTES`] 预算内
    WithinDenseBudget,
    /// 超预算，且槽数远超非空桶上界（稀疏能省下大量内存）
    SparseSavesMemory,
    /// 超预算，但占用率足够高（稀疏省不了多少，Dense 更快）
    OccupancyHigh,
}

/// 依据容量上界选择后端（纯函数，便于测试，需求 5.1/12.2）。
///
/// 只按容量判断，**不含字数维度**；生产路径请用 [`choose_backend_for`]，
/// 它能把「大容量 + 少字数」的稀疏场景正确判成 Sparse。
#[inline]
#[allow(dead_code)] // 生产走 choose_backend_for；此函数保留给 new_with_threshold 与阈值边界测试
pub fn choose_backend_with_threshold(capacity: usize, threshold: usize) -> BackendKind {
    if capacity <= threshold {
        BackendKind::Dense
    } else {
        BackendKind::Sparse
    }
}

/// 生产用后端选择：容量上界、Dense 绝对占用、实际字数三者联合判定（纯函数，便于测试）。
///
/// 判据与理由见文件头：容量超绝对上限 → Sparse；Dense 占用在预算内 → Dense；
/// 否则再看占用率（槽数 vs 非空桶上界 `n_chars × SPARSE_ENTRY_FACTOR`）。
///
/// `elem_size` 传 `size_of::<B>()`：同一份 `capacity` 在两个桶类型下的字节占用不同，
/// 用槽数当阈值会把「SimpleBucket 的小桶」和「FullBucket 的大桶」混为一谈。
#[inline]
pub fn choose_backend_for(capacity: usize, n_chars: usize, elem_size: usize) -> BackendKind {
    choose_backend_detailed(capacity, n_chars, elem_size).0
}

/// 同 [`choose_backend_for`]，但额外返回选择理由（供日志解释，不做二次判断）。
#[inline]
pub fn choose_backend_detailed(
    capacity: usize,
    n_chars: usize,
    elem_size: usize,
) -> (BackendKind, BackendReason) {
    // 绝对上限：容量过大时 Dense 无论如何都吃不下。
    if capacity > SPARSE_THRESHOLD {
        return (BackendKind::Sparse, BackendReason::OverAbsoluteLimit);
    }
    // 占用在预算内：数组索引比哈希查找快，主循环收益远大于省下的这点内存。
    if capacity.saturating_mul(elem_size) <= DENSE_BUDGET_BYTES {
        return (BackendKind::Dense, BackendReason::WithinDenseBudget);
    }
    // 超预算 + 槽数远超可能的非空桶数：稀疏化省内存且改善局部性。
    if capacity > n_chars.saturating_mul(SPARSE_ENTRY_FACTOR) {
        (BackendKind::Sparse, BackendReason::SparseSavesMemory)
    } else {
        (BackendKind::Dense, BackendReason::OccupancyHigh)
    }
}

// 测试用：线程局部阈值覆盖，使 `BucketStore::new`（含 Evaluator/SimpleEvaluator 内部构造）
// 可被强制走稀疏后端，从而在小 code_space 的属性测试中覆盖稀疏路径（生产不编译）。
#[cfg(test)]
thread_local! {
    static TEST_THRESHOLD: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
pub(crate) fn test_threshold_override() -> Option<usize> {
    TEST_THRESHOLD.with(|c| c.get())
}
#[cfg(test)]
pub(crate) fn set_test_threshold_override(v: Option<usize>) {
    TEST_THRESHOLD.with(|c| c.set(v));
}

/// 桶元素需实现「是否为空」判定，使存储层能在两种后端下统一识别非空桶
/// （密集后端遍历/读取时据此跳过空槽；稀疏后端据此维持有界不变量）。
pub trait Bucket: Default + Clone {
    /// 该桶是否为空（无成员）。
    fn is_empty(&self) -> bool;

    /// 原地重置为空桶，**保留已分配的成员容量**（不释放底层缓冲）。
    ///
    /// 供密集后端的 `remove` 复用槽位：清空成员而非以 `Default` 替换，从而避免
    /// 桶空↔非空反复转换时反复 `free`/`malloc` 成员缓冲（退火热路径的主要分配抖动来源）。
    /// 重置后 `is_empty()` 必须为真。
    fn reset(&mut self);
}

enum Backend<B: Bucket> {
    /// 按 `code` 直接索引、大小 = capacity（等价于基线实现）。
    Dense(Vec<B>),
    /// 仅存非空桶；缺失键等价于空桶（需求 2.4）。
    Sparse(FxHashMap<u32, B>),
}

/// 统一桶存储：对调用方屏蔽密集/稀疏后端（需求 5.5）。
pub struct BucketStore<B: Bucket> {
    backend: Backend<B>,
    kind: BackendKind,
}

impl<B: Bucket> BucketStore<B> {
    #![allow(dead_code)] // backend_kind/nonempty_count 等访问器仅测试/日志用
    /// 按默认判据构建（需求 5.1-5.4）。
    ///
    /// `n_chars` 参与后端选择：稀疏后端的非空桶数恒 ≤ 它，故「大容量 + 少字数」应走稀疏
    /// （见 [`choose_backend_for`]）。传 `usize::MAX` 等价于只看容量。
    pub fn new(capacity: usize, n_chars: usize) -> Self {
        // 测试可经线程局部覆盖阈值以强制走稀疏/密集后端（生产编译时整段移除，零开销）。
        #[cfg(test)]
        {
            if let Some(t) = test_threshold_override() {
                return Self::new_with_threshold(capacity, t);
            }
        }
        // 用本桶类型真实的元素大小参与判据：Dense 的占用是 `capacity × size_of::<B>()`。
        Self::new_by_kind(
            capacity,
            choose_backend_for(capacity, n_chars, std::mem::size_of::<B>()),
        )
    }

    /// 按指定阈值构建（**测试专用**：强制走稀疏/密集路径以覆盖两后端一致性）。
    ///
    /// 生产路径走 [`BucketStore::new`] + [`choose_backend_for`]（容量 + 字数联合判据）。
    #[allow(dead_code)] // 仅 #[cfg(test)] 与单测调用；生产编译时无调用者
    pub fn new_with_threshold(capacity: usize, threshold: usize) -> Self {
        Self::new_by_kind(capacity, choose_backend_with_threshold(capacity, threshold))
    }

    /// 按已定的后端类型构建存储本体。
    fn new_by_kind(capacity: usize, kind: BackendKind) -> Self {
        let backend = match kind {
            BackendKind::Dense => Backend::Dense(vec![B::default(); capacity]),
            BackendKind::Sparse => Backend::Sparse(FxHashMap::default()),
        };
        Self { backend, kind }
    }

    /// 所选后端类型（日志/测试用）。
    #[inline]
    pub fn backend_kind(&self) -> BackendKind {
        self.kind
    }

    /// 为稀疏后端预留容纳 `n` 个非空桶的容量（密集后端为 no-op）。
    ///
    /// 稀疏后端非空桶数恒 ≤ n_chars，构造后按 n_chars 预留可：
    /// (1) 避免退火期反复 rehash（插入/删除churn 触发的扩容）；
    /// (2) 降低负载因子、缩短探测序列，减少 `get_mut_or_insert`/`get_mut` 的缓存缺失。
    /// 这是 `update_char` 在 max_parts=5（稀疏）场景下的主要单步开销来源（需求：性能）。
    #[inline]
    pub fn reserve_nonempty(&mut self, n: usize) {
        if let Backend::Sparse(m) = &mut self.backend {
            m.reserve(n);
        }
    }

    /// 只读取非空桶；空桶（密集空槽或稀疏缺失键）返回 `None`（需求 2.4）。
    ///
    /// 不变量「`Some` ⟺ 非空」在两种后端下一致成立，使 `is_code_blocked` 等判定
    /// 可统一写为 `get(code).is_some()`（需求 9.1）。
    #[inline]
    pub fn get(&self, code: u32) -> Option<&B> {
        match &self.backend {
            Backend::Dense(v) => {
                let b = &v[code as usize];
                if b.is_empty() {
                    None
                } else {
                    Some(b)
                }
            }
            Backend::Sparse(m) => m.get(&code),
        }
    }

    /// 可变取非空桶；空桶返回 `None`。
    #[inline]
    pub fn get_mut(&mut self, code: u32) -> Option<&mut B> {
        match &mut self.backend {
            Backend::Dense(v) => {
                let b = &mut v[code as usize];
                if b.is_empty() {
                    None
                } else {
                    Some(b)
                }
            }
            Backend::Sparse(m) => m.get_mut(&code),
        }
    }

    /// 取该 `code` 的桶可变引用，必要时以空桶（`Default`）插入（需求 1.1）。
    ///
    /// 调用方负责在操作后若桶变空则调用 `remove`，以维持有界不变量（需求 2.1）。
    #[inline]
    pub fn get_mut_or_insert(&mut self, code: u32) -> &mut B {
        match &mut self.backend {
            Backend::Dense(v) => &mut v[code as usize],
            Backend::Sparse(m) => m.entry(code).or_default(),
        }
    }

    /// 访问**已存在**的桶并就地修改；回调返回后若桶变空则移除——稀疏后端单次哈希查找
    /// 即完成「访问 + 条件移除」，省去「`get_mut` 再 `remove`」的第二次查找（退火热路径）。
    ///
    /// 调用方须保证 `code` 当前非空（否则 panic）。密集后端：原地修改并在空时 `reset()`
    /// （保留容量）；稀疏后端：用 `Entry` 在同一查找内修改并按需删除条目（维持有界不变量）。
    #[inline]
    pub fn modify_existing_remove_if_empty<R>(
        &mut self,
        code: u32,
        f: impl FnOnce(&mut B) -> R,
    ) -> R {
        match &mut self.backend {
            Backend::Dense(v) => {
                let b = &mut v[code as usize];
                let r = f(b);
                if b.is_empty() {
                    b.reset();
                }
                r
            }
            Backend::Sparse(m) => match m.entry(code) {
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    let r = f(e.get_mut());
                    if e.get().is_empty() {
                        e.remove();
                    }
                    r
                }
                std::collections::hash_map::Entry::Vacant(_) => {
                    panic!("modify_existing_remove_if_empty: 桶 {code} 不存在（应为非空）")
                }
            },
        }
    }

    /// 移除桶。
    ///
    /// - 密集后端：原地 `reset()` 清空成员（**保留容量**，不释放缓冲）且不缩容——避免桶
    ///   空↔非空反复转换造成的 `free`/`malloc` 抖动（需求 2.2 + 性能）。重置后 `get` 返回 `None`。
    /// - 稀疏后端：删除条目以维持「条目 ⟺ 非空桶」有界不变量（需求 2.1）；成员若为内联
    ///   小桶（≤2 成员，在 `Members` 内联容量内）则无堆缓冲可释放，亦无抖动。
    #[inline]
    pub fn remove(&mut self, code: u32) {
        match &mut self.backend {
            Backend::Dense(v) => v[code as usize].reset(),
            Backend::Sparse(m) => {
                m.remove(&code);
            }
        }
    }

    /// 遍历全部非空桶（需求 7）。密集后端跳过空槽，稀疏后端直接迭代。
    ///
    /// 仅供测试/非热路径使用（返回 `Box<dyn>`）。生产代码请用 `for_each_nonempty`
    /// （单态化、可内联、无堆分配与虚调用），以保证密集后端构建/对账路径零性能回归。
    pub fn iter_nonempty(&self) -> Box<dyn Iterator<Item = (u32, &B)> + '_> {
        match &self.backend {
            Backend::Dense(v) => Box::new(
                v.iter()
                    .enumerate()
                    .filter(|(_, b)| !b.is_empty())
                    .map(|(i, b)| (i as u32, b)),
            ),
            Backend::Sparse(m) => Box::new(m.iter().map(|(&c, b)| (c, b))),
        }
    }

    /// 对每个非空桶调用 `f(code, &bucket)`（单态化、可内联、无 `Box`/虚调用）。
    /// 生产构建/对账等非热路径用此遍历，确保密集后端与基线 `for code in 0..cs` 同等性能。
    #[inline]
    pub fn for_each_nonempty(&self, mut f: impl FnMut(u32, &B)) {
        match &self.backend {
            Backend::Dense(v) => {
                for (i, b) in v.iter().enumerate() {
                    if !b.is_empty() {
                        f(i as u32, b);
                    }
                }
            }
            Backend::Sparse(m) => {
                for (&c, b) in m.iter() {
                    f(c, b);
                }
            }
        }
    }

    /// 当前非空桶数（观测/测试用，需求 2.3）。
    pub fn nonempty_count(&self) -> usize {
        match &self.backend {
            Backend::Dense(v) => v.iter().filter(|b| !b.is_empty()).count(),
            Backend::Sparse(m) => m.len(),
        }
    }
}

/// 合并后的全码桶状态（取代基线的 `code_to_chars`/`bucket_freq_sum`/
/// `bucket_max_freq`/`bucket_first` 四个 `code_space` 大小数组，需求 1.2）。
///
/// 成员用 `u32` 表示汉字下标 `ci`（n_chars 远小于 `u32::MAX`，需求 6.1）。
#[derive(Clone)]
pub struct FullBucket {
    /// 桶成员（汉字下标 ci）。内联小向量，≤2 成员不触碰堆。
    pub members: Members,
    /// 桶频率和。
    pub freq_sum: u64,
    /// 桶内最大频率。
    pub max_freq: u64,
    /// 桶首选字 ci；空桶为 `u32::MAX`（需求 2.4）。
    pub first: u32,
}

impl Default for FullBucket {
    #[inline]
    fn default() -> Self {
        Self {
            members: Members::new(),
            freq_sum: 0,
            max_freq: 0,
            first: u32::MAX,
        }
    }
}

impl Bucket for FullBucket {
    #[inline]
    fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    #[inline]
    fn reset(&mut self) {
        self.members.clear(); // 保留容量，不释放
        self.freq_sum = 0;
        self.max_freq = 0;
        self.first = u32::MAX;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试用最小桶：成员计数即可判空。
    #[derive(Clone, Default)]
    struct TestBucket {
        members: Vec<u32>,
        freq_sum: u64,
    }
    impl Bucket for TestBucket {
        fn is_empty(&self) -> bool {
            self.members.is_empty()
        }
        fn reset(&mut self) {
            self.members.clear();
            self.freq_sum = 0;
        }
    }

    #[test]
    fn choose_backend_threshold_boundary() {
        // 纯阈值判据（`new_with_threshold` 用的那条）：容量 ≤ 阈值 → Dense，＞ 阈值 → Sparse。
        let by_cap = |c: usize| choose_backend_with_threshold(c, SPARSE_THRESHOLD);
        assert_eq!(by_cap(SPARSE_THRESHOLD), BackendKind::Dense);
        assert_eq!(by_cap(SPARSE_THRESHOLD + 1), BackendKind::Sparse);
        // 运行时容量（不写死 max_parts）：32^4 在阈值内、32^5 超阈值。
        let code_base = 32usize;
        assert_eq!(by_cap(code_base.pow(4)), BackendKind::Dense);
        assert_eq!(by_cap(code_base.pow(5)), BackendKind::Sparse);
    }

    // 生产判据：容量上限、Dense 绝对占用、字数三者共同决定后端。
    // 关键回归 1：4 码定长方案（code_space = 32^4 = 1,048,576，n_chars ≈ 1.1 万）必须走 Sparse，
    // 否则每评估器预分配 1,048,576 × 48 B ≈ 48 MB，而实际非空桶只有 ~1.1 万个（利用率 1%）。
    // 关键回归 2：3 码方案（32^3 = 32,768 → 约 1.5 MB）必须走 Dense——旧判据按占用率
    // 把它判成 Sparse，实测 SA 吞吐从 11.0 万步/秒掉到 7.0 万步/秒。
    #[test]
    fn choose_backend_for_considers_bytes_and_n_chars() {
        let code_base = 32usize;
        // 用真实桶大小，断言随结构体变化自动调整。
        let elem = std::mem::size_of::<FullBucket>();

        // 4 码 + 1.1 万字：约 48 MB 超预算，且槽数远超非空桶上界 11,177×4 = 44,708 → Sparse
        assert_eq!(
            choose_backend_for(code_base.pow(4), 11_177, elem),
            BackendKind::Sparse,
            "4 码定长 + 1.1 万字应走稀疏后端（否则 48 MB/评估器）"
        );
        // 3 码 + 1.1 万字：约 1.5 MB 在预算内 → Dense（数组索引比哈希快）
        assert_eq!(
            choose_backend_for(code_base.pow(3), 11_177, elem),
            BackendKind::Dense,
            "3 码方案占用在预算内，应走密集后端（走 Sparse 会损失约 36% SA 吞吐）"
        );
        // 预算边界：恰好等于预算 → Dense；超 1 字节 → 转看占用率
        let 预算容量 = DENSE_BUDGET_BYTES / elem;
        assert_eq!(choose_backend_for(预算容量, 1, elem), BackendKind::Dense);
        assert_eq!(
            choose_backend_for(预算容量 + 1, 1, elem),
            BackendKind::Sparse,
            "超预算且几乎无占用 → 稀疏"
        );
        // 小容量无条件 Dense（简码各级别桶落在这里）
        assert_eq!(choose_backend_for(1, 1, 4096), BackendKind::Dense);
        assert_eq!(choose_backend_for(1, usize::MAX, 4096), BackendKind::Dense);
        // 绝对上限优先：容量超上限时即便字数极多也走 Sparse（饱和乘法不溢出）
        assert_eq!(
            choose_backend_for(SPARSE_THRESHOLD + 1, usize::MAX / 8, 48),
            BackendKind::Sparse
        );
        // 超预算但占用率很高（字数足够多）→ Dense：稀疏省不了内存，白白多付哈希开销
        assert_eq!(
            choose_backend_for(code_base.pow(4), 1_000_000, elem),
            BackendKind::Dense
        );
    }

    // Feature: sparse-bucket-memory-optimization, Property 1: 空桶语义一致
    // 未写入或已移除的桶经 get 返回 None（两后端一致）。
    #[test]
    fn prop1_empty_bucket_semantics_both_backends() {
        for &threshold in &[0usize, usize::MAX] {
            let mut s: BucketStore<TestBucket> = BucketStore::new_with_threshold(16, threshold);
            // 从未写入 → None
            assert!(s.get(3).is_none());
            // 写入后非空 → Some
            s.get_mut_or_insert(3).members.push(7);
            assert!(s.get(3).is_some());
            // 移除后 → None
            s.remove(3);
            assert!(s.get(3).is_none());
        }
    }

    // Feature: sparse-bucket-memory-optimization, Property 2: 稀疏有界性
    // 任意操作序列后，非空桶数 == 实际非空桶集合大小（稀疏后端无残留空条目）。
    #[test]
    fn prop2_sparse_bounded_no_empty_residue() {
        let cap = 64usize;
        let mut sparse: BucketStore<TestBucket> = BucketStore::new_with_threshold(cap, 0);
        assert_eq!(sparse.backend_kind(), BackendKind::Sparse);
        // 插入若干，再清空其中一部分
        for code in 0..20u32 {
            sparse.get_mut_or_insert(code).members.push(code);
        }
        for code in 0..8u32 {
            let b = sparse.get_mut(code).unwrap();
            b.members.clear();
            if b.is_empty() {
                sparse.remove(code);
            }
        }
        assert_eq!(sparse.nonempty_count(), 12);
        // 非空桶数不超过曾插入的不同 code 数（有界，需求 2.3）
        assert!(sparse.nonempty_count() <= 20);
    }

    // Feature: sparse-bucket-memory-optimization, Property 3: 双后端可观察等价
    // 对同一操作序列，Dense 与 Sparse 的 iter_nonempty 结果集合一致。
    #[test]
    fn prop3_dense_sparse_observable_equivalence() {
        let cap = 128usize;
        let mut dense: BucketStore<TestBucket> = BucketStore::new_with_threshold(cap, usize::MAX);
        let mut sparse: BucketStore<TestBucket> = BucketStore::new_with_threshold(cap, 0);
        assert_eq!(dense.backend_kind(), BackendKind::Dense);
        assert_eq!(sparse.backend_kind(), BackendKind::Sparse);

        // 伪随机操作序列（确定性）
        let ops: &[(u32, u64)] = &[
            (5, 10),
            (5, 20),
            (9, 3),
            (40, 100),
            (5, 0),
            (9, 7),
            (127, 1),
            (40, 2),
        ];
        for &(code, v) in ops {
            for st in [&mut dense, &mut sparse] {
                let b = st.get_mut_or_insert(code);
                b.members.push(v as u32);
                b.freq_sum += v;
            }
        }
        // 清空 code 9 并移除
        for st in [&mut dense, &mut sparse] {
            let b = st.get_mut(9).unwrap();
            b.members.clear();
            b.freq_sum = 0;
            st.remove(9);
        }

        let mut d: Vec<(u32, Vec<u32>, u64)> = dense
            .iter_nonempty()
            .map(|(c, b)| (c, b.members.clone(), b.freq_sum))
            .collect();
        let mut s: Vec<(u32, Vec<u32>, u64)> = sparse
            .iter_nonempty()
            .map(|(c, b)| (c, b.members.clone(), b.freq_sum))
            .collect();
        d.sort_by_key(|t| t.0);
        s.sort_by_key(|t| t.0);
        assert_eq!(d, s);
        assert_eq!(dense.nonempty_count(), sparse.nonempty_count());
    }

    #[test]
    fn full_bucket_default_is_empty_sentinel() {
        let b = FullBucket::default();
        assert!(b.is_empty());
        assert_eq!(b.first, u32::MAX);
        assert_eq!(b.freq_sum, 0);
        assert_eq!(b.max_freq, 0);
    }

    // 内存估算不变量：SmallVec<[u32;4]> 与 Vec<u32> 同为 24 字节，FullBucket 仍 48 字节，
    // 故后端日志 `capacity × size_of::<FullBucket>()` 的内存估算不因内联化而改变。
    #[test]
    fn full_bucket_size_unchanged_48_bytes() {
        assert_eq!(
            std::mem::size_of::<Members>(),
            24,
            "Members(SmallVec<[u32;2]>) 应为 24 字节"
        );
        assert_eq!(
            std::mem::size_of::<FullBucket>(),
            48,
            "FullBucket 应保持 48 字节，内存估算不变"
        );
    }

    // remove 后 Dense 槽位保留成员容量（不释放），消除空↔非空转换的分配抖动。
    #[test]
    fn dense_remove_retains_member_capacity() {
        let mut s: BucketStore<FullBucket> = BucketStore::new_with_threshold(16, usize::MAX);
        assert_eq!(s.backend_kind(), BackendKind::Dense);
        // 撑大某桶到溢出堆（>4 成员），记录其容量
        {
            let b = s.get_mut_or_insert(3);
            for i in 0..8u32 {
                b.members.push(i);
            }
        }
        let cap_before = {
            let b = s.get(3).unwrap();
            b.members.capacity()
        };
        assert!(cap_before >= 8);
        // remove 后该桶为空，但容量保留（reset 而非 Default），再插入复用
        s.remove(3);
        assert!(s.get(3).is_none(), "remove 后应判空");
        let cap_after = s.get_mut_or_insert(3).members.capacity();
        assert_eq!(
            cap_after, cap_before,
            "Dense remove 应原地重置并保留容量，不释放缓冲"
        );
    }
}
