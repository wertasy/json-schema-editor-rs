mod common;
use common::{PtySession, PtySpawn, repo_root};

#[test]
fn dbg_real_bash() {
    let dir = tempfile::tempdir().unwrap();
    let schema = dir.path().join("s.json");
    let cfg = dir.path().join("c.json");
    std::fs::write(&schema, r#"{"type":"object"}"#).unwrap();
    std::fs::write(&cfg, r#"{"name":"x"}"#).unwrap();
    let mut s = match PtySession::spawn(
        repo_root(),
        &["-s", schema.to_str().unwrap(), cfg.to_str().unwrap()],
        &[],
    ) {
        PtySpawn::Ok(s) => s,
        PtySpawn::Unavailable(e) => panic!("{e}"),
    };
    s.wait_frame("root", 0, &["↑/↓ move"]);
    s.send(b":");
    s.wait_frame("prompt", 0, &[": command"]);
    std::thread::sleep(std::time::Duration::from_millis(200));
    let b0 = s.snapshot().len();
    s.send(b"!git st");
    std::thread::sleep(std::time::Duration::from_millis(400));
    let b_mid = s.snapshot().len();
    let snap = s.snapshot();
    println!(
        "=== mid-typing ===\n{}",
        &snap[snap.len().saturating_sub(400)..]
    );
    s.send(b"\t");
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let snap = s.snapshot();
    println!("=== after !git st TAB ===\n{}", &snap[b0..]);
    let b1 = s.snapshot().len();
    s.send(b"\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f"); // clear "git st"
    std::thread::sleep(std::time::Duration::from_millis(200));
    s.send(b"!systemctl status\t");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let snap = s.snapshot();
    println!("=== after systemctl TAB ===\n{}", &snap[b1..]);
}
