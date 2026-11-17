// =========================================================================
// 纯 Rust 宇浩测评内核
// =========================================================================
//
// 取代旧版 assess.rs 里对 `pnpm cli`（yuhao-assess Node CLI）的 spawn 调用。
// f64 输出严格复刻 Node CLI 的累加顺序 / Map 迭代顺序，已与生产方案
// codegenie-s4.jsonc 的 CLI 产物逐字段对拍：497 项中 492 逐位相等、
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

// =========================================================================
// 连续文本当量（替代 fill-continuous.mts；纯 Rust，无 Node 依赖）
// =========================================================================
//
// 复刻 continuousEquivalentService.ts + atoms/continuousEquivalent.ts：
//   清洗语料 → 字符→按键映射 → mulberry32 抽 20000 个 100 字窗口
//   → 逐窗口相邻码对当量 → 统计 / 分位数 / CVaR / 分布压缩。
// 语料实测全在 BMP（CJK Unified + 1 个 ExtA `㝢` + 两个标点），无代理对，
// 故 TS 的 substring/.length 与 Rust 的 char 索引逐位等价。

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

/// mulberry32：与 TS 版逐位一致，保证 20000 个窗口起点完全相同。
struct Mulberry32 {
    state: u32,
}
impl Mulberry32 {
    fn new(seed: u32) -> Self {
        Self { state: seed }
    }
    fn next(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6d2b79f5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        ((t ^ (t >> 14)) as f64) / 4294967296.0
    }
}

/// 语料清洗：保留 Script=Han 汉字与「，。」。覆盖实测语料出现的全部区间。
fn 是语料汉字(c: char) -> bool {
    let cp = c as u32;
    cp == 0x3007
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x4E00..=0x9FFF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0x20000..=0x323AF).contains(&cp)
        || (0x2F800..=0x2FA1F).contains(&cp)
}

/// 升序数组线性插值分位数（复刻 取分位數）。
fn 取分位数(升序: &[f64], q: f64) -> f64 {
    let n = 升序.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return 升序[0];
    }
    let 位置 = (n - 1) as f64 * q;
    let 下标 = 位置.floor() as usize;
    let 上标 = (下标 + 1).min(n - 1);
    let 余 = 位置 - 下标 as f64;
    升序[下标] + (升序[上标] - 升序[下标]) * 余
}

/// 右尾条件均值 CVaR（复刻 取條件尾部均值：二分找首个 ≥ 门槛）。
fn 取条件尾部均值(升序: &[f64], 门槛: f64) -> f64 {
    let mut 左 = 0usize;
    let mut 右 = 升序.len();
    while 左 < 右 {
        let 中 = (左 + 右) / 2;
        if 升序[中] < 门槛 {
            左 = 中 + 1;
        } else {
            右 = 中;
        }
    }
    if 左 >= 升序.len() {
        return 门槛;
    }
    let mut 和 = 0.0;
    for &v in &升序[左..] {
        和 += v;
    }
    和 / (升序.len() - 左) as f64
}

/// 复刻 壓縮分佈：分布(当量值,个数) → {起始,步長,個數}，格点 0.001。
fn 压缩分布(分布: &[(f64, usize)]) -> Value {
    let 步长 = 0.001f64;
    if 分布.is_empty() {
        return json!({ "起始": 0.0, "步長": 步长, "個數": [] });
    }
    let 起始 = 分布[0].0;
    let 末 = 分布[分布.len() - 1].0;
    let 格点 = ((末 - 起始) / 步长).round() as usize + 1;
    let mut 个数 = vec![0usize; 格点];
    for &(值, 计) in 分布 {
        let 下标 = ((值 - 起始) / 步长).round() as i64;
        if 下标 >= 0 && (下标 as usize) < 格点 {
            个数[下标 as usize] += 计;
        }
    }
    json!({ "起始": 起始, "步長": 步长, "個數": 个数 })
}

