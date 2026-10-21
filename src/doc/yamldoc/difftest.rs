// THROWAWAY differential test: Rust yamldoc vs the Go probe over the corpus.
#[cfg(test)]
mod tests {
    use crate::doc::codec_for;

    fn chain(a: std::fs::ReadDir, b: std::fs::ReadDir) -> impl Iterator<Item = std::path::PathBuf> {
        a.filter_map(|e| e.ok())
            .map(|e| e.path())
            .chain(b.filter_map(|e| e.ok()).map(|e| e.path()))
    }

    #[test]
    fn differential_vs_go() {
        // Requires the Go probe harness (/tmp/yamlprobe); skips elsewhere.
        if !std::path::Path::new("/tmp/yamlprobe/probe").exists() {
            eprintln!("skipping: no Go probe at /tmp/yamlprobe/probe");
            return;
        }
        let mut checked = 0;
        let mut failed = Vec::new();
        let corpus = chain(
            std::fs::read_dir("/tmp/yamlprobe/cases").unwrap(),
            std::fs::read_dir("/tmp/yamlprobe/cases2").unwrap(),
        );
        let mut paths: Vec<_> = corpus
            .filter(|p| p.extension().map_or(false, |e| e == "yaml"))
            .collect();
        paths.sort();
        for path in paths {
            let src = std::fs::read_to_string(&path).unwrap();
            let go = std::process::Command::new("/tmp/yamlprobe/probe")
                .arg(&path)
                .output()
                .unwrap();
            let c = codec_for(&path.to_string_lossy());
            let rust_out: Result<String, String> = match c.parse(&src) {
                Ok(p) => c.write(&p.doc, &p.meta).map_err(|e| e.msg),
                Err(e) => Err(e.msg),
            };
            let go_out = if go.status.success() {
                Ok(String::from_utf8_lossy(&go.stdout).to_string())
            } else {
                Err(String::from_utf8_lossy(&go.stderr).to_string())
            };
            checked += 1;
            match (&rust_out, &go_out) {
                (Ok(r), Ok(g)) if r == g => {}
                (Err(re), Err(ge)) => {
                    // both error: message texts are library-specific for parse
                    // errors; only count as match when both failed
                    let _ = (re, ge);
                }
                _ => failed.push(format!(
                    "=== {}\n--go--\n{:?}\n--rust--\n{:?}\n--go stderr: {:?}",
                    path.display(),
                    go_out,
                    rust_out,
                    String::from_utf8_lossy(&go.stderr)
                )),
            }
        }
        assert!(
            failed.is_empty(),
            "{} of {} differ:\n{}",
            failed.len(),
            checked,
            failed.join("\n")
        );
    }
}
