use super::*;

// --- search filtering --------------------------------------------------------

/// filterSearch narrows the index to rows matching q. A query starting with
/// "$" is a document-path query; a query starting with "." matches the
/// keyword against path segments from any node; any other query is ordinary
/// text (title, key and scalar value).
pub fn filter_search(
    ctx: &MenuCtx<'_>,
    all: &[Row],
    q: &str,
    fold: bool,
    re: Option<&Regex>,
) -> Vec<Row> {
    let (schema, doc) = (ctx.schema, ctx.doc);
    if q.is_empty() {
        return all.to_vec();
    }
    if is_path_query(q) {
        let mut picked = vec![false; all.len()];
        let mut out: Vec<Row> = Vec::new();
        if let Some(re) = re {
            for (i, r) in all.iter().enumerate() {
                let body = path_body(&r.path);
                if re.find(&body).map(|m| m.start()) == Some(0) {
                    picked[i] = true;
                    out.push(r.clone());
                }
            }
            out.sort_by_key(|r| {
                let body = path_body(&r.path);
                !matches!(re.find(&body), Some(m) if m.start() == 0 && m.end() == body.len())
            });
        } else {
            let needle = if fold {
                q.to_lowercase()
            } else {
                q.to_string()
            };
            for (i, r) in all.iter().enumerate() {
                let mut p = path_string(&r.path);
                if fold {
                    p = p.to_lowercase();
                }
                if p.starts_with(&needle) {
                    picked[i] = true;
                    out.push(r.clone());
                }
            }
            out.sort_by_key(|r| {
                let mut p = path_string(&r.path);
                if fold {
                    p = p.to_lowercase();
                }
                p != needle
            });
        }
        out.extend(text_search_rows(
            schema,
            doc,
            all,
            q,
            fold,
            None,
            Some(&picked),
            true,
        ));
        return out;
    }
    if is_dot_query(q) {
        let mut picked = vec![false; all.len()];
        let mut out = dot_query_rows(all, &mut picked, q, fold);
        out.extend(text_search_rows(
            schema,
            doc,
            all,
            q,
            fold,
            None,
            Some(&picked),
            true,
        ));
        return out;
    }
    text_search_rows(schema, doc, all, q, fold, re, None, false)
}

/// isPathQuery reports whether q is a document-path query: "$", "$.", or
/// "$[" prefix.
pub fn is_path_query(q: &str) -> bool {
    q == "$" || q.starts_with("$.") || q.starts_with("$[")
}

/// isDotQuery reports whether q is an any-node path query: "." plus at
/// least one more character.
pub fn is_dot_query(q: &str) -> bool {
    q.len() > 1 && q.starts_with('.')
}

/// dotQueryRows ranks rows whose path matches a leading-dot query from any
/// node (dotQueryRank); ties keep document order. Rows it lists are marked
/// in picked so the text fallback skips them.
pub fn dot_query_rows(all: &[Row], picked: &mut [bool], q: &str, fold: bool) -> Vec<Row> {
    let mut hits: Vec<(Row, usize)> = Vec::new();
    for (i, r) in all.iter().enumerate() {
        if let Some(rank) = dot_query_rank(&r.path, &q[1..], fold) {
            picked[i] = true;
            hits.push((r.clone(), rank));
        }
    }
    hits.sort_by_key(|h| h.1);
    hits.into_iter().map(|h| h.0).collect()
}

/// dotQueryRank scores one row path against the keyword after the leading
/// dot; rank 0+ for a run ending in the leaf segment, 16+ for one ending
/// higher up; None when nothing matches.
pub fn dot_query_rank(path: &[PathSeg], keyword: &str, fold: bool) -> Option<usize> {
    let mut parts: Vec<String> = keyword.split('.').map(String::from).collect();
    if fold {
        parts = parts.into_iter().map(|p| p.to_lowercase()).collect();
    }
    let segs: Vec<String> = path
        .iter()
        .map(|seg| match seg {
            PathSeg::Key(k) => k.clone(),
            PathSeg::Idx(i) => i.to_string(),
        })
        .map(|s| if fold { s.to_lowercase() } else { s })
        .collect();
    let mut best: Option<usize> = None;
    if parts.len() <= segs.len() {
        for s in 0..=segs.len() - parts.len() {
            let mut tier = 0u8;
            for (k, part) in parts.iter().enumerate() {
                let t = seg_match(&segs[s + k], part);
                if t == 0 {
                    tier = 0;
                    break;
                }
                if t > tier {
                    tier = t;
                }
            }
            if tier == 0 {
                continue;
            }
            // the run ends above the leaf: a descendant hit
            let r = if s + parts.len() == segs.len() {
                (tier - 1) as usize // ends in the leaf segment: the node itself
            } else {
                16 + (tier - 1) as usize
            };
            if best.is_none() || r < best.unwrap() {
                best = Some(r);
            }
        }
    }
    best
}

