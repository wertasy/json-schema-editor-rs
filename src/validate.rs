//! Authoritative save-time validation using the jsonschema library. It is
//! the backstop for everything the form layer does not fully enforce.
//!
//! Rust port of `internal/validate/validate.go`. Library-dependent error
//! WORDING intentionally differs from Go's jsonschema v6; the error
//! structure (leaf aggregation, `at '$.path'` document paths) is identical.

use crate::doc::jsondoc::{self, Doc, PathSeg};
use crate::fetch;
use jsonschema::Retrieve;
use serde_json::Value;

/// httpLoader adapts fetch::load so remote documents (the root schema and
/// nested remote $refs) share the schema cache and the per-process memo
/// instead of downloading again.
struct FetchRetriever;

impl Retrieve for FetchRetriever {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let data = fetch::load(uri.as_str())?;
        Ok(serde_json::from_slice(&data)?)
    }
}

/// V is a compiled schema ready to validate configuration documents.
pub struct V {
    validator: jsonschema::Validator,
}

/// New compiles the schema at loc (local path, http(s):// or file:// URL).
/// Remote documents (root and nested remote $refs) flow through the
/// fetch-backed retriever.
pub fn new(loc: &str) -> Result<V, String> {
    let canon = fetch::canonical_loc(loc)?;
    let data = fetch::load(&canon)?;
    // Local schemas parse through the document tree like Go's AddResource
    // path (encoding/json-compatible semantics, "%s: %w" error wrapping).
    let doc =
        jsondoc::parse(&String::from_utf8_lossy(&data)).map_err(|e| format!("{canon}: {e}"))?;
    let schema = doc.to_value(doc.root);

    let validator = jsonschema::options()
        .with_retriever(FetchRetriever)
        .with_base_uri(canon)
        .build(&schema)
        .map_err(|e| e.to_string())?;
    Ok(V { validator })
}

impl V {
    /// Check validates cfg and returns one message per leaf error, like
    /// "at '$.server.port': minimum: got 80, want 1,024". Empty result means
    /// valid.
    pub fn check(&self, cfg: &Doc) -> Vec<String> {
        let value = cfg.to_value(cfg.root);
        self.validator
            .iter_errors(&value)
            .map(|err| with_doc_path(&err, cfg))
            .collect()
    }
}

/// withDocPath renders the library's instance location as the editor's
/// document path ("at '$.a.b[0]': <detail>"). The path is rebuilt from the
/// error's JSON pointer rather than parsed back out of the message; the
/// document at each level decides whether a segment is an array index or an
/// object key, so a numeric key stays ".123" instead of turning into "[123]".
fn with_doc_path(err: &jsonschema::ValidationError<'_>, doc: &Doc) -> String {
    let pointer = err.instance_path().as_str();
    let mut path: Vec<PathSeg> = Vec::new();
    let mut cur = Some(doc.root);
    for raw in pointer.split('/').skip(1) {
        let Some(node) = cur else { break };
        let seg = raw.replace("~1", "/").replace("~0", "~");
        let step = match doc.value(node) {
            jsondoc::Value::Array(_) => seg
                .parse::<usize>()
                .map(PathSeg::Idx)
                .unwrap_or(PathSeg::Key(seg)),
            _ => PathSeg::Key(seg),
        };
        cur = doc.get(&extend(&path, &step));
        path.push(step);
    }
    format!("at '{}': {msg}", jsondoc::path_string(&path), msg = err)
}

fn extend(path: &[PathSeg], seg: &PathSeg) -> Vec<PathSeg> {
    let mut out = path.to_vec();
    out.push(seg.clone());
    out
}
