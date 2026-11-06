// =========================================================================
// 🧩 prepare 的方案参数表
// =========================================================================
//
// 对应 `moling/init.sh` 里的 `case "$SCHEMA" in ...)` 分支：每个方案由一组
// 常量构成（编码规则、韵码来源、字频混合权重、键位分档、声韵修正开关……）。
// 这些值是方案的**定义**而非本地可调参数，因此写进代码而不是 TOML；
// 构建链要换方案只需 `--schema <名>`。
//
// 与 bash 版的差异：bash 支持 moling/moqing/xiaoming/yaoling/yueling/yuling
// 六套，这里只实现仓库实际使用与需要对照的三套（魔灵/灵明/星陈）。
// `moqing`（三码）、`xiaoming`（五码）的声码韵码分支未移植，传入会明确报错。
// =========================================================================

/// 单字编码规则（对应 bash 版的 `ENCODE_RULE`）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum 编码规则 {
    /// 魔灵：A1A2A3AzSzYz，二根字回头码 A1A2S2S1Y1
    魔灵,
    /// 灵明：单根 ASY / 多根 A1S1A2A3AzSzYz（≥4 根且首根有声母时跳第三根）
    灵明,
    /// 星陈：只取 A/S 两位（1 根 A1S1S1、2 根 A1A2S2S1、3 根 A1A2A3S3、≥4 根 A1A2A3A末）
    星陈,
}

/// 补码（韵码）来源（对应 bash 版的 `USE_VOWEL` 与方案分支）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum 韵码来源 {
    /// 取字根首笔笔画（`USE_VOWEL=0`），笔画还要按左右声母做手感重映射
    首笔,
    /// 取字根韵腹（`USE_VOWEL=1`），灵明走这条
    韵腹,
    /// 不出补码（星陈只取 A/S）
    无,
}

/// 一套方案的全部生成参数
pub struct 方案 {
    /// 方案名（命令行 `--schema` 的取值）
    pub 名: &'static str,
    /// 方案中文名，仅用于日志
    pub 名称: &'static str,
    pub 编码规则: 编码规则,
    pub 韵码: 韵码来源,
    /// `USE_MIXED_FREQ`：繁体字频权重，0 表示只用简体字频
    pub 混合字频权重: f64,
    /// `MAX_CODE_LEN`
    pub 最大码长: usize,
    /// 字根键位全集。
    ///
    /// 上游脚本原本按根频分三档（`TOP_ROOT_FREQ`/`TOP_ROOT_KEYS`、
    /// `HOT_ROOT_FREQ`/`HOT_ROOT_KEYS`、`ALL_ROOT_KEYS`）给不同频率的字根
    /// 不同的候选键集。现已**取消按根频自动分派**：聚类簇内字根共享一键，
    /// 键位候选一律给全集，交给退火自己挑。
    pub 全部字根键位: &'static str,
    /// 声码（小码）统一候选键位。
    ///
    /// `None` = 沿用各字根拼音声母（现状：声码与读音绑定，逐根钉死）；
    /// `Some(键集)` = 所有 `.S` 约束统一给这组候选键，交给退火在集合内分配
    /// （星陈用这条：声码不再按声母固定，而是在指定键集里统一优化）。
    pub 声码键位: Option<&'static str>,
    /// 是否让**每个字根都有小码槽**（星陈口径）。
    ///
    /// `false`（魔灵/灵明）= 老规矩：声码长度 > 1 才算「有声母」，才给 `.S` 槽；
    /// `true`（星陈）= 不看声母，`声码键位` 里的候选键对所有字根一视同仁，
    /// 查不到读音的花括号部件、符号根同样分到一个小码。
    pub 小码恒有: bool,
    /// `OPTIMIZE_KEY_<声母>` 重映射表
    pub 优化键: &'static [(char, char)],
    /// `OPTIMIZE_KEYS`：交给退火优化、不写死约束的声母集合。
    /// 现有三套方案都是空串（即全部写死），非空时本子命令不支持。
    pub 优化声母集: &'static str,
    /// `USE_PINYIN_DU_FOR_TU`：字根「土」是否取「杜」音 du
    pub 土用杜音: bool,
    /// `USE_PINYIN_VOU_FOR_KOU`：字根「口」是否取 vou 音
    pub 口用vou音: bool,
    /// `USE_STROKE_5_FOR_6`：笔画 6 是否并入 5
    pub 笔画6并入5: bool,
    /// 方案自带的字根表文件名；`None` 表示沿用 `roots.txt`
    pub 字根表: Option<&'static str>,
}

impl 方案 {
    /// `OPTIMIZE_KEY_<声母>` 查表
    pub fn 优化键(&self, 声母: char) -> Option<char> {
        self.优化键
            .iter()
            .find(|(源, _)| *源 == 声母)
            .map(|(_, 目标)| *目标)
    }
}

/// 魔灵（@qq3qq）：仓库默认方案，`moling/config.ron` 用它
pub const 魔灵: 方案 = 方案 {
    名: "moling",
    名称: "魔靈",
    编码规则: 编码规则::魔灵,
    韵码: 韵码来源::首笔,
    混合字频权重: 0.1,
    最大码长: 4,
    全部字根键位: "qwertyuiopasdfghjklxcvbnm",
    声码键位: None,
    小码恒有: false,
    优化键: &[('0', 'w'), ('y', 'k'), ('z', 'v')],
    优化声母集: "",
    土用杜音: true,
    口用vou音: false,
    笔画6并入5: true,
    字根表: None,
};

/// 灵明（@朱宇浩）：`USE_VOWEL=1`，韵码取韵腹，字根表由脚本初始化生成
pub const 灵明: 方案 = 方案 {
    名: "yuling",
    名称: "靈明",
    编码规则: 编码规则::灵明,
    韵码: 韵码来源::韵腹,
    混合字频权重: 1.0,
    最大码长: 4,
    全部字根键位: "qwertyuiopasdfghjklxcvbnm",
    声码键位: None,
    小码恒有: false,
    优化键: &[('0', 'j'), ('q', 'k'), ('z', 'v')],
    优化声母集: "",
    土用杜音: false,
    口用vou音: false,
    笔画6并入5: true,
    字根表: Some("字根声韵-yuling.txt"),
};

/// 星陈（@朱宇浩）：只取大码与小码，不出补码。bash 版没有这套，
/// 取码规则见 `export_table::取码序列`。
pub const 星陈: 方案 = 方案 {
    名: "yustar",
    名称: "星陳",
    编码规则: 编码规则::星陈,
    韵码: 韵码来源::无,
    混合字频权重: 0.1,
    最大码长: 4,
    全部字根键位: "qwertyuiopasdfghjklxcvbnm",
    声码键位: Some("weisdfjklmvnhgur"),
    小码恒有: true,
    优化键: &[],
    优化声母集: "",
    土用杜音: false,
    口用vou音: false,
    笔画6并入5: true,
    字根表: None,
};

/// 按名字取方案
pub fn 取(名: &str) -> Result<&'static 方案, String> {
    match 名 {
        "moling" => Ok(&魔灵),
        "yuling" => Ok(&灵明),
        "yustar" => Ok(&星陈),
        other => Err(format!(
            "未知方案 '{other}'；本子命令支持：moling（魔靈）、yuling（靈明）、yustar（星陳）"
        )),
    }
}
