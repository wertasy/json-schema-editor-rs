#![allow(dead_code)] // wired into the menu in later phases
//! One line per read: Rust port of Go `internal/ui/lineio.go`.
//!
//! huh's accessible prompts each read the form input greedily; with a pipe
//! feeding a whole answer script, the first prompt would swallow every
//! remaining line, leaving later prompts at EOF. Serving one line per
//! [`LineReader::read_line`] call keeps sequential prompts in lockstep with
//! the script (and behaves identically to a tty's line buffering).

use std::io::BufRead;

/// LineReader delivers at most one line (up to and including '\n') per
/// [`read_line`](Self::read_line) call; the trailing newline is stripped.
pub struct LineReader<R: BufRead> {
    src: R,
    pending: Vec<u8>,
    /// underlying reader exhausted
    src_eof: bool,
    /// EOF has been handed to a caller at least once
    saw_eof: bool,
}

impl<R: BufRead> LineReader<R> {
    pub fn new(src: R) -> LineReader<R> {
        LineReader {
            src,
            pending: Vec::new(),
            src_eof: false,
            saw_eof: false,
        }
    }

    /// Reads one line; `None` marks EOF (empty buffer at end of input).
    /// A read error on the underlying source is treated like EOF, mirroring
    /// the Go reader.
    pub fn read_line(&mut self) -> Option<String> {
        loop {
            if let Some(i) = self.pending.iter().position(|&b| b == b'\n') {
                let mut line: Vec<u8> = self.pending.drain(..=i).collect();
                line.pop(); // strip '\n'
                return Some(String::from_utf8_lossy(&line).into_owned());
            }
            if self.src_eof {
                self.saw_eof = true;
                if self.pending.is_empty() {
                    return None;
                }
                let line = String::from_utf8_lossy(&self.pending).into_owned();
                self.pending.clear();
                return Some(line);
            }
            match self.src.fill_buf() {
                Ok(chunk) if chunk.is_empty() => self.src_eof = true,
                Ok(chunk) => {
                    self.pending.extend_from_slice(chunk);
                    let n = chunk.len();
                    self.src.consume(n);
                }
                Err(_) => self.src_eof = true,
            }
        }
    }

    /// EOFSeen reports whether input has been exhausted, so drivers can stop
    /// instead of looping on defaulted prompts.
    pub fn eof_seen(&self) -> bool {
        self.saw_eof
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn lines(src: &str) -> Vec<Option<String>> {
        let mut lr = LineReader::new(Cursor::new(src.to_string()));
        let mut out = Vec::new();
        loop {
            let l = lr.read_line();
            out.push(l.clone());
            if l.is_none() {
                assert!(lr.eof_seen(), "eof_seen must be set when EOF is handed out");
                break;
            }
        }
        out
    }

    #[test]
    fn one_line_per_read() {
        assert_eq!(
            lines("a\nb\nc\n"),
            vec![
                Some("a".to_string()),
                Some("b".to_string()),
                Some("c".to_string()),
                None,
            ]
        );
    }

    #[test]
    fn last_line_without_newline_is_served_then_eof() {
        assert_eq!(
            lines("a\nb\nc"),
            vec![
                Some("a".to_string()),
                Some("b".to_string()),
                Some("c".to_string()),
                None
            ]
        );
    }

    #[test]
    fn empty_input_is_immediate_eof() {
        assert_eq!(lines(""), vec![None]);
    }

    #[test]
    fn blank_lines_are_preserved() {
        assert_eq!(
            lines("a\n\nb\n"),
            vec![
                Some("a".into()),
                Some(String::new()),
                Some("b".into()),
                None
            ]
        );
    }

    #[test]
    fn eof_seen_only_after_exhaustion() {
        let mut lr = LineReader::new(Cursor::new("one\n".to_string()));
        assert!(!lr.eof_seen());
        assert_eq!(lr.read_line(), Some("one".to_string()));
        assert!(!lr.eof_seen());
        assert_eq!(lr.read_line(), None);
        assert!(lr.eof_seen());
        assert_eq!(lr.read_line(), None);
        assert!(lr.eof_seen());
    }

    /// A reader that dumps its entire buffer in one read — the greedy-scan
    /// scenario the Go lineReader exists for.
    struct Greedy(Vec<u8>, bool);

    impl std::io::Read for Greedy {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1 {
                return Ok(0);
            }
            self.1 = true;
            let n = self.0.len().min(buf.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            Ok(n)
        }
    }

    impl BufRead for Greedy {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            if self.1 { Ok(&[]) } else { Ok(&self.0) }
        }
        fn consume(&mut self, n: usize) {
            self.0.drain(..n);
        }
    }

    #[test]
    fn greedy_read_still_yields_one_line_per_call() {
        let mut lr = LineReader::new(Greedy(b"yes\nno\nsave\n".to_vec(), false));
        assert_eq!(lr.read_line(), Some("yes".to_string()));
        assert_eq!(lr.read_line(), Some("no".to_string()));
        assert!(!lr.eof_seen());
        assert_eq!(lr.read_line(), Some("save".to_string()));
        assert_eq!(lr.read_line(), None);
        assert!(lr.eof_seen());
    }

    #[test]
    fn utf8_multibyte_lines() {
        assert_eq!(
            lines("名字\n值\n"),
            vec![Some("名字".to_string()), Some("值".to_string()), None]
        );
    }
}
