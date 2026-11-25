//! Small helpers shared by every list surface: cursor arithmetic and
//! terminal-cell text measuring and clipping.

/// Advances `cur` by `delta` over `n` items, wrapping at both ends (a
/// non-positive list stays at 0).
pub fn wrap_cursor(cur: usize, delta: i64, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let n = n as i64;
    let mut c = (cur as i64 + delta) % n;
    if c < 0 {
        c += n;
    }
    c as usize
}

/// Moves `cur` by `delta` pages of `page` rows, clamped to `n`.
pub fn page_cursor(cur: usize, delta: i64, page: usize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let page = page.max(1) as i64;
    let last = n as i64 - 1;
    let mut c = cur as i64;
    if delta < 0 {
        c = (c - page).max(0);
    } else if delta > 0 {
        c = (c + page).min(last);
    }
    c as usize
}

// --- terminal-cell text helpers (Go ansi.StringWidthWc / ansi.TruncateWc) ---

/// displayWidth returns the terminal cell width of s (ANSI escapes are
/// skipped; wide runes count two cells).
pub(crate) fn display_width(s: &str) -> usize {
    let mut w = 0usize;
    let mut esc = false;
    for c in s.chars() {
        if esc {
            if c == 'm' {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            esc = true;
            continue;
        }
        w += char_width(c);
    }
    w
}

pub(crate) fn char_width(c: char) -> usize {
    let cp = c as u32;
    // wide: CJK, Hangul, full-width forms, emoji ranges (wcwidth semantics,
    // approximated)
    if (0x1100..=0x115F).contains(&cp)
        || (0x2E80..=0xA4CF).contains(&cp)
        || (0xAC00..=0xD7A3).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFE10..=0xFE19).contains(&cp)
        || (0xFE30..=0xFE6F).contains(&cp)
        || (0xFF00..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x1F300..=0x1F64F).contains(&cp)
        || (0x20000..=0x3FFFD).contains(&cp)
    {
        2
    } else {
        1
    }
}

/// clipDisplay truncates s to max terminal cells, ANSI-aware. A reset closes
/// any style left open by the cut.
pub(crate) fn clip_display(s: &str, max: usize) -> String {
    if max < 1 {
        return String::new();
    }
    if display_width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    let mut styled = false;
    let mut esc = false;
    'outer: for c in s.chars() {
        if esc {
            out.push(c);
            if c == 'm' {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            out.push(c);
            esc = true;
            styled = true;
            continue;
        }
        let cw = char_width(c);
        if w + cw > max {
            break 'outer;
        }
        out.push(c);
        w += cw;
    }
    if styled {
        out.push_str("\x1b[0m");
    }
    out
}

/// clip_keep_tail clips `body` so a protected tail of `keep` cells (change
/// dot, severity glyph) still fits into `avail` cells, marking the cut with
/// an ellipsis. A body that already fits comes back unchanged; the caller
/// appends the tail itself.
pub(crate) fn clip_keep_tail(body: &str, avail: usize, keep: usize) -> String {
    let keep = keep.min(avail);
    let budget = avail.saturating_sub(keep);
    if display_width(body) <= budget {
        return body.to_string();
    }
    if budget < 2 {
        return String::new();
    }
    let mut out = clip_display(body, budget - 1);
    out.push('…');
    out
}

/// clip_left_ellipsis keeps the end of plain text within max cells, marking
/// the cut with a leading `…`. The tail is never dropped: it is the part that
/// distinguishes candidates sharing a long prefix.
pub(crate) fn clip_left_ellipsis(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".to_string();
    }
    let mut tail: Vec<char> = Vec::new();
    let mut w = 0usize;
    for c in s.chars().rev() {
        let cw = char_width(c);
        if w + cw > max - 1 {
            break;
        }
        tail.push(c);
        w += cw;
    }
    let mut out = String::from("…");
    out.extend(tail.into_iter().rev());
    out
}

/// clipLeft keeps the tail of s within max runes, marking the cut with "…".
pub(crate) fn clip_left(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let r: Vec<char> = s.chars().collect();
    if r.len() <= max {
        return s.to_string();
    }
    if max == 1 {
        return "…".to_string();
    }
    "…".to_string() + &r[r.len() - (max - 1)..].iter().collect::<String>()
}

/// wrapText wraps s to max display cells per line, preserving explicit
/// newlines. It breaks at the last space when that leaves a non-empty prefix;
/// otherwise it breaks between runes.
pub(crate) fn wrap_text(s: &str, max: usize) -> Vec<String> {
    if max < 1 {
        return vec![s.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        for r in para.chars() {
            let mut cand = line.clone();
            cand.push(r);
            if display_width(&cand) <= max {
                line = cand;
                continue;
            }
            if line.is_empty() {
                line = r.to_string();
                continue;
            }
            if let Some(i) = line.rfind(' ') {
                if i > 0 {
                    out.push(line[..i].to_string());
                    line = line[i + 1..].to_string();
                    line.push(r);
                    continue;
                }
            }
            out.push(line);
            line = r.to_string();
        }
        out.push(line);
    }
    out
}
