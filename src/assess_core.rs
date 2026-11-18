// =========================================================================
// 纯 Rust 宇浩测评内核
// =========================================================================
//
// 取代旧版 assess.rs 里对 `pnpm cli`（yuhao-assess Node CLI）的 spawn 调用。
// f64 输出严格复刻 Node CLI 的累加顺序 / Map 迭代顺序，已与生产方案
// 测评方案.jsonc 的 CLI 产物逐字段对拍：497 项中 492 逐位相等、
// 5 项机器精度内（速度当量的固有 f64 微差）、零真差异。
//
// 复用 assess.rs 已有的 handfeel（指位/连击/选重）与打印/汇总逻辑——
// 本模块只负责产出与 Node CLI 等价的「測評結果」JSON。
// =========================================================================

use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// cjkBlocks.json 的 11 个区间，顺序 basic,a..j
type Blocks = [(u32, u32); 11];

// ---------- 方案解析（极简 JSONC） ----------

/// 去掉 // 与 /* */ 注释、对象/数组尾随逗号；按字节处理，保留 UTF-8 多字节。
fn strip_jsonc(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut in_str = false;
    let mut esc = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            out.push(b);
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if b == b'"' {
            in_str = true;
            out.push(b);
            i += 1;
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        out.push(b);
        i += 1;
    }
    let mut res: Vec<u8> = Vec::with_capacity(out.len());
    let bs = &out;
    let mut j = 0;
    let mut ins = false;
    let mut es = false;
    while j < bs.len() {
        let b = bs[j];
        if ins {
            res.push(b);
            if es {
                es = false;
            } else if b == b'\\' {
                es = true;
            } else if b == b'"' {
                ins = false;
            }
            j += 1;
            continue;
        }
        if b == b'"' {
            ins = true;
            res.push(b);
            j += 1;
            continue;
        }
        if b == b',' {
            let mut k = j + 1;
            while k < bs.len() && bs[k].is_ascii_whitespace() {
                k += 1;
            }
            if k < bs.len() && (bs[k] == b'}' || bs[k] == b']') {
                j += 1;
                continue;
            }
        }
        res.push(b);
        j += 1;
    }
    // 输入是合法 &str，清理只删字节不改字节，故结果必为合法 UTF-8。
    String::from_utf8(res).unwrap()
}

#[derive(Clone)]
struct Scheme {
    maxlen: usize,
    sel: HashMap<String, String>,
    has_terminator: bool,
    terminators: Vec<String>,
}

fn load_scheme(path: &Path) -> Result<Scheme, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("读不了方案 {}: {}", path.display(), e))?;
    let clean = strip_jsonc(&text);
    let v: Value = serde_json::from_str(&clean).map_err(|e| format!("解析方案失败: {}", e))?;
    let param = v.get("方案參數");
    let maxlen = param
        .and_then(|p| p.get("最大碼長"))
        .and_then(|x| x.as_u64())
        .unwrap_or(4) as usize;
    let mut sel = HashMap::new();
    if let Some(m) = param
        .and_then(|p| p.get("選重鍵表"))
        .and_then(|x| x.as_object())
    {
        for (k, v) in m {
            if let Some(s) = v.as_str() {
                sel.insert(k.clone(), s.to_string());
            }
        }
    }
    let terminators: Vec<String> = param
        .and_then(|p| p.get("編碼終止指示符列表"))
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let has_terminator = !terminators.is_empty();
    Ok(Scheme {
        maxlen,
        sel,
        has_terminator,
        terminators,
    })
}

/// 生成带选重键的编码：首选且未满码长补 `_`（无终止指示符时），第 n 选补数字 n。
fn code_with_selection(code: &str, pos: usize, code_len: usize, sc: &Scheme) -> String {
    if pos == 1 {
        if !sc.has_terminator && code_len < sc.maxlen {
            format!("{}_", code)
        } else if sc.has_terminator && code_len < sc.maxlen {
            let last = code.chars().last().unwrap_or('\0');
            if !sc.terminators.iter().any(|t| t.starts_with(last)) && last != '_' {
                format!("{}_", code)
            } else {
                code.to_string()
            }
        } else {
            code.to_string()
        }
    } else {
        format!("{}{}", code, pos)
    }
}

