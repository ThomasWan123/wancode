//! Work 锚点 wire 契约与解析（v0.20 W3-a，设计稿 §1.2）。
//!
//! 锚点把「模型引用的一段文字」钉回**原件里的精确位置**。它是 Work 层
//! 「引用可回源」的地基，也是 G24 电池的机器可断言形态。
//!
//! # 坐标系（定死，消除跨运行时歧义）
//!
//! - **偏移单位 = UTF-16 code unit**：前端 JS 的原生单位，查看器高亮无需换算；
//! - **区间 = 半开、0 基**：`[start, end)`；
//! - **寻址空间 = `raw`**：即**原始抽取文本**，不是归一化文本。
//!
//! 最后一条是关键：归一化（NFC + 空白折叠）**只**用于 excerpt 的相等性判定，
//! **绝不**参与定位——否则「归一改变长度」会让高亮整体错位。两件事分开，
//! 定位永远在 raw 空间。
//!
//! # fail-closed
//!
//! `source_sha256` 失配 / locator 越界 / 区间落在代理对中间 / 归一后摘录不符
//! → 一律「来源已失效」，**不做近似指向**。宁可让引用不可点，也不把用户
//! 送到错误的位置——那比没有引用更糟。
//!
//! 本模块只做**契约与解析**，不含任何文件解析（PDF/DOCX 抽取归 W3-b/c，
//! 其解析栈选型待功能面 spike 用真实样本定，见 W1 证据的 NOT-RUN）。

use serde::{Deserialize, Serialize};

use crate::work_staging::ImportId;

/// 锚点 wire 格式版本。形状变更必须 bump 并显式处理旧版本。
pub const CURRENT_ANCHOR_SCHEMA: u32 = 1;

/// excerpt 长度上限（UTF-16 code unit，与偏移单位一致）。
pub const MAX_EXCERPT_UTF16: usize = 500;

/// 文档类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocKind {
    Pdf,
    Docx,
}

/// 定位子：按 kind 必填不同字段（设计 §1.2）。
///
/// `raw_range` 两者都有——它才是**唯一消歧**手段：同页/同块里重复出现的
/// 相同 excerpt，靠 raw_range 区分是第几次出现。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum Locator {
    Pdf {
        page: u32,
        chunk: u32,
        raw_range: [usize; 2],
    },
    Docx {
        /// 块路径，如 `body/p[41]`。
        block_path: String,
        /// 该块内被覆盖的 run 序号（DOCX 会把一句话拆进多个 run）。
        run_ordinals: Vec<u32>,
        raw_range: [usize; 2],
    },
}

impl Locator {
    pub fn raw_range(&self) -> [usize; 2] {
        match self {
            Locator::Pdf { raw_range, .. } | Locator::Docx { raw_range, .. } => *raw_range,
        }
    }

    /// 定位子与文档类型是否自洽（PDF 锚不能带 DOCX 定位子，反之亦然）。
    pub fn matches_kind(&self, kind: DocKind) -> bool {
        matches!(
            (self, kind),
            (Locator::Pdf { .. }, DocKind::Pdf) | (Locator::Docx { .. }, DocKind::Docx)
        )
    }
}

/// 锚点。`import_id` + 完整 `source_sha256` 联合定位文档——重导入同一文件会
/// 铸造新 import_id，旧锚仍指旧导入（不会被静默改指新副本）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub anchor_schema: u32,
    /// 导入身份。用 W2 的严格新类型（自定义 Deserialize 校验固定语法），
    /// 篡改/伪造的 id 在反序列化即被拒（codex W3-a R1-F2：此前是裸 String）。
    pub import_id: ImportId,
    /// 完整 sha256（64 位小写 hex，不截断）。
    pub source_sha256: String,
    pub kind: DocKind,
    /// 坐标系三元组：写进 wire 是为了让消费方**不必猜**，读时严格校验。
    pub offset_unit: String,
    pub range_kind: String,
    pub range_space: String,
    pub locator: Locator,
    /// 展示用摘录（归一形态），≤500 UTF-16 单元。
    pub excerpt: String,
}

