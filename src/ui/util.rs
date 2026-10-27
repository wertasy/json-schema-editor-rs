//! Small cursor helpers shared by every scrolling list surface.

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