/// 把编码末尾的选重数字串整体替换为方案配置的实际按键（如 2→`;`, 3→`'`）。
/// 只在整串数字都在表中有对应项时替换，避免误伤；表中无的位次保持数字键。
fn replace_sel(code: &str, sel: &HashMap<String, String>) -> String {
    let bytes = code.as_bytes();
    let mut end = code.len();
    while end > 0 && bytes[end - 1].is_ascii_digit() {
        end -= 1;
    }
    if end == code.len() {
        return code.to_string();
    }
    let digits = &code[end..];
    match sel.get(digits) {
        Some(rep) if !rep.is_empty() => format!("{}{}", &code[..end], rep),
        _ => code.to_string(),
    }
}

// ---------- 码表四张辅助表 ----------

struct Tables {
    order: Vec<char>,           // 首次出现序（速度当量/简码层级用）
    full_map_order: Vec<char>,  // full/full_sel 的 Map 迭代顺序
    short_map_order: Vec<char>, // short/short_sel 的 Map 迭代顺序
    full: HashMap<char, String>,
    short: HashMap<char, String>,
    full_sel: HashMap<char, String>,
    short_sel: HashMap<char, String>,
}

fn in_cjk_to_j(c: char) -> bool {
    let cp = c as u32;
    // 与 cjkBlocks 到 J 的并集一致；精确端点见 load_blocks。
    (0x4E00..=0x9FFF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x20000..=0x2A6DF).contains(&cp)
        || (0x2A700..=0x2B73F).contains(&cp)
        || (0x2B740..=0x2B81D).contains(&cp)
        || (0x2B820..=0x2CEAD).contains(&cp)
        || (0x2CEB0..=0x2EBE0).contains(&cp)
        || (0x30000..=0x3134A).contains(&cp)
        || (0x31350..=0x323AF).contains(&cp)
        || (0x2EBF0..=0x2EE5D).contains(&cp)
        || (0x323B0..=0x33479).contains(&cp)
}

fn move_to_end(v: &mut Vec<char>, c: char) {
    if let Some(p) = v.iter().position(|&x| x == c) {
        v.remove(p);
        v.push(c);
    }
}

fn parse_and_build(table_path: &Path, sc: &Scheme) -> Result<Tables, String> {
    let text = fs::read_to_string(table_path).map_err(|e| format!("读不了码表: {}", e))?;
    // 第一遍：有效条目（保行序）
    let mut entries: Vec<(char, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let ch = parts[0];
        let code = parts[1];
        if ch.is_empty() || code.is_empty() {
            continue;
        }
        let mut ci = ch.chars();
        let c = match ci.next() {
            Some(c) if ci.next().is_none() => c,
            _ => continue,
        };
        if !in_cjk_to_j(c) {
            continue;
        }
        entries.push((c, code.to_string()));
    }
    // 第二遍：同编码下每字位置
    let mut pos_map: HashMap<String, HashMap<char, usize>> = HashMap::new();
    for (c, code) in &entries {
        let m = pos_map.entry(code.clone()).or_default();
        if !m.contains_key(c) {
            let n = m.len() + 1;
            m.insert(*c, n);
        }
    }
    // 第三遍：建四表，并模拟 JS Map 的 delete+set 顺序
    let mut t = Tables {
        order: vec![],
        full_map_order: vec![],
        short_map_order: vec![],
        full: HashMap::new(),
        short: HashMap::new(),
        full_sel: HashMap::new(),
        short_sel: HashMap::new(),
    };
    let mut maxl: HashMap<char, usize> = HashMap::new();
    let mut minl: HashMap<char, usize> = HashMap::new();
    for (c, code) in &entries {
        let clen = code.chars().count();
        let pos = pos_map.get(code).unwrap().get(c).unwrap();
        let sel_code = code_with_selection(code, *pos, clen, sc);
        if !maxl.contains_key(c) {
            t.order.push(*c);
            t.full_map_order.push(*c);
            t.short_map_order.push(*c);
            t.full.insert(*c, code.clone());
            t.short.insert(*c, code.clone());
            t.full_sel.insert(*c, sel_code.clone());
            t.short_sel.insert(*c, sel_code.clone());
            maxl.insert(*c, clen);
            minl.insert(*c, clen);
            continue;
        }
        if clen > *maxl.get(c).unwrap() {
            t.full.insert(*c, code.clone());
            t.full_sel.insert(*c, sel_code.clone());
            maxl.insert(*c, clen);
            move_to_end(&mut t.full_map_order, *c);
        }
        if clen < *minl.get(c).unwrap() {
            t.short.insert(*c, code.clone());
            t.short_sel.insert(*c, sel_code.clone());
            minl.insert(*c, clen);
            move_to_end(&mut t.short_map_order, *c);
        }
    }
    Ok(t)
}