/// 坐标系常量（wire 上逐字这三个值，读时不符即拒）。
pub const OFFSET_UNIT_UTF16: &str = "utf16";
pub const RANGE_KIND_HALF_OPEN: &str = "half_open_zero_based";
pub const RANGE_SPACE_RAW: &str = "raw";

impl Anchor {
    /// 以当前契约构造锚点。excerpt 按归一形态存储并截断到上限。
    pub fn new(
        import_id: ImportId,
        source_sha256: impl Into<String>,
        kind: DocKind,
        locator: Locator,
        raw_excerpt: &str,
    ) -> Self {
        Self {
            anchor_schema: CURRENT_ANCHOR_SCHEMA,
            import_id,
            source_sha256: source_sha256.into(),
            kind,
            offset_unit: OFFSET_UNIT_UTF16.into(),
            range_kind: RANGE_KIND_HALF_OPEN.into(),
            range_space: RANGE_SPACE_RAW.into(),
            locator,
            excerpt: truncate_utf16(&normalize_for_equality(raw_excerpt), MAX_EXCERPT_UTF16),
        }
    }
}

/// 解析失败的结构化原因。前端据此显示「来源已失效」并**禁用点击**
/// （设计验收：无锚引用不可点击）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum AnchorError {
    /// wire 版本不认识。
    UnsupportedSchema { found: u32, supported: u32 },
    /// 坐标系声明与本实现不符（例如别的运行时写了 utf8 偏移）。
    CoordinateSystemMismatch { field: &'static str, found: String },
    /// 定位子与 kind 不自洽。
    LocatorKindMismatch,
    /// 文档哈希与锚点记录的不符——原件已被替换/重导入。
    SourceHashMismatch { expected: String, actual: String },
    /// 区间非法（start > end）或越界。
    RangeOutOfBounds { range: [usize; 2], doc_utf16_len: usize },
    /// 区间端点落在 UTF-16 代理对中间（星平面字符被劈开）。
    RangeSplitsSurrogatePair { offset: usize },
    /// 归一后摘录与该区间的实际文本不符——映射已失效，不近似指向。
    ExcerptMismatch { expected: String, actual: String },
    /// `source_sha256` 不是 64 位小写 hex——形状都不对，不进入比较。
    MalformedSourceHash { found: String },
}

impl std::fmt::Display for AnchorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnchorError::UnsupportedSchema { found, supported } => {
                write!(f, "锚点 schema {found} 不受支持（当前 {supported}）")
            }
            AnchorError::CoordinateSystemMismatch { field, found } => {
                write!(f, "坐标系字段 {field} 不符：{found}")
            }
            AnchorError::LocatorKindMismatch => write!(f, "定位子与文档类型不符"),
            AnchorError::SourceHashMismatch { .. } => write!(f, "来源已失效：原件哈希不符"),
            AnchorError::RangeOutOfBounds { range, doc_utf16_len } => write!(
                f,
                "来源已失效：区间 [{}, {}) 越界（文档长度 {doc_utf16_len}）",
                range[0], range[1]
            ),
            AnchorError::RangeSplitsSurrogatePair { offset } => {
                write!(f, "来源已失效：偏移 {offset} 劈开了代理对")
            }
            AnchorError::ExcerptMismatch { .. } => write!(f, "来源已失效：摘录与原文不符"),
            AnchorError::MalformedSourceHash { found } => {
                write!(f, "锚点 source_sha256 形状非法：{found}")
            }
        }
    }
}
impl std::error::Error for AnchorError {}

/// 相等性判定用的归一化：连续空白折叠为单空格 + 首尾去空白。
///
/// **只用于比较，不用于定位**（定位在 raw 空间，见模块头）。CRLF 属于
/// 「连续空白」，折叠后与 LF 等价——这正是跨运行时用例要覆盖的。
///
/// # 已知缺口：NFC 尚未实现
///
/// 设计 §1.2 要求相等性判定为 **NFC + 空白折叠**；此处**只做空白折叠**。
/// NFC 需要 `unicode-normalization` crate，而 wancode 是 grok-build
/// workspace 成员、共用其 `Cargo.lock`；实测该 lock 与一次全新解析相差
/// 2283 增 / 152 删——**与本改动无关的既有漂移**（不加任何依赖、仅跑
/// `cargo metadata --offline` 即可复现）。为一个 crate 把这堆无关 churn
/// 拖进 PR 不可评审，故 NFC 待 lock 漂移单独裁决后再补。
///
/// **后果（必须知情）**：组合字符等价（`e`+U+0301 vs U+00E9）当前**不**成立。
/// 补上 NFC 会改变相等性语义，届时必须 bump [`CURRENT_ANCHOR_SCHEMA`]，
/// 因为此前铸造的 excerpt 是非 NFC 形态。
pub fn normalize_for_equality(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    out.trim().to_string()
}