/// 单口径蒙特卡洛，返回压缩后的统计块（复刻 蒙特卡洛連續文本當量）。
fn 蒙特卡洛连续当量(
    清洗: &[char],
    tab: &HashMap<char, String>,
    当量表: &HashMap<String, f64>,
    sel: &HashMap<String, String>,
    样本数: usize,
    窗口: usize,
) -> Result<Value, String> {
    let m = 清洗.len();
    if m < 窗口 {
        return Err(format!("语料仅 {} 字，不足一个窗口 {}", m, 窗口));
    }

    // 字符 → 按键串（复刻 構建字符按鍵映射）；标点单列，汉字查表后 replace_sel
    let mut 映射: HashMap<char, String> = HashMap::new();
    let mut 未编码: HashSet<char> = HashSet::new();
    for &c in 清洗 {
        if 映射.contains_key(&c) || 未编码.contains(&c) {
            continue;
        }
        if c == '，' {
            映射.insert(c, ",".to_string());
            continue;
        }
        if c == '。' {
            映射.insert(c, ".".to_string());
            continue;
        }
        match tab.get(&c) {
            Some(code) => {
                映射.insert(c, replace_sel(code, sel));
            }
            None => {
                未编码.insert(c);
            }
        }
    }
    let 未编码字数 = if 未编码.is_empty() {
        0
    } else {
        清洗.iter().filter(|c| 未编码.contains(c)).count()
    };

    let 起点上限 = m - 窗口;
    let mut rng = Mulberry32::new(20260801);
    let mut 样本: Vec<f64> = Vec::with_capacity(样本数);
    let mut 累计码对 = 0usize;
    let mut 累计有效 = 0usize;

    for _ in 0..样本数 {
        let 起点 = (rng.next() * ((起点上限 + 1) as f64)).floor() as usize;
        let mut 序列 = String::with_capacity(窗口 * 3);
        for c in &清洗[起点..起点 + 窗口] {
            if let Some(k) = 映射.get(c) {
                序列.push_str(k);
            }
        }
        // 序列全为 ASCII（字母 / 数字 / , . _ ; '），字节切片 == TS substring
        let 字节 = 序列.as_bytes();
        let 码对数 = 字节.len().saturating_sub(1);
        let mut 总当量 = 0.0f64;
        let mut 有效码对 = 0usize;
        for i in 0..码对数 {
            let 对 = std::str::from_utf8(&字节[i..i + 2]).unwrap();
            if let Some(&v) = 当量表.get(对) {
                总当量 += v;
                有效码对 += 1;
            }
        }
        累计码对 += 码对数;
        累计有效 += 有效码对;
        if 有效码对 > 0 {
            样本.push(总当量 / 有效码对 as f64);
        }
    }

    if 样本.is_empty() {
        return Err("没有得到任何有效样本，请检查码表与当量表".to_string());
    }
    样本.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = 样本.len();
    let 平均数 = 样本.iter().fold(0.0f64, |s, &v| s + v) / n as f64;
    let 方差 = 样本
        .iter()
        .fold(0.0f64, |s, &v| s + (v - 平均数) * (v - 平均数))
        / n as f64;

    // 分布：Math.round(v*1000)/1000 归并（BTreeMap 整数键天然升序）
    let mut 计数: BTreeMap<i64, usize> = BTreeMap::new();
    for &v in &样本 {
        *计数.entry((v * 1000.0).round() as i64).or_insert(0) += 1;
    }
    let 分布: Vec<(f64, usize)> = 计数.iter().map(|(&k, &c)| (k as f64 / 1000.0, c)).collect();

    let 九零 = 取分位数(&样本, 0.9);
    Ok(json!({
        "樣本數": n,
        "平均數": 平均数,
        "中位數": 取分位数(&样本, 0.5),
        "九零分位數": 九零,
        "九零條件分位數": 取条件尾部均值(&样本, 九零),
        "標準差": 方差.sqrt(),
        "最小值": 样本[0],
        "最大值": 样本[n - 1],
        "分佈": 压缩分布(&分布),
        "語料字數": m,
        "窗口長度": 窗口,
        "未編碼字數": 未编码字数,
        "碼對覆蓋率": if 累计码对 > 0 { 累计有效 as f64 / 累计码对 as f64 } else { 0.0 },
    }))
}