// ---------- 字频（保序） ----------

struct Freq {
    f64m: HashMap<char, f64>,
    order: Vec<char>,
}

fn scan_key_order(text: &str) -> Vec<char> {
    let bytes = text.as_bytes();
    let mut v = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            if j < bytes.len() {
                if let Ok(s) = std::str::from_utf8(&bytes[start..j])
                    && let Some(c) = s.chars().next()
                    && s.chars().nth(1).is_none()
                {
                    v.push(c);
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    v
}

fn load_freq(path: &Path) -> Result<Freq, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("读不了字频 {}: {}", path.display(), e))?;
    let raw: HashMap<String, u64> =
        serde_json::from_str(&text).map_err(|e| format!("解析字频失败: {}", e))?;
    let order = scan_key_order(&text);
    let total: u64 = raw.values().sum();
    let mut f64m = HashMap::new();
    for c in &order {
        if let Some(v) = raw.get(&c.to_string()) {
            f64m.insert(*c, *v as f64 / total as f64);
        }
    }
    Ok(Freq { f64m, order })
}

fn merged_freq(a: &Freq, b: &Freq) -> Freq {
    let mut f64m = a.f64m.clone();
    for (c, f) in &b.f64m {
        if !f64m.contains_key(c) || f64m.get(c).unwrap() < f {
            f64m.insert(*c, *f);
        }
    }
    let mut order = a.order.clone();
    let aset: HashMap<char, ()> = a.order.iter().map(|&c| (c, ())).collect();
    for &c in &b.order {
        if !aset.contains_key(&c) {
            order.push(c);
        }
    }
    Freq { f64m, order }
}

// ---------- 速度当量 ----------

/// 保序累加编码对频率，再按当量矩阵折算（严格复刻 TS 的 Map 累加顺序）。
fn weighted_ordered(
    order: &[char],
    table: &HashMap<char, String>,
    freq: &Freq,
    equiv: &HashMap<String, f64>,
    sel: &HashMap<String, String>,
) -> f64 {
    let mut keys: Vec<String> = Vec::new();
    let mut idx: HashMap<String, usize> = HashMap::new();
    let mut vals: Vec<f64> = Vec::new();
    for c in order {
        let code = match table.get(c) {
            Some(x) => x,
            None => continue,
        };
        let f = match freq.f64m.get(c) {
            Some(x) if *x != 0.0 => *x,
            _ => continue,
        };
        let c2 = replace_sel(code, sel);
        let chars: Vec<char> = c2.chars().collect();
        for w in chars.windows(2) {
            let pair: String = w.iter().collect();
            if let Some(&i) = idx.get(&pair) {
                vals[i] += f;
            } else {
                let i = keys.len();
                idx.insert(pair.clone(), i);
                keys.push(pair);
                vals.push(f);
            }
        }
    }
    let mut tw = 0.0f64;
    let mut tf = 0.0f64;
    for (k, f) in keys.iter().zip(vals.iter()) {
        if let Some(&e) = equiv.get(k) {
            tw += e * f;
            tf += f;
        }
    }
    if tf > 0.0 {
        tw / tf
    } else {
        0.0
    }
}