/// segMatch scores keyword against one path segment: 1 = segment prefix,
/// 2 = infix, 3 = fuzzy subsequence, 0 = no match.
pub fn seg_match(seg: &str, keyword: &str) -> u8 {
    if seg.starts_with(keyword) {
        return 1;
    }
    if seg.contains(keyword) {
        return 2;
    }
    if subsequence_match(seg, keyword) {
        return 3;
    }
    0
}

/// subsequenceMatch reports whether keyword's runes appear in seg in order.
pub fn subsequence_match(seg: &str, keyword: &str) -> bool {
    let rs: Vec<char> = keyword.chars().collect();
    let mut i = 0;
    for r in seg.chars() {
        if i < rs.len() && r == rs[i] {
            i += 1;
        }
    }
    i == rs.len()
}

pub(crate) fn text_search_rows(
    schema: &Schema,
    doc: &Doc,
    all: &[Row],
    q: &str,
    fold: bool,
    re: Option<&Regex>,
    picked: Option<&[bool]>,
    raw: bool,
) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    if let Some(re) = re {
        for (i, r) in all.iter().enumerate() {
            if picked.map(|p| p[i]).unwrap_or(false) {
                continue;
            }
            let Some(text) = (if raw {
                row_fallback_text(schema, doc, r)
            } else {
                row_search_text(schema, doc, r)
            }) else {
                continue;
            };
            if re.is_match(&text) {
                out.push(r.clone());
            }
        }
        out.sort_by_key(|r| {
            let target = row_key(schema, r);
            !matches!(re.find(&target), Some(m) if m.start() == 0 && m.end() == target.len())
        });
        return out;
    }
    let needle = if fold {
        q.to_lowercase()
    } else {
        q.to_string()
    };
    for (i, r) in all.iter().enumerate() {
        if picked.map(|p| p[i]).unwrap_or(false) {
            continue;
        }
        let Some(mut text) = (if raw {
            row_fallback_text(schema, doc, r)
        } else {
            row_search_text(schema, doc, r)
        }) else {
            continue;
        };
        if fold {
            text = text.to_lowercase();
        }
        if text.contains(&needle) {
            out.push(r.clone());
        }
    }
    let eq = |a: &str, b: &str| {
        if fold {
            a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase()
        } else {
            a == b
        }
    };
    out.sort_by_key(|r| if eq(&row_key(schema, r), q) { 0 } else { 1 });
    out
}

pub(crate) fn row_fallback_text(schema: &Schema, doc: &Doc, r: &Row) -> Option<String> {
    let Some(f) = r.fn_ else {
        if r.kind != RowKind::Unknown {
            return None;
        }
        let key = match r.path.last() {
            Some(PathSeg::Key(k)) => k.clone(),
            Some(PathSeg::Idx(i)) => i.to_string(),
            None => String::new(),
        };
        if let Some(s) = row_raw_scalar(schema, doc, r) {
            return Some(format!("{key} {s}"));
        }
        return Some(key);
    };
    let node = &schema.nodes[f as usize];
    let mut text = format!("{} {}", node.title, node.key);
    if let Some(PathSeg::Key(k)) = r.path.last() {
        if *k != node.key {
            text += &format!(" {k}");
        }
    }
    if let Some(s) = row_raw_scalar(schema, doc, r) {
        text += &format!(" {s}");
    }
    Some(text)
}

