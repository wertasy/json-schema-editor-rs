//! `$ref` resolution and cross-document loading (`$defs`, remote refs).

use super::*;
use crate::doc::jsoncdoc;

impl Parser {
    pub(super) fn cached_load(&mut self, loc: &str) -> Result<NodeId, SmodelError> {
        if let Some(&d) = self.docs.get(loc) {
            return Ok(d);
        }
        let data = fetch::load(loc).map_err(SmodelError)?;
        let text = String::from_utf8(data)
            .map_err(|_| SmodelError(format!("{loc}: invalid UTF-8 in schema document")))?;
        let parsed =
            jsoncdoc::parse_strict(&text).map_err(|e| SmodelError(format!("{loc}: {e}")))?;
        let root = graft(&mut self.doc, &parsed);
        self.docs.insert(loc.to_string(), root);
        Ok(root)
    }

    /// resolveRef resolves a $ref value against ctx, loading remote
    /// documents through the parser cache.
    pub(super) fn resolve_ref(
        &mut self,
        r: &str,
        ctx: &RefCtx,
        depth: usize,
    ) -> Result<(NodeId, RefCtx), String> {
        if let Some(ptr) = r.strip_prefix('#') {
            let _ = ptr;
            return match self.pointer_at(ctx.doc_root, r) {
                Some(t) => Ok((t, ctx.clone())),
                None => Err(format!("unresolvable pointer {r}")),
            };
        }
        if depth > MAX_DEPTH {
            return Err("ref chain too deep".into());
        }
        let base = Url::parse(&doc_base(&self.doc, ctx)).map_err(|e| e.to_string())?;
        let abs = base.join(r).map_err(|e| e.to_string())?;
        let frag = abs.fragment().unwrap_or("").to_string();
        let mut doc_url = abs.clone();
        doc_url.set_fragment(None);
        let doc_loc = doc_url.to_string();
        let doc_root = self.cached_load(&doc_loc).map_err(|e| e.0)?;
        let target = self
            .pointer_at(doc_root, &frag)
            .ok_or_else(|| format!("unresolvable pointer {frag} in {doc_loc}"))?;
        Ok((
            target,
            RefCtx {
                doc_root,
                loc: doc_loc,
            },
        ))
    }

    /// JSON pointer resolution rooted at an arbitrary document root (the
    /// shared arena holds several grafted documents); mirrors
    /// `Doc::resolve_json_pointer`.
    pub(super) fn pointer_at(&self, start: NodeId, ptr: &str) -> Option<NodeId> {
        let ptr = ptr.strip_prefix('#').unwrap_or(ptr);
        if ptr.is_empty() {
            return Some(start);
        }
        let decoded = percent_decode(ptr);
        let ptr: &str = decoded.as_deref().unwrap_or(ptr);
        if ptr.is_empty() {
            return Some(start);
        }
        let mut cur = start;
        for raw in ptr.strip_prefix('/').unwrap_or(ptr).split('/') {
            let tok = raw.replace("~1", "/").replace("~0", "~");
            match self.doc.value(cur) {
                Value::Object(members) => {
                    cur = members.iter().find(|m| m.key == tok)?.val;
                }
                Value::Array(items) => {
                    cur = *items.get(tok.parse::<usize>().ok()?)?;
                }
                _ => return None,
            }
        }
        Some(cur)
    }
}