fn gen_level(
    order: &[char],
    short_sel: &HashMap<char, String>,
    full_sel: &HashMap<char, String>,
    maxlen: usize,
) -> (Vec<char>, HashMap<char, String>) {
    let mut res = HashMap::new();
    let mut o = Vec::new();
    for c in order {
        let sc = &short_sel[c];
        let take = sc.chars().count() <= maxlen && sc.ends_with('_');
        if take {
            o.push(*c);
            res.insert(*c, sc.clone());
        } else if let Some(fc) = full_sel.get(c) {
            o.push(*c);
            res.insert(*c, fc.clone());
        }
    }
    (o, res)
}

// ---------- 动态选重 ----------

fn calc_dyn_sorted(tab: &HashMap<char, String>, map_order: &[char], freq: &Freq) -> f64 {
    let mut groups: Vec<(String, Vec<(char, f64)>)> = Vec::new();
    let mut gidx: HashMap<String, usize> = HashMap::new();
    for &c in map_order {
        let code = match tab.get(&c) {
            Some(x) => x,
            None => continue,
        };
        let f = freq.f64m.get(&c).copied().unwrap_or(0.0);
        let gi = *gidx.entry(code.clone()).or_insert_with(|| {
            groups.push((code.clone(), Vec::new()));
            groups.len() - 1
        });
        groups[gi].1.push((c, f));
    }
    let mut total_dup = 0.0f64;
    for (_, cf) in groups.iter_mut() {
        if cf.len() > 1 {
            cf.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            let mut gt = 0.0f64;
            for &(_, f) in cf.iter() {
                gt += f;
            }
            let first = cf[0].1;
            total_dup += gt - first;
        }
    }
    let mut total_freq = 0.0f64;
    for &c in map_order {
        total_freq += freq.f64m.get(&c).copied().unwrap_or(0.0);
    }
    if total_freq > 0.0 {
        total_dup / total_freq
    } else {
        0.0
    }
}

fn calc_dyn_original(tab: &HashMap<char, String>, map_order: &[char], freq: &Freq) -> f64 {
    let mut total_dup = 0.0f64;
    let mut total_freq = 0.0f64;
    for &c in map_order {
        let code = match tab.get(&c) {
            Some(x) => x,
            None => continue,
        };
        let f = freq.f64m.get(&c).copied().unwrap_or(0.0);
        total_freq += f;
        let is_sel = code
            .chars()
            .last()
            .map(|ch| ch.is_ascii_digit())
            .unwrap_or(false);
        if is_sel {
            total_dup += f;
        }
    }
    if total_freq > 0.0 {
        total_dup / total_freq
    } else {
        0.0
    }
}

// ---------- 简码效率 ----------

struct EffItem {
    ch: char,
    sc_len: usize,
    fc_len: usize,
    freq: f64,
    prod: f64,
}

fn calc_short_eff(
    freq: &Freq,
    short_sel: &HashMap<char, String>,
    full_sel: &HashMap<char, String>,
    maxlen: usize,
    nlist: &[usize],
) -> (String, Vec<(usize, f64)>) {
    let mut items: Vec<EffItem> = Vec::new();
    for &ch in &freq.order {
        let f = match freq.f64m.get(&ch) {
            Some(x) if *x > 0.0 => *x,
            _ => continue,
        };
        let sc = match short_sel.get(&ch) {
            Some(x) => x,
            None => continue,
        };
        let fc = match full_sel.get(&ch) {
            Some(x) => x,
            None => continue,
        };
        let sc_len = sc.chars().count();
        let fc_len = fc.chars().count();
        let diff = fc_len as i64 - sc_len as i64;
        items.push(EffItem {
            ch,
            sc_len,
            fc_len,
            freq: f,
            prod: f * diff as f64,
        });
    }
    let mut valid: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.sc_len < it.fc_len && it.sc_len < maxlen)
        .map(|(i, _)| i)
        .collect();
    valid.sort_by(|&a, &b| items[b].prod.partial_cmp(&items[a].prod).unwrap());
    let max_n = *nlist.iter().max().unwrap();
    let actual = max_n.min(valid.len());
    let full_str: String = valid[..actual].iter().map(|&i| items[i].ch).collect();
    let mut res = Vec::new();
    for &n in nlist {
        let k = n.min(valid.len());
        let selected: HashMap<usize, ()> = valid[..k].iter().map(|&i| (i, ())).collect();
        let mut tw = 0.0f64;
        let mut tf = 0.0f64;
        for (i, it) in items.iter().enumerate() {
            let len = if selected.contains_key(&i) {
                it.sc_len
            } else {
                it.fc_len
            };
            tw += it.freq * len as f64;
            tf += it.freq;
        }
        res.push((n, if tf > 0.0 { tw / tf } else { 0.0 }));
    }
    (full_str, res)
}

