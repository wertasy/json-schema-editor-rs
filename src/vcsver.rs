// Go-compatible semantic-version parsing, ordering, and pseudo-versions —
// the small part of `golang.org/x/mod/semver` + `module.PseudoVersion` that
// `build.rs` needs to derive `--version` from git tags the way `cmd/go` does.
//
// The file is `include!`d by `build.rs` and compiled by the crate only in
// test builds (`#[cfg(test)] #[path = "vcsver.rs"] mod vcsver;`), so the
// parser and comparator are covered by `cargo test` without shipping dead
// code.

use std::cmp::Ordering;

/// A parsed semantic version. Numeric fields are decimal strings (Go's
/// semver compares them by length then lexically, so arbitrary precision is
/// preserved). `pre` keeps its leading `-`, `build` its leading `+`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    major: String,
    minor: String,
    patch: String,
    pre: String,
    build: String,
}

impl Version {
    /// Parse `vMAJOR[.MINOR[.PATCH[-PRERELEASE][+BUILD]]]`, matching
    /// `golang.org/x/mod/semver` (including the `v1` / `v1.2` shorthands).
    pub fn parse(v: &str) -> Option<Self> {
        let v = v.strip_prefix('v')?;
        let (major, rest) = parse_int(v)?;
        if rest.is_empty() {
            return Some(Self::new(major, "0", "0", "", ""));
        }
        let (minor, rest) = parse_int(rest.strip_prefix('.')?)?;
        if rest.is_empty() {
            return Some(Self::new(major, minor, "0", "", ""));
        }
        let (patch, mut rest) = parse_int(rest.strip_prefix('.')?)?;

        let mut pre = "";
        if rest.starts_with('-') {
            (pre, rest) = parse_prerelease(rest)?;
        }
        let mut build = "";
        if rest.starts_with('+') {
            (build, rest) = parse_build(rest)?;
        }
        if !rest.is_empty() {
            return None;
        }
        Some(Self::new(major, minor, patch, pre, build))
    }

    fn new(major: &str, minor: &str, patch: &str, pre: &str, build: &str) -> Self {
        Self {
            major: major.to_string(),
            minor: minor.to_string(),
            patch: patch.to_string(),
            pre: pre.to_string(),
            build: build.to_string(),
        }
    }

    /// Canonical formatting: full major.minor.patch, prerelease kept, build
    /// metadata dropped (`semver.Canonical`).
    pub fn canonical(&self) -> String {
        format!("v{}.{}.{}{}", self.major, self.minor, self.patch, self.pre)
    }

    /// Whether `raw` is already canonical, i.e. usable verbatim as a tag
    /// version (`semver.Canonical(raw) == raw`).
    pub fn is_canonical(&self, raw: &str) -> bool {
        self.canonical() == raw
    }

    /// Semantic-version precedence (`semver.Compare`); build metadata is
    /// ignored.
    pub fn compare(&self, other: &Self) -> Ordering {
        compare_int(&self.major, &other.major)
            .then_with(|| compare_int(&self.minor, &other.minor))
            .then_with(|| compare_int(&self.patch, &other.patch))
            .then_with(|| compare_prerelease(&self.pre, &other.pre))
    }
}

/// `module.PseudoVersion("", base, t, rev)`: the version Go stamps for a
/// commit that is not exactly at a tag. `base` is the highest reachable tag
/// version (with build metadata retained), `timestamp` is the commit's UTC
/// `yyyymmddhhmmss`, and `rev` is the 12-character revision prefix.
pub fn pseudo_version(base: Option<&Version>, timestamp: &str, rev: &str) -> String {
    let Some(base) = base else {
        return format!("v0.0.0-{timestamp}-{rev}");
    };
    let segment = format!("{timestamp}-{rev}");
    if !base.pre.is_empty() {
        // vX.Y.Z-pre -> vX.Y.Z-pre.0.<timestamp>-<rev>
        format!("{}.0.{segment}{}", base.canonical(), base.build)
    } else {
        // vX.Y.Z -> vX.Y.(Z+1)-0.<timestamp>-<rev>
        format!(
            "v{}.{}.{}-0.{segment}{}",
            base.major,
            base.minor,
            increment_decimal(&base.patch),
            base.build,
        )
    }
}

fn parse_int(v: &str) -> Option<(&str, &str)> {
    let bytes = v.as_bytes();
    if bytes.first().is_none_or(|b| !b.is_ascii_digit()) {
        return None;
    }
    let mut end = 1;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if bytes[0] == b'0' && end != 1 {
        return None;
    }
    Some((&v[..end], &v[end..]))
}

fn parse_prerelease(v: &str) -> Option<(&str, &str)> {
    let bytes = v.as_bytes();
    let mut i = 1;
    let mut start = 1;
    while i < bytes.len() && bytes[i] != b'+' {
        let b = bytes[i];
        if !is_ident_char(b) && b != b'.' {
            return None;
        }
        if b == b'.' {
            if start == i || is_bad_num(&v[start..i]) {
                return None;
            }
            start = i + 1;
        }
        i += 1;
    }
    if start == i || is_bad_num(&v[start..i]) {
        return None;
    }
    Some((&v[..i], &v[i..]))
}