/// 按 UTF-16 单元截断（不劈开代理对：宁可少一个字符也不产生半个）。
fn truncate_utf16(s: &str, max_units: usize) -> String {
    let mut units = 0usize;
    let mut out = String::new();
    for ch in s.chars() {
        let w = ch.len_utf16();
        if units + w > max_units {
            break;
        }
        out.push(ch);
        units += w;
    }
    out
}

/// 64 位小写 hex 判定（sha256 的形状）。
fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// 文本的 UTF-16 长度（code unit 数）。
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// 按 UTF-16 半开区间切片。端点必须落在字符边界上——落在代理对中间即
/// fail-closed（返回 Err），绝不返回半个字符或就近取整。
pub fn utf16_slice(s: &str, start: usize, end: usize) -> Result<String, AnchorError> {
    let total = utf16_len(s);
    if start > end || end > total {
        return Err(AnchorError::RangeOutOfBounds {
            range: [start, end],
            doc_utf16_len: total,
        });
    }
    let mut units = 0usize;
    let mut out = String::new();
    let mut started = false;
    for ch in s.chars() {
        let w = ch.len_utf16();
        // 端点落在本字符内部（w == 2 的星平面字符）→ 劈开了代理对。
        if !started {
            if units == start {
                started = true;
            } else if units < start && start < units + w {
                return Err(AnchorError::RangeSplitsSurrogatePair { offset: start });
            }
        }
        if started {
            if units == end {
                return Ok(out);
            }
            if units < end && end < units + w {
                return Err(AnchorError::RangeSplitsSurrogatePair { offset: end });
            }
            out.push(ch);
        }
        units += w;
    }
    if started || start == total {
        Ok(out)
    } else {
        Err(AnchorError::RangeOutOfBounds {
            range: [start, end],
            doc_utf16_len: total,
        })
    }
}