/// 给方案档案补上「連續文本當量」并写回，完整复刻 fill-continuous.mts。
/// 返回写入的连续当量结果块（供调用方打印摘要）。
pub fn 补连续当量(
    档案路径: &Path,
    yuhao目录: &Path,
    样本数: usize,
    窗口: usize,
) -> Result<Value, String> {
    let 原文 = fs::read_to_string(档案路径).map_err(|e| format!("读不了档案: {}", e))?;
    let 清理 = 原文.trim_start_matches('\u{feff}');
    let mut 档: Value = serde_json::from_str(清理).map_err(|e| format!("解析档案失败: {}", e))?;

    let 码表路径 = 档
        .get("_碼表")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .ok_or_else(|| "档案缺 _碼表，无法定位码表".to_string())?;

    let 参数 = 档.get("方案參數");
    let 最大码长 = 参数
        .and_then(|p| p.get("最大碼長"))
        .and_then(|x| x.as_u64())
        .unwrap_or(4) as usize;
    let mut sel = HashMap::new();
    if let Some(m) = 参数
        .and_then(|p| p.get("選重鍵表"))
        .and_then(|x| x.as_object())
    {
        for (k, v) in m {
            if let Some(s) = v.as_str() {
                sel.insert(k.clone(), s.to_string());
            }
        }
    }
    if sel.is_empty() {
        sel.insert("2".to_string(), ";".to_string());
        sel.insert("3".to_string(), "'".to_string());
    }
    let sc = Scheme {
        maxlen: 最大码长,
        sel: sel.clone(),
        has_terminator: false,
        terminators: vec![],
    };

    let 语料原文 = fs::read_to_string(yuhao目录.join("literature.txt"))
        .map_err(|e| format!("读不了语料: {}", e))?;
    let 清洗: Vec<char> = 语料原文
        .chars()
        .filter(|&c| c == '，' || c == '。' || 是语料汉字(c))
        .collect();

    let eq文本 = fs::read_to_string(yuhao目录.join("equivTable.json"))
        .map_err(|e| format!("读不了当量表: {}", e))?;
    let eq: Value = serde_json::from_str(&eq文本).map_err(|e| format!("解析当量表失败: {}", e))?;
    let 当量表: HashMap<String, f64> = eq
        .get("data")
        .and_then(|d| d.as_object())
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0)))
                .collect()
        })
        .unwrap_or_default();

    let t = parse_and_build(&码表路径, &sc)?;
    let 全 = 蒙特卡洛连续当量(&清洗, &t.full_sel, &当量表, &sel, 样本数, 窗口)?;
    let 简 = 蒙特卡洛连续当量(&清洗, &t.short_sel, &当量表, &sel, 样本数, 窗口)?;

    let 结果 = json!({
        "統計": { "全碼加選重": 全, "全部簡碼加選重": 简 },
        "樣本數": 样本数,
        "窗口長度": 窗口,
        "更新時間": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });

    {
        let obj = 档
            .as_object_mut()
            .ok_or_else(|| "档案根不是 JSON 对象".to_string())?;
        obj.remove("_碼表");
        let 测评 = obj
            .entry("測評結果".to_string())
            .or_insert_with(|| json!({}));
        if let Some(m) = 测评.as_object_mut() {
            m.insert("連續文本當量".into(), 结果.clone());
        }
    }
    let 文本 = serde_json::to_string_pretty(&档).map_err(|e| format!("序列化档案失败: {}", e))?;
    fs::write(档案路径, format!("{}\n", 文本)).map_err(|e| format!("写档案失败: {}", e))?;
    Ok(结果)
}

// =========================================================================
// 测试：连续当量的可复现性护栏（期望值取自 TS mulberry32(20260801)）
// =========================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulberry32_种子20260801_前五值逐位一致() {
        let mut rng = Mulberry32::new(20260801);
        let 期望: [f64; 5] = [
            0.25129492254927754,
            0.977159610716626,
            0.5359782394953072,
            0.07717855530790985,
            0.9042584367562085,
        ];
        for &e in &期望 {
            let got = rng.next();
            assert_eq!(
                got.to_bits(),
                e.to_bits(),
                "mulberry32 与 TS 不符: {} vs {}",
                got,
                e
            );
        }
    }

    #[test]
    fn 压缩分布_三位小数归并成紧凑格点() {
        // 起始 1.211 → 格点 1.211/1.212/1.213，中间空格点记 0
        let 分布 = vec![(1.211, 3usize), (1.212, 5), (1.213, 2)];
        let v = 压缩分布(&分布);
        assert_eq!(v["起始"].as_f64().unwrap(), 1.211);
        assert_eq!(v["步長"].as_f64().unwrap(), 0.001);
        let 个数 = v["個數"].as_array().unwrap();
        let 整数: Vec<i64> = 个数.iter().map(|x| x.as_i64().unwrap()).collect();
        assert_eq!(整数, vec![3, 5, 2]);
        assert_eq!(整数.iter().sum::<i64>(), 10);
    }

    #[test]
    fn 取分位数_线性插值() {
        let 样本 = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(取分位数(&样本, 0.5), 3.0);
        // (n-1)*0.9 = 3.6 → 下标 3，余 0.6 → 4 + (5-4)*0.6 = 4.6
        let p90 = 取分位数(&样本, 0.9);
        assert!((p90 - 4.6).abs() < 1e-12, "p90={}", p90);
    }
}