pub(crate) fn row_raw_scalar(schema: &Schema, doc: &Doc, r: &Row) -> Option<String> {
    let mut n = doc.get(&r.path).filter(|&v| !is_null(doc, v));
    let mut vdoc = doc;
    if n.is_none() {
        match r.fn_.and_then(|f| schema.nodes[f as usize].default_) {
            Some(d) => {
                n = Some(d);
                vdoc = &schema.doc;
            }
            None => return None,
        }
    }
    match vdoc.value(n.unwrap()) {
        Value::Str(s) => Some(s.clone()),
        Value::Num(num) => Some(num.clone()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// pathBody is a row path as a path-query match subject: the rendered path
/// without its leading "$".
pub fn path_body(path: &[PathSeg]) -> String {
    path_string(path)[1..].to_string()
}

/// pathPattern turns a document-path query into a regex over pathBody: the
/// leading "$" is dropped and JSON-path index brackets are escaped.
pub fn path_pattern(q: &str) -> String {
    let body = q.strip_prefix('$').unwrap_or(q);
    let b = body.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'[' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        match body[i + 1..].find(']') {
            Some(k0) => {
                let k = i + 1 + k0;
                if all_digits(&body[i + 1..k]) {
                    out.extend_from_slice(format!("\\[{}\\]", &body[i + 1..k]).as_bytes());
                    i = k + 1;
                    continue;
                }
            }
            None => {
                let rest = &body[i + 1..];
                if rest.is_empty() || all_digits(rest) {
                    // an index still being typed: escape so the regex stays valid
                    out.extend_from_slice(format!("\\[{rest}").as_bytes());
                    i = b.len();
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// allDigits reports whether s consists of ASCII digits and is non-empty.
pub fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// rowKey returns a row's leaf key for exact-key ranking: the last path
/// segment when it is a string, else the schema node's key.
pub fn row_key(schema: &Schema, r: &Row) -> String {
    if let Some(PathSeg::Key(s)) = r.path.last() {
        return s.clone();
    }
    if let Some(f) = r.fn_ {
        return schema.nodes[f as usize].key.clone();
    }
    String::new()
}

/// unknownMatchText is the match text of a schema-less row: its key (the
/// last path segment) plus, for a scalar value, the value overview carried
/// in the label.
pub fn unknown_match_text(r: &Row, with_value: bool) -> String {
    let key = match r.path.last() {
        Some(PathSeg::Key(k)) => k.clone(),
        Some(PathSeg::Idx(i)) => i.to_string(),
        None => String::new(),
    };
    if !with_value {
        return key;
    }
    let overview = match r.label.find("  ") {
        Some(i) => r.label[i + 2..].to_string(),
        None => r.label.clone(),
    };
    format!("{key} {overview}")
}

/// replaceAll replaces every occurrence of old in s with rep, scanning left
/// to right without re-scanning replacements. fold=true matches
/// case-insensitively. With re the query is a regular expression instead.
pub fn replace_all(
    s: &str,
    old: &str,
    rep: &str,
    fold: bool,
    re: Option<&Regex>,
) -> (String, usize) {
    if let Some(re) = re {
        return (re.replace_all(s, rep).into_owned(), re.find_iter(s).count());
    }
    if old.is_empty() {
        return (s.to_string(), 0);
    }
    if !fold {
        let n = s.matches(old).count();
        if n == 0 {
            return (s.to_string(), 0);
        }
        return (s.replace(old, rep), n);
    }
    let sr: Vec<char> = s.chars().collect();
    let or: Vec<char> = old.chars().collect();
    let mut b = String::new();
    let mut n = 0usize;
    let mut i = 0usize;
    while i < sr.len() {
        if i + or.len() <= sr.len() && runes_fold_equal(&sr[i..i + or.len()], &or) {
            b.push_str(rep);
            n += 1;
            i += or.len();
            continue;
        }
        b.push(sr[i]);
        i += 1;
    }
    (b, n)
}

pub(crate) fn runes_fold_equal(a: &[char], b: &[char]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| x.to_lowercase().eq(y.to_lowercase()))
}

/// replaceHits narrows the index to rows whose document value is a string
/// containing q (per fold), relabeling each hit
/// "<path>  <old> → <new>" so the result list doubles as the replacement
/// preview.
pub fn replace_hits(
    ctx: &MenuCtx<'_>,
    all: &[Row],
    q: &str,
    rep: &str,
    fold: bool,
    re: Option<&Regex>,
) -> Vec<Row> {
    let doc = ctx.doc;
    let mut out: Vec<Row> = Vec::new();
    let mut seen: Vec<Vec<PathSeg>> = Vec::new();
    for r in all {
        if seen.iter().any(|p| path_eq(p, &r.path)) {
            continue;
        }
        let Some(n) = doc.get(&r.path) else {
            continue;
        };
        let Value::Str(s) = doc.value(n) else {
            continue;
        };
        let s = s.clone();
        seen.push(r.path.clone());
        let mut r2 = r.clone();
        if !q.is_empty() {
            let (new, c) = replace_all(&s, q, rep, fold, re);
            if c == 0 {
                continue;
            }
            r2.label = format!(
                "{}  {} → {}",
                path_string(&r.path),
                value_clip(&s),
                value_clip(&new)
            );
        }
        out.push(r2);
    }
    out
}
