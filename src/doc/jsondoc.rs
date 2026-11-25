//! Order-preserving JSON document tree (arena).
//!
//! Rust port of `internal/jsondoc/jsondoc.go`. The Go node graph (shared
//! `*Node` pointers for YAML aliasing) becomes an arena: nodes live in
//! `Doc::nodes` and are addressed by `NodeId`. Sharing one node — the whole
//! point of YAML aliases and merge keys — is sharing one `NodeId`, which
//! `Doc::clone()` (undo snapshots) preserves by construction.
//!
//! Numbers keep their exact literal ("1.0" stays "1.0", "1e3" stays "1e3").

use std::collections::HashSet;

/// One node in the arena: index into [`Doc::nodes`].
pub type NodeId = u32;

/// The value of one node; exactly one variant is inhabited (Null for JSON null).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Object(Vec<Member>),
    Array(Vec<NodeId>),
    Str(String),
    /// The exact numeric literal as written.
    Num(String),
    Bool(bool),
    Null,
}

impl Default for Value {
    fn default() -> Value {
        Value::Null
    }
}

/// One key/value entry of an object, in document order.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub key: String,
    pub val: NodeId,
}

/// Anchor name and merge refs ride the node next to its value, mirroring
/// `jsondoc.Node`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct NodeData {
    pub value: Value,
    /// YAML anchor name ("" for plain JSON data).
    pub anchor: String,
    /// YAML merge-key refs: the anchored mapping nodes this mapping merges,
    /// in sequence order. Only the value of a "<<" member ever holds refs;
    /// such a node carries `Value::Null`.
    pub merge: Vec<NodeId>,
    /// TOML: the exact source literal when the Doc form is canonicalized
    /// (non-decimal integers, date/time values). Empty for every other
    /// format and for values the editor created. A value-neutral write keeps
    /// it (see `carry_raw`); a changed value replaces the node and drops it.
    pub raw: String,
}

/// One document: the arena plus its root. `Clone` preserves alias sharing
/// (same `NodeId`s), so undo snapshots keep references intact.
#[derive(Clone, Debug)]
pub struct Doc {
    pub nodes: Vec<NodeData>,
    pub root: NodeId,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{msg}")]
pub struct DocError {
    pub msg: String,
}

fn err<T>(msg: impl Into<String>) -> Result<T, DocError> {
    Err(DocError { msg: msg.into() })
}

/// Policy for a cross-arena subtree copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyMode {
    /// Keep the source spelling and alias identity: `raw`, merge refs and
    /// anchor names ride along (schema defaults, values moved between docs).
    Preserve,
    /// Materialize the resolved value: merge members become plain members,
    /// anchors/merge refs are dropped and `raw` is cleared (pasted content).
    Detached,
}

impl Doc {
    pub fn new() -> Doc {
        let mut d = Doc {
            nodes: Vec::new(),
            root: 0,
        };
        let root = d.push(NodeData::default());
        d.root = root;
        d
    }