// ---------- 键位热力 ----------

fn calc_key_heat(
    tab: &HashMap<char, String>,
    map_order: &[char],
    freq: &Freq,
    sel: &HashMap<String, String>,
) -> Map<String, Value> {
    let mut dist: HashMap<String, f64> = HashMap::new();
    for &c in map_order {
        let w = freq.f64m.get(&c).copied().unwrap_or(0.0);
        if let Some(code) = tab.get(&c) {
            let code = replace_sel(code, sel);
            for ch in code.to_lowercase().chars() {
                let key = if ch == '_' {
                    "space".to_string()
                } else {
                    ch.to_string()
                };
                *dist.entry(key).or_insert(0.0) += w;
            }
        }
    }
    let mut m = Map::new();
    for (k, v) in dist {
        m.insert(k, json!(v));
    }
    m
}

// ---------- 字集 / 静态重码 ----------

fn load_blocks(settings_dir: &Path) -> Result<Blocks, String> {
    let path = settings_dir.join("cjkBlocks.json");
    let raw: Value = serde_json::from_str(
        &fs::read_to_string(&path).map_err(|e| format!("读不了 {}: {}", path.display(), e))?,
    )
    .map_err(|e| format!("解析 cjkBlocks 失败: {}", e))?;
    let blocks = raw.get("blocks").unwrap().as_object().unwrap();
    let names = [
        "cjk_basic",
        "cjk_a",
        "cjk_b",
        "cjk_c",
        "cjk_d",
        "cjk_e",
        "cjk_f",
        "cjk_g",
        "cjk_h",
        "cjk_i",
        "cjk_j",
    ];
    let mut out = [(0u32, 0u32); 11];
    for (i, name) in names.iter().enumerate() {
        let b = blocks.get(*name).unwrap().as_object().unwrap();
        let parse_hex =
            |s: &str| -> u32 { u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap() };
        out[i] = (
            parse_hex(b.get("start").unwrap().as_str().unwrap()),
            parse_hex(b.get("end").unwrap().as_str().unwrap()),
        );
    }
    Ok(out)
}