fn parse_build(v: &str) -> Option<(&str, &str)> {
    let bytes = v.as_bytes();
    let mut i = 1;
    let mut start = 1;
    while i < bytes.len() {
        let b = bytes[i];
        if !is_ident_char(b) && b != b'.' {
            return None;
        }
        if b == b'.' {
            if start == i {
                return None;
            }
            start = i + 1;
        }
        i += 1;
    }
    if start == i {
        return None;
    }
    Some((&v[..i], &v[i..]))
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

fn is_bad_num(v: &str) -> bool {
    let bytes = v.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    i == bytes.len() && i > 1 && bytes[0] == b'0'
}

fn is_num(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())
}

fn compare_int(x: &str, y: &str) -> Ordering {
    x.len().cmp(&y.len()).then_with(|| x.cmp(y))
}

fn compare_prerelease(x: &str, y: &str) -> Ordering {
    match (x.is_empty(), y.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        (false, false) => {}
    }
    // Both include the leading '-'.
    let xs: Vec<&str> = x[1..].split('.').collect();
    let ys: Vec<&str> = y[1..].split('.').collect();
    for (dx, dy) in xs.iter().zip(ys.iter()) {
        if dx == dy {
            continue;
        }
        let (nx, ny) = (is_num(dx), is_num(dy));
        if nx != ny {
            return if nx {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        return if nx { compare_int(dx, dy) } else { dx.cmp(dy) };
    }
    xs.len().cmp(&ys.len())
}

fn increment_decimal(s: &str) -> String {
    let mut bytes = s.as_bytes().to_vec();
    let mut i = bytes.len();
    loop {
        if i == 0 {
            bytes.insert(0, b'1');
            break;
        }
        i -= 1;
        if bytes[i] == b'9' {
            bytes[i] = b'0';
        } else {
            bytes[i] += 1;
            break;
        }
    }
    String::from_utf8(bytes).expect("decimal digits stay valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: &str) -> Version {
        Version::parse(v).unwrap_or_else(|| panic!("{v} should parse"))
    }

    #[test]
    fn parses_and_canonicalizes_like_go() {
        assert_eq!(parse("v1.2.3").canonical(), "v1.2.3");
        assert!(parse("v1.2.3").is_canonical("v1.2.3"));
        assert_eq!(parse("v1").canonical(), "v1.0.0");
        assert_eq!(parse("v1.2").canonical(), "v1.2.0");
        assert!(!parse("v1.2").is_canonical("v1.2"));
        assert_eq!(parse("v1.2.3-rc.1").canonical(), "v1.2.3-rc.1");
        assert_eq!(parse("v1.2.3-rc.1+meta").canonical(), "v1.2.3-rc.1");
        assert!(!parse("v1.2.3-rc.1+meta").is_canonical("v1.2.3-rc.1+meta"));

        for bad in [
            "1.2.3",
            "v",
            "v01.2.3",
            "v1.02.3",
            "v1.2.3-",
            "v1.2.3-01",
            "v1.2.3+",
            "v1..2",
            "v1.2.3-rc..1",
            "v1.2.3+meta..1",
            "v1.2.3 ",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn orders_by_semver_precedence() {
        let ordered = [
            "v0.0.0-20261022081434-ab1b465ca537", // pseudo before any release
            "v0.2.9",
            "v0.3.0-alpha",
            "v0.3.0-alpha.1",
            "v0.3.0-beta.2",
            "v0.3.0-beta.11",
            "v0.3.0-rc.1",
            "v0.3.0",
            "v0.10.0",
        ];
        for pair in ordered.windows(2) {
            let (a, b) = (parse(pair[0]), parse(pair[1]));
            assert_eq!(a.compare(&b), Ordering::Less, "{a:?} < {b:?}");
            assert_eq!(b.compare(&a), Ordering::Greater, "{b:?} > {a:?}");
        }
        assert_eq!(
            parse("v1.2.3+meta").compare(&parse("v1.2.3")),
            Ordering::Equal
        );
    }

    #[test]
    fn builds_pseudo_versions_like_go() {
        let ts = "20261022081434";
        let rev = "ab1b465ca537";
        let cases = [
            (None, "v0.0.0-20261022081434-ab1b465ca537"),
            (Some("v0.3.0"), "v0.3.1-0.20261022081434-ab1b465ca537"),
            (
                Some("v0.3.0-rc.1"),
                "v0.3.0-rc.1.0.20261022081434-ab1b465ca537",
            ),
            (
                Some("v0.3.0+meta"),
                "v0.3.1-0.20261022081434-ab1b465ca537+meta",
            ),
            (Some("v1.9.9"), "v1.9.10-0.20261022081434-ab1b465ca537"),
            (Some("v1.99"), "v1.99.1-0.20261022081434-ab1b465ca537"),
        ];
        for (base, want) in cases {
            let base = base.map(parse);
            assert_eq!(pseudo_version(base.as_ref(), ts, rev), want);
        }
    }
}