/// 解析锚点：对着**原始抽取文本**与文档哈希验证，返回该区间的 raw 文本。
///
/// 全程 fail-closed：任一不符即结构化错误，调用方据此把引用置为不可点击。
pub fn resolve_anchor(
    anchor: &Anchor,
    doc_sha256: &str,
    raw_text: &str,
) -> Result<String, AnchorError> {
    if anchor.anchor_schema != CURRENT_ANCHOR_SCHEMA {
        return Err(AnchorError::UnsupportedSchema {
            found: anchor.anchor_schema,
            supported: CURRENT_ANCHOR_SCHEMA,
        });
    }
    // 坐标系必须逐字相符——别的运行时若写了 utf8 偏移，这里必须拒而不是
    // 「凑合按 utf16 解释」（那正是高亮错位的来源）。
    for (field, actual, expected) in [
        ("offset_unit", anchor.offset_unit.as_str(), OFFSET_UNIT_UTF16),
        ("range_kind", anchor.range_kind.as_str(), RANGE_KIND_HALF_OPEN),
        ("range_space", anchor.range_space.as_str(), RANGE_SPACE_RAW),
    ] {
        if actual != expected {
            return Err(AnchorError::CoordinateSystemMismatch {
                field,
                found: actual.to_string(),
            });
        }
    }
    if !anchor.locator.matches_kind(anchor.kind) {
        return Err(AnchorError::LocatorKindMismatch);
    }
    if !is_sha256_hex(&anchor.source_sha256) {
        return Err(AnchorError::MalformedSourceHash {
            found: anchor.source_sha256.clone(),
        });
    }
    if anchor.source_sha256 != doc_sha256 {
        return Err(AnchorError::SourceHashMismatch {
            expected: anchor.source_sha256.clone(),
            actual: doc_sha256.to_string(),
        });
    }
    let [start, end] = anchor.locator.raw_range();
    let raw = utf16_slice(raw_text, start, end)?;
    // 摘录相等性在**归一形态**下判定；定位已经在 raw 空间完成。
    // **必须逐字相等**（codex W3-a R1-F1）：曾用 `starts_with` 允许「摘录是
    // 切片的前缀」，那是 §1.2 明禁的近似匹配——excerpt="hel" 会验证覆盖
    // "hello" 的区间。构造期对 excerpt 做过 MAX_EXCERPT_UTF16 截断，故这里
    // 对切片施加**同样的截断**后再逐字比较，而不是放宽成前缀。
    let got = truncate_utf16(&normalize_for_equality(&raw), MAX_EXCERPT_UTF16);
    let want = anchor.excerpt.as_str();
    if got != want {
        return Err(AnchorError::ExcerptMismatch {
            expected: want.to_string(),
            actual: got,
        });
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(n: u8) -> String {
        std::iter::repeat_n(format!("{n:02x}"), 32).collect()
    }

    fn pdf_anchor(range: [usize; 2], raw_excerpt: &str) -> Anchor {
        Anchor::new(
            ImportId::mint(),
            sha(0xab),
            DocKind::Pdf,
            Locator::Pdf {
                page: 3,
                chunk: 7,
                raw_range: range,
            },
            raw_excerpt,
        )
    }

    // ── 坐标系与 wire 契约 ────────────────────────────────────────────
    #[test]
    fn wire_shape_round_trips_and_pins_the_coordinate_system() {
        let a = pdf_anchor([0, 5], "hello");
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["anchor_schema"], 1);
        assert_eq!(json["offset_unit"], "utf16");
        assert_eq!(json["range_kind"], "half_open_zero_based");
        assert_eq!(json["range_space"], "raw");
        let back: Anchor = serde_json::from_value(json).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn foreign_coordinate_system_is_rejected_not_reinterpreted() {
        let mut a = pdf_anchor([0, 5], "hello");
        a.offset_unit = "utf8".into();
        assert!(matches!(
            resolve_anchor(&a, &sha(0xab), "hello world"),
            Err(AnchorError::CoordinateSystemMismatch { field: "offset_unit", .. })
        ));
    }

    #[test]
    fn locator_must_match_kind() {
        let mut a = pdf_anchor([0, 5], "hello");
        a.kind = DocKind::Docx; // PDF 定位子 + DOCX kind
        assert!(matches!(
            resolve_anchor(&a, &sha(0xab), "hello"),
            Err(AnchorError::LocatorKindMismatch)
        ));
    }

    // ── fail-closed 四类失配（设计 §1.2）─────────────────────────────
    #[test]
    fn wrong_hash_is_fail_closed() {
        let a = pdf_anchor([0, 5], "hello");
        assert!(matches!(
            resolve_anchor(&a, &sha(0xcd), "hello"),
            Err(AnchorError::SourceHashMismatch { .. })
        ));
    }

    #[test]
    fn out_of_range_is_fail_closed() {
        let a = pdf_anchor([0, 99], "hello");
        assert!(matches!(
            resolve_anchor(&a, &sha(0xab), "hello"),
            Err(AnchorError::RangeOutOfBounds { .. })
        ));
    }

    #[test]
    fn excerpt_mismatch_is_fail_closed_not_approximate() {
        let mut a = pdf_anchor([0, 5], "hello");
        a.excerpt = "goodbye".into();
        assert!(matches!(
            resolve_anchor(&a, &sha(0xab), "hello"),
            Err(AnchorError::ExcerptMismatch { .. })
        ));
    }

    #[test]
    fn unsupported_schema_is_fail_closed() {
        let mut a = pdf_anchor([0, 5], "hello");
        a.anchor_schema = 99;
        assert!(matches!(
            resolve_anchor(&a, &sha(0xab), "hello"),
            Err(AnchorError::UnsupportedSchema { found: 99, .. })
        ));
    }

    // ── 跨运行时定位用例（设计明列）──────────────────────────────────

    #[test]
    fn crlf_and_runs_of_whitespace_fold_only_for_equality_never_for_locating() {
        // raw 里是 CRLF + 多空格；归一后应与单空格版本相等，但**定位仍在
        // raw 空间**——区间必须按 raw 的长度算。
        let raw = "第一行\r\n\r\n  第二行";
        let target = "第一行\r\n\r\n  第二行";
        let end = utf16_len(target);
        let a = pdf_anchor([0, end], target);
        assert_eq!(a.excerpt, "第一行 第二行", "excerpt 存归一形态");
        let got = resolve_anchor(&a, &sha(0xab), raw).expect("应解析成功");
        assert_eq!(got, raw, "返回的是 raw 原文，不是归一文本");
    }

    #[test]
    fn combining_characters_locate_verbatim_nfc_equivalence_is_a_declared_gap() {
        // "é" 的两种写法：预组合 U+00E9 vs e + U+0301（组合尖音符）。
        let decomposed = "cafe\u{0301}";
        let precomposed = "caf\u{e9}";
        // **当前**两者不等——NFC 未实现（见 normalize_for_equality 的缺口说明）。
        // 本断言锁住「已知缺口」而不是假装它不存在：NFC 落地时它会失败，
        // 那正是提醒去 bump anchor_schema 的信号。
        assert_ne!(
            normalize_for_equality(decomposed),
            normalize_for_equality(precomposed),
            "NFC 未实现时两者不应相等；若变为相等说明 NFC 已落地，需 bump schema"
        );
        // 锚点在**分解形态**的原文上：raw 长度是 5 个 UTF-16 单元。
        let a = pdf_anchor([0, utf16_len(decomposed)], decomposed);
        let got = resolve_anchor(&a, &sha(0xab), decomposed).expect("应解析成功");
        assert_eq!(got, decomposed, "定位返回逐字 raw，不做归一替换");
    }

    #[test]
    fn astral_characters_count_as_two_utf16_units() {
        // 𝄞 (U+1D11E) 是星平面字符：UTF-16 里占 2 个 code unit，Rust char 占 1。
        let raw = "a𝄞b";
        assert_eq!(utf16_len(raw), 4, "a(1) + 𝄞(2) + b(1)");
        // 取中间那个星平面字符：raw_range = [1, 3)
        let a = pdf_anchor([1, 3], "𝄞");
        let got = resolve_anchor(&a, &sha(0xab), raw).expect("应解析成功");
        assert_eq!(got, "𝄞");
    }

    #[test]
    fn splitting_a_surrogate_pair_is_fail_closed() {
        // 端点落在 𝄞 中间（offset 2）——必须拒，不得返回半个字符或就近取整。
        let raw = "a𝄞b";
        let a = pdf_anchor([1, 2], "?");
        assert!(matches!(
            resolve_anchor(&a, &sha(0xab), raw),
            Err(AnchorError::RangeSplitsSurrogatePair { offset: 2 })
        ));
        let b = pdf_anchor([2, 4], "?");
        assert!(matches!(
            resolve_anchor(&b, &sha(0xab), raw),
            Err(AnchorError::RangeSplitsSurrogatePair { offset: 2 })
        ));
    }

    #[test]
    fn duplicate_excerpt_on_one_page_is_disambiguated_by_raw_range() {
        // 同一页里同样的文字出现两次——靠 raw_range 精确导航到**第二次**。
        let raw = "总结：合格。中间段落。总结：合格。";
        let first = raw.find("总结：合格。").unwrap();
        let second = raw.rfind("总结：合格。").unwrap();
        assert_ne!(first, second);
        let utf16_of = |byte_idx: usize| utf16_len(&raw[..byte_idx]);
        let start = utf16_of(second);
        let end = start + utf16_len("总结：合格。");
        let a = pdf_anchor([start, end], "总结：合格。");
        let got = resolve_anchor(&a, &sha(0xab), raw).expect("应解析成功");
        assert_eq!(got, "总结：合格。");
        // 关键：区间指向的是第二次出现，不是第一次。
        assert_eq!(start, utf16_of(second));
        assert_ne!(start, utf16_of(first));
    }

    #[test]
    fn docx_split_runs_are_covered_by_one_range() {
        // DOCX 常把一句话拆进多个 run；锚点用 run_ordinals 记录覆盖了哪几个，
        // 但**定位仍靠块内 raw_range**。
        let block_raw = "这句话被拆成三段。";
        let a = Anchor::new(
            ImportId::mint(),
            sha(0xab),
            DocKind::Docx,
            Locator::Docx {
                block_path: "body/p[41]".into(),
                run_ordinals: vec![2, 3, 4],
                raw_range: [0, utf16_len(block_raw)],
            },
            block_raw,
        );
        let got = resolve_anchor(&a, &sha(0xab), block_raw).expect("应解析成功");
        assert_eq!(got, block_raw);
        match &a.locator {
            Locator::Docx { run_ordinals, block_path, .. } => {
                assert_eq!(run_ordinals, &vec![2, 3, 4]);
                assert_eq!(block_path, "body/p[41]");
            }
            _ => panic!("应为 DOCX 定位子"),
        }
    }

    #[test]
    fn anchor_carries_a_strict_import_id_and_rejects_a_forged_one() {
        // codex W3-a R1-F2：此前 import_id 是裸 String，测试只比了两个字面量,
        // 什么也没证明。现在用 W2 的严格新类型——伪造/篡改的 id 在**反序列化**
        // 即被拒。至于「重导入铸新身份、旧锚不跟随」，那需要清单参与，归拥有
        // 清单的那个切片证明，本切片不宣称。
        let a = pdf_anchor([0, 5], "hello");
        let json = serde_json::to_value(&a).unwrap();
        assert!(
            json["import_id"].as_str().unwrap().starts_with("imp-"),
            "wire 上是铸造格式的 import_id"
        );
        // 伪造一个不合语法的 id → 整个锚点反序列化失败（fail-closed）。
        let mut forged = json.clone();
        forged["import_id"] = serde_json::Value::String("imp-../../escape".into());
        assert!(
            serde_json::from_value::<Anchor>(forged).is_err(),
            "非法 import_id 必须让锚点反序列化失败"
        );
        // 两次铸造互不相同（身份确实是新铸的，不是常量）。
        assert_ne!(ImportId::mint(), ImportId::mint());
    }

    #[test]
    fn excerpt_is_capped_without_splitting_surrogate_pairs() {
        let long: String = std::iter::repeat_n('𝄞', 400).collect(); // 800 UTF-16 单元
        let a = pdf_anchor([0, 800], &long);
        assert!(utf16_len(&a.excerpt) <= MAX_EXCERPT_UTF16);
        // 不得产生半个代理对：截断后仍是合法 Rust 字符串且全为完整字符。
        assert!(a.excerpt.chars().all(|c| c == '𝄞'));
    }

    #[test]
    fn empty_range_at_end_is_valid() {
        let raw = "abc";
        let a = pdf_anchor([3, 3], "");
        assert_eq!(resolve_anchor(&a, &sha(0xab), raw).unwrap(), "");
    }

    // codex W3-a R1-F1：**前缀不算相符**。这一类正是旧 starts_with 放过的。
    #[test]
    fn a_proper_prefix_of_the_range_text_is_a_mismatch() {
        let mut a = pdf_anchor([0, 5], "hello");
        a.excerpt = "hel".into(); // 真前缀
        assert!(matches!(
            resolve_anchor(&a, &sha(0xab), "hello"),
            Err(AnchorError::ExcerptMismatch { .. })
        ));
    }

    #[test]
    fn an_empty_excerpt_cannot_resolve_a_non_empty_range() {
        let mut a = pdf_anchor([0, 5], "hello");
        a.excerpt = String::new();
        assert!(matches!(
            resolve_anchor(&a, &sha(0xab), "hello"),
            Err(AnchorError::ExcerptMismatch { .. })
        ));
    }

    #[test]
    fn malformed_source_hash_is_rejected_before_comparison() {
        let mut a = pdf_anchor([0, 5], "hello");
        a.source_sha256 = "NOTAHASH".into();
        assert!(matches!(
            resolve_anchor(&a, "NOTAHASH", "hello"),
            Err(AnchorError::MalformedSourceHash { .. })
        ));
    }
}