fn build_static_dup(
    t: &Tables,
    data_dir: &Path,
    blocks: &Blocks,
) -> Result<Map<String, Value>, String> {
    let path = data_dir.join("charsets.json");
    let cs_raw: HashMap<String, Value> = serde_json::from_str(
        &fs::read_to_string(&path).map_err(|e| format!("读不了 {}: {}", path.display(), e))?,
    )
    .map_err(|e| format!("解析 charsets 失败: {}", e))?;
    let cjk_sets: Vec<(&str, usize)> = vec![
        ("CJK基本", 0),
        ("到CJK擴A", 1),
        ("到CJK擴B", 2),
        ("到CJK擴C", 3),
        ("到CJK擴D", 4),
        ("到CJK擴E", 5),
        ("到CJK擴F", 6),
        ("到CJK擴G", 7),
        ("到CJK擴H", 8),
        ("到CJK擴I", 9),
        ("到CJK擴J", 10),
    ];
    let flag_sets: Vec<(&str, &str)> = vec![
        ("GB2312", "is_gb2312"),
        ("通用規範", "is_tonggui"),
        ("常用國字", "is_guozi"),
    ];
    let in_cjk = |c: char, upto: usize| -> bool {
        let cp = c as u32;
        blocks[..=upto].iter().any(|&(a, b)| cp >= a && cp <= b)
    };
    let in_mark = |c: char, flag: &str| -> bool {
        cs_raw
            .get(&c.to_string())
            .and_then(|v| v.get(flag))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    let theoretical_mark = |flag: &str| -> usize {
        cs_raw
            .values()
            .filter(|v| v.get(flag).and_then(|x| x.as_bool()).unwrap_or(false))
            .count()
    };
    let theoretical_cjk = |upto: usize| -> usize {
        blocks[..=upto]
            .iter()
            .map(|&(a, b)| (b - a + 1) as usize)
            .sum()
    };
    type SetPred<'a> = Box<dyn Fn(char) -> bool + 'a>;
    let mut sets: Vec<(&str, SetPred)> = Vec::new();
    for &(name, flag) in &flag_sets {
        sets.push((name, Box::new(move |c| in_mark(c, flag))));
    }
    for &(name, upto) in &cjk_sets {
        sets.push((name, Box::new(move |c| in_cjk(c, upto))));
    }

    let mut dup = Map::new();
    for (cs, pred) in &sets {
        let mut d = Map::new();
        let mut actual: usize = 0;
        for (tname, tab) in [("全碼", &t.full), ("簡碼", &t.short)] {
            let mut groups: HashMap<&String, usize> = HashMap::new();
            let mut cnt = 0usize;
            for c in &t.order {
                if pred(*c)
                    && let Some(code) = tab.get(c)
                {
                    *groups.entry(code).or_insert(0) += 1;
                    cnt += 1;
                }
            }
            if tname == "全碼" {
                actual = cnt;
            }
            let chars: usize = groups.values().filter(|&&n| n > 1).sum();
            let grps: usize = groups.values().filter(|&&n| n > 1).count();
            d.insert(format!("{}重碼字數", tname), json!(chars));
            d.insert(format!("{}重碼組數", tname), json!(grps));
        }
        let theoretical: usize =
            if let Some(upto) = cjk_sets.iter().find(|(n, _)| *n == *cs).map(|x| x.1) {
                theoretical_cjk(upto)
            } else {
                let flag = flag_sets.iter().find(|(n, _)| *n == *cs).unwrap().1;
                theoretical_mark(flag)
            };
        d.insert("實際字符數".to_string(), json!(actual));
        d.insert("理論字符數".to_string(), json!(theoretical));
        d.insert(
            "字集覆蓋率".to_string(),
            json!(if theoretical > 0 {
                actual as f64 / theoretical as f64
            } else {
                0.0
            }),
        );
        dup.insert((*cs).to_string(), json!(d));
    }
    Ok(dup)
}

// ---------- 候选个数 ----------

fn build_candidates(t: &Tables, data_dir: &Path, blocks: &Blocks) -> Map<String, Value> {
    let cs_path = data_dir.join("charsets.json");
    let cs_raw: HashMap<String, Value> =
        serde_json::from_str(&fs::read_to_string(&cs_path).unwrap()).unwrap();
    let flag_sets: Vec<(&str, &str)> = vec![
        ("GB2312", "is_gb2312"),
        ("通用規範", "is_tonggui"),
        ("常用國字", "is_guozi"),
    ];
    let cjk_sets: Vec<(&str, usize)> = vec![
        ("CJK基本", 0),
        ("到CJK擴A", 1),
        ("到CJK擴B", 2),
        ("到CJK擴C", 3),
        ("到CJK擴D", 4),
        ("到CJK擴E", 5),
        ("到CJK擴F", 6),
        ("到CJK擴G", 7),
        ("到CJK擴H", 8),
        ("到CJK擴I", 9),
        ("到CJK擴J", 10),
    ];
    let in_cjk = |c: char, upto: usize| -> bool {
        let cp = c as u32;
        blocks[..=upto].iter().any(|&(a, b)| cp >= a && cp <= b)
    };
    let in_mark = |c: char, flag: &str| -> bool {
        cs_raw
            .get(&c.to_string())
            .and_then(|v| v.get(flag))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    type SetPred<'a> = Box<dyn Fn(char) -> bool + 'a>;
    let mut sets: Vec<(&str, SetPred)> = Vec::new();
    for &(name, flag) in &flag_sets {
        sets.push((name, Box::new(move |c| in_mark(c, flag))));
    }
    for &(name, upto) in &cjk_sets {
        sets.push((name, Box::new(move |c| in_cjk(c, upto))));
    }

    let mut out = Map::new();
    for (name, pred) in &sets {
        // TS 遍历 fullCodeTable(full) 的 keys —— 即 full 表的 Map 迭代序
        // （一字多码取最长码时 delete+set 移到末尾），用 full_map_order 精确复刻。
        let mut groups: Vec<(&String, Vec<char>)> = Vec::new();
        let mut gidx: HashMap<&String, usize> = HashMap::new();
        for c in &t.full_map_order {
            if pred(*c)
                && let Some(code) = t.full.get(c)
            {
                let gi = *gidx.entry(code).or_insert_with(|| {
                    groups.push((code, Vec::new()));
                    groups.len() - 1
                });
                groups[gi].1.push(*c);
            }
        }
        let max_n = groups.iter().map(|(_, v)| v.len()).max().unwrap_or(0);
        let codes: Vec<String> = groups
            .iter()
            .filter(|(_, v)| v.len() == max_n)
            .map(|(k, _)| (*k).clone())
            .collect();
        out.insert(
            (*name).to_string(),
            json!({
                "最大候選個數": max_n, "編碼列表": codes,
            }),
        );
    }
    out.insert("字符數".to_string(), json!(t.full.len()));
    out
}

// =========================================================================
// 对外接口
// =========================================================================

/// 对一张码表跑完整的宇浩测评，返回与 Node CLI 等价的「測評結果」JSON。
///
/// `码表` 单张码表路径；`方案` JSONC 方案路径（取最大码长/选重键表/终止指示符）；
/// `data_dir` 字频与字集 JSON 目录、`settings_dir` 当量矩阵与 CJK 区块目录；
/// 两者均可直接指向本仓平铺的 yuhao-data/（equivTable/cjkBlocks 已纳入本仓）。
pub fn 测评(
    码表: &Path,
    方案: &Path,
    data_dir: &Path,
    settings_dir: &Path,
) -> Result<Value, String> {
    let sc = load_scheme(方案)?;
    let maxlen = sc.maxlen;
    let t = parse_and_build(码表, &sc)?;
    let sel = sc.sel.clone();

    // 当量矩阵
    let eq_path = settings_dir.join("equivTable.json");
    let eq_raw: Value = serde_json::from_str(
        &fs::read_to_string(&eq_path)
            .map_err(|e| format!("读不了 {}: {}", eq_path.display(), e))?,
    )
    .map_err(|e| format!("解析当量矩阵失败: {}", e))?;
    let mut equiv = HashMap::new();
    for (k, v) in eq_raw.get("data").unwrap().as_object().unwrap() {
        equiv.insert(k.clone(), v.as_f64().unwrap());
    }

    // 五套字频（保序）
    let scf = load_freq(&data_dir.join("charAbsoluteFrequencySC.json"))?;
    let tcf = load_freq(&data_dir.join("charAbsoluteFrequencyTC.json"))?;
    let zh = load_freq(&data_dir.join("charAbsoluteFrequencyZhihu.json"))?;
    let gj = load_freq(&data_dir.join("charAbsoluteFrequencyGuji.json"))?;
    let hj = merged_freq(&scf, &tcf);

    let (lvl1_order, lvl1) = gen_level(&t.order, &t.short_sel, &t.full_sel, 2);
    let (lvl2_order, lvl2) = gen_level(&t.order, &t.short_sel, &t.full_sel, 3);

    // 速度当量：5 频 × 4 层级
    let tables: Vec<(&str, &Vec<char>, &HashMap<char, String>)> = vec![
        ("全碼", &t.order, &t.full_sel),
        ("一級簡碼", &lvl1_order, &lvl1),
        ("二級簡碼", &lvl2_order, &lvl2),
        ("全部簡碼", &t.order, &t.short_sel),
    ];
    let freq_list: Vec<(&str, &Freq)> = vec![
        ("北語簡體字頻", &scf),
        ("臺標繁體字頻", &tcf),
        ("知乎簡體字頻", &zh),
        ("古籍繁體字頻", &gj),
        ("繁簡聯合字頻", &hj),
    ];
    let mut speed = Map::new();
    for (fname, freq) in &freq_list {
        for (tname, order, tab) in &tables {
            let v = weighted_ordered(order, tab, freq, &equiv, &sel);
            speed.insert(format!("{}{}速度當量", fname, tname), json!(v));
        }
    }

    // 静态重码（14 字集）
    let blocks = load_blocks(settings_dir)?;
    let dup = build_static_dup(&t, data_dir, &blocks)?;

    // 动态选重：5 频 × sorted/original × 全/简
    let dyn_specs: Vec<(&str, &str, &Freq)> = vec![
        ("知乎簡體動態選重率", "知乎簡體動態選重率原序", &zh),
        ("北語簡體動態選重率", "北語簡體動態選重率原序", &scf),
        ("臺標繁體動態選重率", "臺標繁體動態選重率原序", &tcf),
        ("古籍繁體動態選重率", "古籍繁體動態選重率原序", &gj),
        ("繁簡聯合動態選重率", "繁簡聯合動態選重率原序", &hj),
    ];
    let mut dynres = Map::new();
    for (skey, okey, freq) in &dyn_specs {
        dynres.insert(
            (*skey).to_string(),
            json!({
                "全碼": calc_dyn_sorted(&t.full, &t.full_map_order, freq),
                "簡碼": calc_dyn_sorted(&t.short, &t.short_map_order, freq),
            }),
        );
        dynres.insert(
            (*okey).to_string(),
            json!({
                "全碼": calc_dyn_original(&t.full_sel, &t.full_map_order, freq),
                "簡碼": calc_dyn_original(&t.short_sel, &t.short_map_order, freq),
            }),
        );
    }

    // 简码效率：5 频 × 28 N
    let n_values: Vec<usize> = vec![
        100, 200, 300, 400, 500, 600, 700, 800, 900, 1000, 1500, 2000, 2500, 3000, 3500, 4000,
        4500, 5000, 6000, 7000, 8000, 9000, 10000, 12000, 15000, 20000, 30000, 50000,
    ];
    let eff_specs: Vec<(&str, &Freq)> = vec![
        ("知乎簡體字頻下之簡碼效率", &zh),
        ("北語簡體字頻下之簡碼效率", &scf),
        ("臺標繁體字頻下之簡碼效率", &tcf),
        ("古籍繁體字頻下之簡碼效率", &gj),
        ("繁簡聯合字頻下之簡碼效率", &hj),
    ];
    let mut effres = Map::new();
    for (key, freq) in &eff_specs {
        let (full_str, nres) = calc_short_eff(freq, &t.short_sel, &t.full_sel, maxlen, &n_values);
        let nval: Vec<Value> = nres
            .iter()
            .map(|(n, v)| {
                json!({
                    "最有效率的簡碼個數": n, "字頻加權碼長": v,
                })
            })
            .collect();
        effres.insert(
            (*key).to_string(),
            json!({
                "N值結果": nval, "完整字符字符串": full_str,
            }),
        );
    }

    // 键位热力（含选重键折算）
    let heat = json!({
        "全碼": calc_key_heat(&t.full_sel, &t.full_map_order, &scf, &sel),
        "簡碼": calc_key_heat(&t.short_sel, &t.short_map_order, &scf, &sel),
    });

    // 候选个数
    let candidates = build_candidates(&t, data_dir, &blocks);

    Ok(json!({
        "測評結果": {
            "速度當量分析": speed,
            "靜態重碼分析": dup,
            "動態選重分析": dynres,
            "簡碼效率分析": effres,
            "鍵位熱力": heat,
            "候選個數分析": candidates,
        },
    }))
}