    pub fn push(&mut self, data: NodeData) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(data);
        id
    }

    // --- constructors -----------------------------------------------------

    pub fn new_object(&mut self) -> NodeId {
        self.push(NodeData {
            value: Value::Object(Vec::new()),
            ..Default::default()
        })
    }
    pub fn new_array(&mut self) -> NodeId {
        self.push(NodeData {
            value: Value::Array(Vec::new()),
            ..Default::default()
        })
    }
    pub fn new_str(&mut self, s: impl Into<String>) -> NodeId {
        self.push(NodeData {
            value: Value::Str(s.into()),
            ..Default::default()
        })
    }
    pub fn new_num(&mut self, lit: impl Into<String>) -> NodeId {
        self.push(NodeData {
            value: Value::Num(lit.into()),
            ..Default::default()
        })
    }
    pub fn new_bool(&mut self, b: bool) -> NodeId {
        self.push(NodeData {
            value: Value::Bool(b),
            ..Default::default()
        })
    }
    pub fn new_null(&mut self) -> NodeId {
        self.push(NodeData {
            value: Value::Null,
            ..Default::default()
        })
    }

    // --- accessors ----------------------------------------------------------

    pub fn node(&self, id: NodeId) -> &NodeData {
        &self.nodes[id as usize]
    }
    pub fn value(&self, id: NodeId) -> &Value {
        &self.nodes[id as usize].value
    }

    /// valueOpt returns the node's value, None for an out-of-range id.
    pub fn value_opt(&self, id: NodeId) -> Option<&Value> {
        self.nodes.get(id as usize).map(|n| &n.value)
    }

    /// Get returns the node at path; key segments index object keys, index
    /// segments array items.
    pub fn get(&self, path: &[PathSeg]) -> Option<NodeId> {
        let mut cur = self.root;
        for p in path {
            cur = match (&self.nodes[cur as usize].value, p) {
                (Value::Object(members), PathSeg::Key(k)) => {
                    members.iter().find(|m| &m.key == k)?.val
                }
                (Value::Array(items), PathSeg::Idx(i)) => items.get(*i).copied()?,
                _ => return None,
            };
        }
        Some(cur)
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut NodeData {
        &mut self.nodes[id as usize]
    }

    fn member_index(&self, n: NodeId, key: &str) -> Option<usize> {
        match &self.nodes[n as usize].value {
            Value::Object(members) => members.iter().position(|m| m.key == key),
            _ => None,
        }
    }

    /// empty reports whether the node holds no value at all (JSON null),
    /// i.e. it can be replaced by a container without data loss.
    pub fn empty(&self, n: NodeId) -> bool {
        matches!(self.nodes[n as usize].value, Value::Null)
    }

    fn container_for(&mut self, seg: &PathSeg) -> NodeId {
        match seg {
            PathSeg::Idx(_) => self.new_array(),
            PathSeg::Key(_) => self.new_object(),
        }
    }

    /// suitable reports whether n can be indexed by `next` without loss.
    fn suitable(&self, n: NodeId, next: &PathSeg) -> bool {
        match next {
            PathSeg::Idx(_) => {
                matches!(self.nodes[n as usize].value, Value::Array(_)) || self.empty(n)
            }
            PathSeg::Key(_) => {
                matches!(self.nodes[n as usize].value, Value::Object(_)) || self.empty(n)
            }
        }
    }

    // --- mutation -----------------------------------------------------------

    /// Set writes v at path, auto-creating missing intermediate containers
    /// (object for key segments, array for index segments). Existing keys
    /// keep their position; new keys are appended. A current value carrying
    /// a YAML anchor is written through in place with its anchor kept.
    /// Structural conflicts (indexing a scalar) or out-of-range array
    /// indexes are errors — paths come from the schema-driven navigation
    /// stack, so those are programmer errors.
    pub fn set(&mut self, path: &[PathSeg], v: NodeId) -> Result<(), DocError> {
        if path.is_empty() {
            self.nodes[self.root as usize] = self.nodes[v as usize].clone();
            return Ok(());
        }
        let mut cur = self.root;
        for i in 0..path.len() {
            let last = i == path.len() - 1;
            match &path[i] {
                PathSeg::Key(k) => {
                    if !matches!(self.nodes[cur as usize].value, Value::Object(_)) {
                        if !self.empty(cur) {
                            return err(format!("jsondoc: Set string key {k:?} into non-object"));
                        }
                        let c = self.container_for(&path[i]);
                        let data = self.nodes[c as usize].clone();
                        self.nodes[cur as usize] = data;
                    }
                    if last {
                        self.overwrite_member(cur, k, v);
                        return Ok(());
                    }
                    let next = path[i + 1].clone();
                    let child = match self.member_index(cur, k) {
                        Some(ci) => {
                            // Go keeps an empty or non-empty unsuitable child
                            // here: an empty one is converted in place at the
                            // next level, a real scalar errors there.
                            let c = match &self.nodes[cur as usize].value {
                                Value::Object(m) => m[ci].val,
                                _ => unreachable!(),
                            };
                            c
                        }
                        None => {
                            let nc = self.container_for(&next);
                            self.set_member(cur, k, nc);
                            nc
                        }
                    };
                    cur = child;
                }
                PathSeg::Idx(k) => {
                    let k = *k;
                    if !matches!(self.nodes[cur as usize].value, Value::Array(_)) {
                        if !self.empty(cur) {
                            return err(format!("jsondoc: Set index {k} into non-array"));
                        }
                        let c = self.container_for(&path[i]);
                        let data = self.nodes[c as usize].clone();
                        self.nodes[cur as usize] = data;
                    }
                    let len = match &self.nodes[cur as usize].value {
                        Value::Array(items) => items.len(),
                        _ => unreachable!(),
                    };
                    if k == len {
                        // append
                        if last {
                            if let Value::Array(items) = &mut self.nodes[cur as usize].value {
                                items.push(v);
                            }
                            return Ok(());
                        }
                        let child = self.container_for(&path[i + 1]);
                        if let Value::Array(items) = &mut self.nodes[cur as usize].value {
                            items.push(child);
                        }
                        cur = child;
                    } else if k < len {
                        if last {
                            self.overwrite_item(cur, k, v);
                            return Ok(());
                        }
                        let old = match &self.nodes[cur as usize].value {
                            Value::Array(items) => items[k],
                            _ => unreachable!(),
                        };
                        let next = path[i + 1].clone();
                        let child = if self.empty(old) || !self.suitable(old, &next) {
                            let nc = self.container_for(&next);
                            if let Value::Array(items) = &mut self.nodes[cur as usize].value {
                                items[k] = nc;
                            }
                            nc
                        } else {
                            old
                        };
                        cur = child;
                    } else {
                        return err(format!(
                            "jsondoc: Set array index {k} out of range (len {len})"
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn set_member(&mut self, n: NodeId, key: &str, v: NodeId) {
        if let Value::Object(members) = &mut self.nodes[n as usize].value {
            for m in members.iter_mut() {
                if m.key == key {
                    m.val = v;
                    return;
                }
            }
            members.push(Member {
                key: key.to_string(),
                val: v,
            });
        }
    }

    /// A value-neutral write keeps the node's exact TOML source spelling.
    /// Editing a 0x1F field and submitting the displayed 31 must not
    /// canonicalize the file just because the value was re-applied.
    fn carry_raw(&mut self, old: NodeId, v: NodeId) {
        if v != old
            && self.nodes[v as usize].raw.is_empty()
            && !self.nodes[old as usize].raw.is_empty()
            && self.nodes[v as usize].value == self.nodes[old as usize].value
        {
            self.nodes[v as usize].raw = self.nodes[old as usize].raw.clone();
        }
    }

    /// Set's member write at the last path segment. A current value carrying
    /// a YAML anchor is written through in place with its anchor kept: the
    /// node may be shared with alias occurrences, and swapping the id would
    /// silently detach them — every reference sees the write. pub(crate) so
    /// the fixture tests in doc::jsondoc_tests can exercise plain member
    /// replacement directly.
    pub(crate) fn overwrite_member(&mut self, n: NodeId, key: &str, v: NodeId) {
        let found = {
            let Value::Object(members) = &mut self.nodes[n as usize].value else {
                return;
            };
            match members.iter().position(|m| m.key == key) {
                Some(idx) => Some((idx, members[idx].val)),
                None => {
                    members.push(Member {
                        key: key.to_string(),
                        val: v,
                    });
                    None
                }
            }
        };
        let Some((idx, old)) = found else { return };
        self.carry_raw(old, v);
        if self.nodes[old as usize].anchor.is_empty() {
            if let Value::Object(members) = &mut self.nodes[n as usize].value {
                members[idx].val = v;
            }
        } else {
            let anchor = self.nodes[old as usize].anchor.clone();
            let mut data = self.nodes[v as usize].clone();
            data.anchor = anchor;
            self.nodes[old as usize] = data;
        }
    }

    /// Set's array write at the last path segment, with the same anchored
    /// write-through as [`Doc::overwrite_member`].
    fn overwrite_item(&mut self, n: NodeId, idx: usize, v: NodeId) {
        let old = match &mut self.nodes[n as usize].value {
            Value::Array(items) => items[idx],
            _ => return,
        };
        self.carry_raw(old, v);
        if self.nodes[old as usize].anchor.is_empty() {
            if let Value::Array(items) = &mut self.nodes[n as usize].value {
                items[idx] = v;
            }
        } else {
            let anchor = self.nodes[old as usize].anchor.clone();
            let mut data = self.nodes[v as usize].clone();
            data.anchor = anchor;
            self.nodes[old as usize] = data;
        }
    }

    /// Detach replaces the node at path with v even when that node is an
    /// anchored (possibly alias-shared) node: materializing an alias into an
    /// independent copy must break the reference, unlike [`Doc::set`] which
    /// writes through to the shared node. Reports whether the path existed.
    pub fn detach(&mut self, path: &[PathSeg], v: NodeId) -> bool {
        if path.is_empty() {
            self.nodes[self.root as usize] = self.nodes[v as usize].clone();
            return true;
        }
        let Some(parent) = self.get(&path[..path.len() - 1]) else {
            return false;
        };
        match &path[path.len() - 1] {
            PathSeg::Key(k) => {
                if let Value::Object(members) = &mut self.nodes[parent as usize].value {
                    for m in members.iter_mut() {
                        if &m.key == k {
                            m.val = v;
                            return true;
                        }
                    }
                }
                false
            }
            PathSeg::Idx(i) => {
                if let Value::Array(items) = &mut self.nodes[parent as usize].value {
                    if *i < items.len() {
                        items[*i] = v;
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Remove deletes the node at path, reporting whether it existed.
    pub fn remove(&mut self, path: &[PathSeg]) -> bool {
        if path.is_empty() {
            return false;
        }
        let Some(parent) = self.get(&path[..path.len() - 1]) else {
            return false;
        };
        match &path[path.len() - 1] {
            PathSeg::Key(k) => {
                if let Value::Object(members) = &mut self.nodes[parent as usize].value {
                    if let Some(i) = members.iter().position(|m| &m.key == k) {
                        members.remove(i);
                        return true;
                    }
                }
                false
            }
            PathSeg::Idx(i) => {
                if let Value::Array(items) = &mut self.nodes[parent as usize].value {
                    if *i < items.len() {
                        items.remove(*i);
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Append adds v to the array at path, auto-creating intermediates and
    /// the array itself. Errors if the path resolves to a non-array.
    pub fn append(&mut self, path: &[PathSeg], v: NodeId) -> Result<(), DocError> {
        let arr = match self.get(path) {
            Some(a) => {
                if !matches!(self.nodes[a as usize].value, Value::Array(_)) {
                    if !self.empty(a) {
                        return err("jsondoc: Append into non-array");
                    }
                    let last = path.last().cloned().unwrap_or(PathSeg::Key(String::new()));
                    let c = self.container_for(&last);
                    let data = self.nodes[c as usize].clone();
                    self.nodes[a as usize] = data;
                    a
                } else {
                    a
                }
            }
            None => {
                let arr_node = self.new_array();
                self.set(path, arr_node)?;
                self.get(path).expect("set just created it")
            }
        };
        if let Value::Array(items) = &mut self.nodes[arr as usize].value {
            items.push(v);
            Ok(())
        } else {
            err("jsondoc: Append into non-array")
        }
    }

    /// Insert splices v into the array at path at index idx, shifting the
    /// elements from idx on by one; idx clamps to [0, len]. Auto-creates
    /// intermediates and the array itself, like [`Doc::append`].
    pub fn insert(&mut self, path: &[PathSeg], idx: usize, v: NodeId) -> Result<(), DocError> {
        let arr = self.array_at(path)?;
        let len = match &self.nodes[arr as usize].value {
            Value::Array(items) => items.len(),
            _ => unreachable!(),
        };
        let idx = idx.min(len);
        if let Value::Array(items) = &mut self.nodes[arr as usize].value {
            items.insert(idx, v);
        }
        Ok(())
    }

    /// InsertMember adds key=v to the object at path at member index idx,
    /// shifting the members from idx on by one; idx clamps to [0, len]. An
    /// existing key keeps its position and only gets its value replaced
    /// (with the anchored write-through). Auto-creates missing intermediates
    /// and the object itself; errors if the path resolves to a non-object.
    pub fn insert_member(
        &mut self,
        path: &[PathSeg],
        key: &str,
        idx: usize,
        v: NodeId,
    ) -> Result<(), DocError> {
        let obj = match self.get(path) {
            Some(o) => o,
            None => {
                // the menu target was never written to the document: build the chain
                let obj_node = self.new_object();
                self.set(path, obj_node)?;
                self.get(path).expect("set just created it")
            }
        };
        if !matches!(self.nodes[obj as usize].value, Value::Object(_)) {
            if !self.empty(obj) {
                return err("jsondoc: InsertMember into non-object");
            }
            let c = self.new_object();
            let data = self.nodes[c as usize].clone();
            self.nodes[obj as usize] = data;
        }
        if self.member_index(obj, key).is_some() {
            self.overwrite_member(obj, key, v);
            return Ok(());
        }
        let len = match &self.nodes[obj as usize].value {
            Value::Object(members) => members.len(),
            _ => unreachable!(),
        };
        let idx = idx.min(len);
        if let Value::Object(members) = &mut self.nodes[obj as usize].value {
            members.insert(
                idx,
                Member {
                    key: key.to_string(),
                    val: v,
                },
            );
        }
        Ok(())
    }

    /// Move relocates the array element at index from so that it ends up at
    /// index to, keeping the other elements' relative order; from == to is a
    /// no-op success. Reports false when the path is missing, does not
    /// resolve to an array, or either index is out of range.
    pub fn move_(&mut self, path: &[PathSeg], from: usize, to: usize) -> bool {
        let Some(parent) = self.get(path) else {
            return false;
        };
        let Value::Array(items) = &mut self.nodes[parent as usize].value else {
            return false;
        };
        if from >= items.len() || to >= items.len() {
            return false;
        }
        if from == to {
            return true;
        }
        let el = items.remove(from);
        items.insert(to, el);
        true
    }

    fn array_at(&mut self, path: &[PathSeg]) -> Result<NodeId, DocError> {
        match self.get(path) {
            Some(a) => {
                if !matches!(self.nodes[a as usize].value, Value::Array(_)) {
                    if !self.empty(a) {
                        return err("jsondoc: Insert into non-array");
                    }
                    let last = path.last().cloned().unwrap_or(PathSeg::Key(String::new()));
                    let c = self.container_for(&last);
                    let data = self.nodes[c as usize].clone();
                    self.nodes[a as usize] = data;
                    Ok(a)
                } else {
                    Ok(a)
                }
            }
            None => {
                let arr_node = self.new_array();
                self.set(path, arr_node)?;
                Ok(self.get(path).expect("set just created it"))
            }
        }
    }

    // --- merge resolution -----------------------------------------------------

    /// MergeRefs lists the anchored mapping nodes merged by this mapping's
    /// "<<" members in document order. Empty when the node merges nothing or
    /// is not a mapping.
    pub fn merge_refs(&self, n: NodeId) -> Vec<NodeId> {
        let mut refs = Vec::new();
        if let Value::Object(members) = &self.nodes[n as usize].value {
            for m in members {
                refs.extend(self.nodes[m.val as usize].merge.iter().copied());
            }
        }
        refs
    }

    /// ResolvedMembers returns the mapping's own members plus the ones its
    /// merge keys pull in, with YAML precedence applied (own keys win,
    /// earlier anchors win over later ones). The member values are the
    /// document's own nodes — for merged keys those live in the anchor's
    /// object, so edits through them reach the source mapping.
    pub fn resolved_members(&self, n: NodeId) -> Vec<Member> {
        match &self.nodes[n as usize].value {
            Value::Object(members) => self.resolve_members(members),
            _ => Vec::new(),
        }
    }

    /// resolveMembers flattens one mapping's member list for value-oriented
    /// consumers (JSON output, detached copies): own members keep their
    /// order, then the merged members follow in "<<: *a *b" order, each key
    /// at most once. A reference already being resolved (a merge cycle)
    /// contributes nothing.
    fn resolve_members<'a>(&'a self, members: &'a [Member]) -> Vec<Member> {
        let mut out = Vec::with_capacity(members.len());
        let mut seen: HashSet<&'a str> = HashSet::new();
        let mut busy: HashSet<NodeId> = HashSet::new();
        self.add_members(members, &mut seen, &mut out, &mut busy);
        out
    }

    fn add_members<'a>(
        &'a self,
        members: &'a [Member],
        seen: &mut HashSet<&'a str>,
        out: &mut Vec<Member>,
        busy: &mut HashSet<NodeId>,
    ) {
        // own members first: explicit keys win over merged ones
        for m in members {
            if !self.nodes[m.val as usize].merge.is_empty() {
                continue;
            }
            if seen.insert(m.key.as_str()) {
                out.push(m.clone());
            }
        }
        // then the merge members in order: earlier anchors win over later
        for m in members {
            let refs = &self.nodes[m.val as usize].merge;
            if refs.is_empty() {
                continue;
            }
            for &r in refs {
                let Value::Object(inner) = &self.nodes[r as usize].value else {
                    continue;
                };
                if !busy.insert(r) {
                    continue;
                }
                self.add_members(inner, seen, out, busy);
                busy.remove(&r);
            }
        }
    }

    // --- value bridge ---------------------------------------------------------

    /// to_value converts the node tree to a serde_json value for the
    /// jsonschema validator. YAML merge keys resolve along the way, so the
    /// validator sees the mapping's effective keys just as a YAML reader
    /// would. Numbers become i64/u64/f64; an invalid literal degrades to a
    /// string like Go's ToAny.
    pub fn to_value(&self, id: NodeId) -> serde_json::Value {
        self.to_value_walk(id, &mut HashSet::new())
    }

    fn to_value_walk(&self, id: NodeId, busy: &mut HashSet<NodeId>) -> serde_json::Value {
        match &self.nodes[id as usize].value {
            Value::Object(_) => {
                if !busy.insert(id) {
                    return serde_json::Value::Null; // a merge cycle: contribute nothing
                }
                let mut map = serde_json::Map::new();
                for m in self.resolved_members(id) {
                    map.insert(m.key.clone(), self.to_value_walk(m.val, busy));
                }
                busy.remove(&id);
                serde_json::Value::Object(map)
            }
            Value::Array(items) => serde_json::Value::Array(
                items
                    .iter()
                    .map(|&it| self.to_value_walk(it, busy))
                    .collect(),
            ),
            Value::Str(s) => serde_json::Value::String(s.clone()),
            Value::Num(lit) => num_to_value(lit),
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Null => serde_json::Value::Null,
        }
    }

    // --- detached copy ---------------------------------------------------------

    /// clone_detached returns a deep copy with alias sharing dissolved: every
    /// node becomes its own and the anchor identity is dropped throughout.
    /// This is how an alias occurrence is materialized (expanded). Merge-key
    /// members resolve into plain members on the way.
    pub fn clone_detached(&mut self, id: NodeId) -> NodeId {
        let value = match self.nodes[id as usize].value.clone() {
            Value::Object(_) => {
                let members = self.resolved_members(id);
                let obj = self.new_object();
                let new_members: Vec<Member> = members
                    .into_iter()
                    .map(|m| Member {
                        key: m.key,
                        val: self.clone_detached(m.val),
                    })
                    .collect();
                if let Value::Object(mm) = &mut self.nodes[obj as usize].value {
                    *mm = new_members;
                }
                return obj;
            }
            Value::Array(items) => {
                let arr = self.new_array();
                let new_items: Vec<NodeId> =
                    items.iter().map(|&it| self.clone_detached(it)).collect();
                if let Value::Array(a) = &mut self.nodes[arr as usize].value {
                    *a = new_items;
                }
                return arr;
            }
            other => other,
        };
        self.push(NodeData {
            value,
            anchor: String::new(),
            merge: Vec::new(),
            raw: self.nodes[id as usize].raw.clone(),
        })
    }

    /// Deep-copies a subtree from `src` into this arena and returns the new
    /// root id. Cross-arena copies cannot share `NodeId`s, so every child is
    /// recreated; `mode` decides whether source spelling and alias identity
    /// survive (see [`CopyMode`]).
    pub fn copy_into(&mut self, src: &Doc, id: NodeId, mode: CopyMode) -> NodeId {
        match mode {
            CopyMode::Preserve => {
                let data = src.node(id);
                let value = match &data.value {
                    Value::Object(members) => Value::Object(
                        members
                            .iter()
                            .map(|m| Member {
                                key: m.key.clone(),
                                val: self.copy_into(src, m.val, mode),
                            })
                            .collect(),
                    ),
                    Value::Array(items) => Value::Array(
                        items
                            .iter()
                            .map(|&i| self.copy_into(src, i, mode))
                            .collect(),
                    ),
                    v => v.clone(),
                };
                let merge = data
                    .merge
                    .iter()
                    .map(|&m| self.copy_into(src, m, mode))
                    .collect();
                self.push(NodeData {
                    value,
                    anchor: data.anchor.clone(),
                    merge,
                    raw: data.raw.clone(),
                })
            }
            CopyMode::Detached => {
                let value = match &src.node(id).value {
                    Value::Object(_) => Value::Object(
                        src.resolved_members(id)
                            .into_iter()
                            .map(|m| Member {
                                key: m.key,
                                val: self.copy_into(src, m.val, mode),
                            })
                            .collect(),
                    ),
                    Value::Array(items) => Value::Array(
                        items
                            .iter()
                            .map(|&i| self.copy_into(src, i, mode))
                            .collect(),
                    ),
                    v => v.clone(),
                };
                self.push(NodeData {
                    value,
                    anchor: String::new(),
                    merge: Vec::new(),
                    raw: String::new(),
                })
            }
        }
    }

    /// The canonical JSON literal of a value (Go smodel.EncodeValue). Numbers
    /// are canonicalized through `serde_json`, unlike [`Doc::compact`], which
    /// preserves the source spelling; enum matching relies on this form.
    pub fn encode_value(&self, id: NodeId) -> String {
        serde_json::to_string(&self.to_value(id)).unwrap_or_default()
    }

    /// The editable text of a scalar node, borrowed: strings and numeric
    /// literals verbatim, booleans as "true"/"false". Containers and null
    /// have no scalar text (`None`).
    pub fn scalar_text(&self, id: NodeId) -> Option<&str> {
        match self.value(id) {
            Value::Str(s) => Some(s),
            Value::Num(lit) => Some(lit),
            Value::Bool(true) => Some("true"),
            Value::Bool(false) => Some("false"),
            _ => None,
        }
    }

    /// Length of the array at `path`; 0 when absent or not an array.
    pub fn array_len(&self, path: &[PathSeg]) -> usize {
        match self.get(path).map(|id| self.value(id)) {
            Some(Value::Array(items)) => items.len(),
            _ => 0,
        }
    }

    // --- rendering ---------------------------------------------------------------

    /// Renders the document with 2-space indentation and a trailing newline.
    pub fn write(&self) -> String {
        let mut out = String::new();
        self.write_node(self.root, 0, &mut out);
        out.push('\n');
        out
    }

    /// Renders any subtree (Go's Node.String()).
    pub fn write_subtree(&self, id: NodeId) -> String {
        let mut out = String::new();
        self.write_node(id, 0, &mut out);
        out.push('\n');
        out
    }

    fn write_node(&self, id: NodeId, indent: usize, out: &mut String) {
        match &self.nodes[id as usize].value {
            Value::Object(_) => {
                // merge members resolve inline: JSON output carries no "<<"
                let members = self.resolved_members(id);
                if members.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (i, m) in members.iter().enumerate() {
                    write_indent(out, indent + 1);
                    write_json_string(out, &m.key);
                    out.push_str(": ");
                    self.write_node(m.val, indent + 1, out);
                    if i < members.len() - 1 {
                        out.push(',');
                    }
                    out.push('\n');
                }
                write_indent(out, indent);
                out.push('}');
            }
            Value::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (i, &it) in items.iter().enumerate() {
                    write_indent(out, indent + 1);
                    self.write_node(it, indent + 1, out);
                    if i < items.len() - 1 {
                        out.push(',');
                    }
                    out.push('\n');
                }
                write_indent(out, indent);
                out.push(']');
            }
            Value::Str(s) => write_json_string(out, s),
            Value::Num(lit) => out.push_str(lit),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Null => out.push_str("null"),
        }
    }

    /// Compact renders the value as one-line JSON, preserving member order
    /// and exact numeric literals.
    pub fn compact(&self, id: NodeId) -> String {
        let mut out = String::new();
        self.write_compact(id, &mut out);
        out
    }

    fn write_compact(&self, id: NodeId, out: &mut String) {
        match &self.nodes[id as usize].value {
            Value::Object(_) => {
                out.push('{');
                for (i, m) in self.resolved_members(id).iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_string(out, &m.key);
                    out.push(':');
                    self.write_compact(m.val, out);
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for (i, &it) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    self.write_compact(it, out);
                }
                out.push(']');
            }
            Value::Str(s) => write_json_string(out, s),
            Value::Num(lit) => out.push_str(lit),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Null => out.push_str("null"),
        }
    }

    /// Summary renders a single-line description for array manager screens:
    /// scalars verbatim, objects show their "title" member or "{…}", arrays
    /// show "[N items]".
    pub fn summary(&self, id: NodeId) -> String {
        match &self.nodes[id as usize].value {
            Value::Object(members) => {
                if let Some(m) = members.iter().find(|m| m.key == "title") {
                    if let Value::Str(s) = &self.nodes[m.val as usize].value {
                        return s.clone();
                    }
                }
                "{…}".to_string()
            }
            Value::Array(items) => format!("[{} items]", items.len()),
            Value::Str(s) => s.clone(),
            Value::Num(lit) => lit.clone(),
            Value::Bool(true) => "true".to_string(),
            Value::Bool(false) => "false".to_string(),
            Value::Null => "null".to_string(),
        }
    }
}

impl Default for Doc {
    fn default() -> Doc {
        Doc::new()
    }
}

/// Path segment: an object key or an array index.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PathSeg {
    Key(String),
    Idx(usize),
}

impl PathSeg {
    pub fn key(s: impl Into<String>) -> PathSeg {
        PathSeg::Key(s.into())
    }
}

// --- number conversion -------------------------------------------------------

/// numToAny: int64 when it parses as one, else float64, else the literal as
/// a string (invalid literals hand the validator a string).
fn num_to_value(lit: &str) -> serde_json::Value {
    if let Ok(i) = lit.parse::<i64>() {
        return serde_json::Value::Number(i.into());
    }
    if let Ok(f) = lit.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return serde_json::Value::Number(n);
        }
    }
    serde_json::Value::String(lit.to_string())
}

/// bump_number_literal adds delta to a numeric literal. Integer spellings
/// stay exact (i128, no f64 round trip); fractional/exponent forms fall back
/// to the shortest JSON float form.
pub fn bump_number_literal(lit: &str, delta: i64) -> Option<String> {
    if !lit.contains(['.', 'e', 'E']) {
        // Integer spellings stay exact; an integer outside i128 is left
        // untouched rather than rounded through f64.
        let v = lit.parse::<i128>().ok()?;
        return v.checked_add(delta as i128).map(|n| n.to_string());
    }
    let f = lit.parse::<f64>().ok()?;
    let sum = f + delta as f64;
    if !sum.is_finite() {
        return None;
    }
    Some(number_text(sum))
}

/// Go encoding/json float encoding: 'f' (shortest) when 1e-6 <= |f| < 1e21
/// or f == 0, else 'e' (shortest) with a negative single-digit-exponent
/// cleanup ("1e-05" → "1e-5"). Non-finite floats cannot be encoded (Go
/// errors; callers degrade to null).
pub(crate) fn number_text(f: f64) -> String {
    if !f.is_finite() {
        return String::new();
    }
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    let abs = f.abs();
    if (1e-6..1e21).contains(&abs) {
        format_float_style(f)
    } else {
        format_exp_style_json(f)
    }
}

/// Shortest round-trip decimal digits of f and the decimal point position
/// dp such that value = 0.digits × 10^dp (Go's decimal scan semantics).
fn shortest_digits(f: f64) -> (String, i32) {
    // Rust's LowerExp gives the shortest round-trip form: d[.ddd]e±X with
    // one mantissa digit before the point.
    let s = format!("{:e}", f.abs());
    let (mant, exp) = s.split_once('e').expect("LowerExp always has an exponent");
    let exp: i32 = exp.parse().expect("decimal exponent");
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    // strip trailing zeros (LowerExp keeps none beyond shortest, but be safe)
    let trimmed = digits.trim_end_matches('0');
    let digits = if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    };
    (digits, exp + 1)
}

/// Place the decimal point for %f style: digits × 10^(dp-1) without an
/// exponent, no trailing ".0".
fn f_style(digits: String, dp: i32) -> String {
    let nd = digits.len() as i32;
    if dp <= 0 {
        // 0.0…0digits
        let mut out = String::from("0.");
        for _ in 0..-dp {
            out.push('0');
        }
        out.push_str(&digits);
        out
    } else if dp >= nd {
        let mut out = digits;
        for _ in 0..(dp - nd) {
            out.push('0');
        }
        out
    } else {
        let (int_part, frac) = digits.split_at(dp as usize);
        format!("{int_part}.{frac}")
    }
}

/// %f style from shortest digits (encoding/json's 'f' branch).
fn format_float_style(f: f64) -> String {
    let (digits, dp) = shortest_digits(f);
    let mut out = String::new();
    if f < 0.0 {
        out.push('-');
    }
    out.push_str(&f_style(digits, dp));
    out
}

/// %e style from shortest digits (ECMAScript/JSON exponent form):
/// d[.ddd]e±dd, then "1e-05" → "1e-5" (exactly one leading zero of a
/// negative exponent removed).
fn format_exp_style_json(f: f64) -> String {
    let (digits, dp) = shortest_digits(f);
    let exp = dp - 1;
    let mut out = String::new();
    if f < 0.0 {
        out.push('-');
    }
    out.push_str(&digits);
    if digits.len() > 1 {
        out.insert(1, '.');
    }
    out.push('e');
    if exp < 0 {
        out.push('-');
    } else {
        out.push('+');
    }
    let e = exp.abs();
    if e < 10 {
        out.push('0');
    }
    out.push_str(&e.to_string());
    // e-0X → e-X (one leading zero only), matching ECMAScript
    let n = out.len();
    let b = out.as_bytes();
    if n >= 4 && b[n - 4] == b'e' && b[n - 3] == b'-' && b[n - 2] == b'0' {
        out.remove(n - 2);
    }
    out
}

// --- string encoding -----------------------------------------------------------

/// writeJSONString: Go json.Marshal's escaping for strings — quotes,
/// backslash, control escapes, and the HTML escapes (< > & plus U+2028/9)
/// encoding/json applies by default.
pub(crate) fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub(crate) fn write_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

// --- path helpers -----------------------------------------------------------

/// PathString renders a document path in the editor's JSON-path style: "$"
/// for the root, object fields appended with ".", array indexes as "[N]" —
/// "$.server.port", "$.mounts[0].src", "$[0]".
/// A copy of `path` with one key segment appended.
pub fn append_key(path: &[PathSeg], key: &str) -> Vec<PathSeg> {
    let mut p = path.to_vec();
    p.push(PathSeg::key(key));
    p
}

/// A copy of `path` with one index segment appended.
pub fn append_idx(path: &[PathSeg], idx: usize) -> Vec<PathSeg> {
    let mut p = path.to_vec();
    p.push(PathSeg::Idx(idx));
    p
}

pub fn path_string(path: &[PathSeg]) -> String {
    let mut b = String::from("$");
    for seg in path {
        match seg {
            PathSeg::Key(k) => {
                b.push('.');
                b.push_str(k);
            }
            PathSeg::Idx(i) => {
                b.push_str(&format!("[{i}]"));
            }
        }
    }
    b
}

/// closesIndex returns the byte position of the "]" closing a "[digits]"
/// array index starting at i, or None when s[i..] does not begin a complete
/// index.
fn closes_index(s: &[u8], i: usize) -> Option<usize> {
    if i >= s.len() || s[i] != b'[' {
        return None;
    }
    let mut j = i + 1;
    while j < s.len() && s[j].is_ascii_digit() {
        j += 1;
    }
    if j == i + 1 || j >= s.len() || s[j] != b']' {
        return None;
    }
    Some(j)
}

/// ParsePath parses the PathString form back into a path. Reports false
/// unless the whole string is "$", "$.field", or "$[N]" followed by more
/// segments. Keys are read up to the next "." or "[N]" index, so a key
/// containing a dot or a complete index suffix does not round-trip (the
/// display does not quote keys).
pub fn parse_path(s: &str) -> Option<Vec<PathSeg>> {
    let bytes = s.as_bytes();
    if !s.starts_with('$') {
        return None;
    }
    let mut path = Vec::new();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b'.' && closes_index(bytes, i).is_none() {
                    i += 1;
                }
                if i == start {
                    return None; // empty key
                }
                path.push(PathSeg::Key(s[start..i].to_string()));
            }
            b'[' => {
                let end = closes_index(bytes, i)?;
                let n: usize = s[i + 1..end].parse().ok()?;
                path.push(PathSeg::Idx(n));
                i = end + 1;
            }
            _ => return None,
        }
    }
    Some(path)
}
