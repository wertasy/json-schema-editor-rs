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

impl Doc {
    pub fn new() -> Doc {
        let mut d = Doc { nodes: Vec::new(), root: 0 };
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
        self.push(NodeData { value: Value::Object(Vec::new()), ..Default::default() })
    }
    pub fn new_array(&mut self) -> NodeId {
        self.push(NodeData { value: Value::Array(Vec::new()), ..Default::default() })
    }
    pub fn new_str(&mut self, s: impl Into<String>) -> NodeId {
        self.push(NodeData { value: Value::Str(s.into()), ..Default::default() })
    }
    pub fn new_num(&mut self, lit: impl Into<String>) -> NodeId {
        self.push(NodeData { value: Value::Num(lit.into()), ..Default::default() })
    }
    pub fn new_bool(&mut self, b: bool) -> NodeId {
        self.push(NodeData { value: Value::Bool(b), ..Default::default() })
    }
    pub fn new_null(&mut self) -> NodeId {
        self.push(NodeData { value: Value::Null, ..Default::default() })
    }

    // --- accessors ----------------------------------------------------------

    pub fn node(&self, id: NodeId) -> &NodeData {
        &self.nodes[id as usize]
    }
    pub fn value(&self, id: NodeId) -> &Value {
        &self.nodes[id as usize].value
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
            PathSeg::Idx(_) => matches!(self.nodes[n as usize].value, Value::Array(_)) || self.empty(n),
            PathSeg::Key(_) => matches!(self.nodes[n as usize].value, Value::Object(_)) || self.empty(n),
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
                        return err(format!("jsondoc: Set array index {k} out of range (len {len})"));
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
            members.push(Member { key: key.to_string(), val: v });
        }
    }

    /// Set's member write at the last path segment. A current value carrying
    /// a YAML anchor is written through in place with its anchor kept: the
    /// node may be shared with alias occurrences, and swapping the id would
    /// silently detach them — every reference sees the write.
    fn overwrite_member(&mut self, n: NodeId, key: &str, v: NodeId) {
        let found = {
            let Value::Object(members) = &mut self.nodes[n as usize].value else { return };
            match members.iter().position(|m| m.key == key) {
                Some(idx) => Some((idx, members[idx].val)),
                None => {
                    members.push(Member { key: key.to_string(), val: v });
                    None
                }
            }
        };
        let Some((idx, old)) = found else { return };
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
            members.insert(idx, Member { key: key.to_string(), val: v });
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
            Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(|&it| self.to_value_walk(it, busy)).collect())
            }
            Value::Str(s) => serde_json::Value::String(s.clone()),
            Value::Num(lit) => num_to_value(lit),
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Null => serde_json::Value::Null,
        }
    }

    /// from_value builds a node tree from a serde_json value. Map keys are
    /// sorted (Go's FromAny sorts; serde_json's default map may not).
    pub fn from_value(&mut self, v: &serde_json::Value) -> NodeId {
        match v {
            serde_json::Value::Null => self.new_null(),
            serde_json::Value::Bool(b) => self.new_bool(*b),
            serde_json::Value::Number(n) => self.new_num(number_literal(n)),
            serde_json::Value::String(s) => self.new_str(s.clone()),
            serde_json::Value::Array(items) => {
                let arr = self.new_array();
                let ids: Vec<NodeId> = items.iter().map(|it| self.from_value(it)).collect();
                if let Value::Array(a) = &mut self.nodes[arr as usize].value {
                    *a = ids;
                }
                arr
            }
            serde_json::Value::Object(map) => {
                let obj = self.new_object();
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let members: Vec<Member> = keys
                    .into_iter()
                    .map(|k| Member { key: k.clone(), val: self.from_value(&map[k]) })
                    .collect();
                if let Value::Object(m) = &mut self.nodes[obj as usize].value {
                    *m = members;
                }
                obj
            }
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
                    .map(|m| Member { key: m.key, val: self.clone_detached(m.val) })
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
        self.push(NodeData { value, anchor: String::new(), merge: Vec::new() })
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

    /// ResolveJSONPointer resolves an RFC 6901 pointer (without the leading
    /// '#') against the document. It percent-decodes the fragment, applies
    /// ~0/~1 unescaping, and supports array indexes ("-" fails to parse and
    /// rejects; object keys by name).
    pub fn resolve_json_pointer(&self, ptr: &str) -> Option<NodeId> {
        let ptr = ptr.strip_prefix('#').unwrap_or(ptr);
        if ptr.is_empty() {
            return Some(self.root);
        }
        if let Ok(dec) = percent_decode(ptr) {
            return self.resolve_json_pointer_inner(&dec);
        }
        self.resolve_json_pointer_inner(ptr)
    }

    fn resolve_json_pointer_inner(&self, ptr: &str) -> Option<NodeId> {
        if ptr.is_empty() {
            return Some(self.root);
        }
        let mut cur = self.root;
        for raw in ptr.strip_prefix('/').unwrap_or(ptr).split('/') {
            let tok = raw.replace("~1", "/").replace("~0", "~");
            match &self.nodes[cur as usize].value {
                Value::Object(members) => {
                    cur = members.iter().find(|m| m.key == tok)?.val;
                }
                Value::Array(items) => {
                    let idx: usize = tok.parse().ok()?;
                    cur = *items.get(idx)?;
                }
                _ => return None,
            }
        }
        Some(cur)
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

/// Canonical literal for a serde_json number: integers exactly, floats via
/// Go encoding/json's formatting rules.
fn number_literal(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    match n.as_f64() {
        Some(f) if f.is_finite() => go_json_float(f),
        _ => "null".to_string(), // unreachable: serde_json numbers are finite
    }
}

/// Go encoding/json float encoding: 'f' (shortest) when 1e-6 <= |f| < 1e21
/// or f == 0, else 'e' (shortest) with a negative single-digit-exponent
/// cleanup ("1e-05" → "1e-5"). Non-finite floats cannot be encoded (Go
/// errors; callers degrade to null).
pub(crate) fn go_json_float(f: f64) -> String {
    if !f.is_finite() {
        return String::new();
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let abs = f.abs();
    if (1e-6..1e21).contains(&abs) {
        format_float_style(f)
    } else {
        format_exp_style_json(f)
    }
}

/// Go strconv.FormatFloat(f, 'g', -1, 64): shortest digits, %e when the
/// decimal exponent is < -4 or >= 6 (two exponent digits, sign always),
/// %f otherwise.
#[allow(dead_code)] // used by the YAML layer (Phase 2)
pub(crate) fn go_format_g(f: f64) -> String {
    if !f.is_finite() {
        return if f.is_nan() { "NaN".into() } else if f > 0.0 { "+Inf".into() } else { "-Inf".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let (digits, dp) = shortest_digits(f);
    let exp = dp - 1; // value = d.ddd × 10^exp
    let mut out = String::new();
    if f < 0.0 {
        out.push('-');
    }
    if exp < -4 || exp >= 6 {
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
    } else {
        out.push_str(&f_style(digits, dp));
    }
    out
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
    let digits = if trimmed.is_empty() { "0".to_string() } else { trimmed.to_string() };
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

/// %e style from shortest digits (encoding/json's 'e' branch): d[.ddd]e±dd,
/// then "1e-05" → "1e-5" (exactly one leading zero of a negative exponent
/// removed, matching encoding/json).
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
    // encoding/json cleanup: e-0X → e-X (one leading zero only)
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

fn write_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

// --- path helpers -----------------------------------------------------------

/// PathString renders a document path in the editor's JSON-path style: "$"
/// for the root, object fields appended with ".", array indexes as "[N]" —
/// "$.server.port", "$.mounts[0].src", "$[0]".
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
                while i < bytes.len()
                    && bytes[i] != b'.'
                    && closes_index(bytes, i).is_none()
                {
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

/// percent_decode mirrors url.PathUnescape: %XX decodes; a malformed escape
/// is an error.
fn percent_decode(s: &str) -> Result<String, ()> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b.get(i + 1..i + 3).ok_or(())?;
            let hi = (hex[0] as char).to_digit(16).ok_or(())?;
            let lo = (hex[1] as char).to_digit(16).ok_or(())?;
            out.push(((hi << 4) | lo) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

/// parse reads one strict JSON document, preserving key order and numeric
/// literals: comments and trailing commas are rejected (the JSONC parser is
/// the same scanner in lenient mode), and trailing data is an error.
pub fn parse(text: &str) -> Result<Doc, DocError> {
    super::jsoncdoc::parse_impl(text, false).map(|(doc, _)| doc)
}

#[cfg(test)]
pub(crate) fn doc(s: &str) -> Doc {
    parse(s).expect("test document must parse")
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::PathSeg::{Idx, Key};
    use serde_json::json;

    const ROUNDTRIP_SRC: &str = r#"{
  "name": "alpha",
  "port": 8080,
  "ratio": 1.0,
  "big": 1e3,
  "enabled": true,
  "nothing": null,
  "nested": {
    "a/b": "slash key",
    "m~n": "tilde key",
    "名字": "unicode"
  },
  "list": [
    "x",
    42,
    {
      "inner": true
    },
    []
  ],
  "empty_obj": {},
  "empty_arr": []
}"#;

    fn set_path(d: &mut Doc, path: Vec<PathSeg>, v: NodeId) {
        d.set(&path, v).expect("Set must succeed");
    }

    #[test]
    fn parse_write_roundtrip() {
        let n = doc(ROUNDTRIP_SRC);
        let out = n.write();
        assert_eq!(out, ROUNDTRIP_SRC.to_string() + "\n");
        // idempotent
        let n2 = doc(&out);
        assert_eq!(n2.write(), out);
        // numeric literals preserved exactly
        let got = n.get(&[Key("ratio".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Num("1.0".into()));
        let got = n.get(&[Key("big".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Num("1e3".into()));
        // key order preserved
        let Value::Object(members) = n.value(n.root) else { panic!() };
        let keys: Vec<&str> = members.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(
            keys.join(","),
            "name,port,ratio,big,enabled,nothing,nested,list,empty_obj,empty_arr"
        );
    }

    #[test]
    fn compact() {
        let n = doc(ROUNDTRIP_SRC);
        assert_eq!(
            n.compact(n.root),
            r#"{"name":"alpha","port":8080,"ratio":1.0,"big":1e3,"enabled":true,"nothing":null,"nested":{"a/b":"slash key","m~n":"tilde key","名字":"unicode"},"list":["x",42,{"inner":true},[]],"empty_obj":{},"empty_arr":[]}"#
        );
        let sub = n.get(&[Key("nested".into())]).unwrap();
        assert_eq!(
            n.compact(sub),
            r#"{"a/b":"slash key","m~n":"tilde key","名字":"unicode"}"#
        );
    }

    #[test]
    fn get_set_remove_append() {
        let mut n = doc(r#"{"a": {"b": 1}}"#);
        assert!(n.get(&[Key("a".into()), Key("b".into()), Key("c".into())]).is_none());
        assert!(n.get(&[Key("a".into()), Key("z".into())]).is_none());

        // Set auto-creates intermediates: key → object, index → array.
        let deep = n.new_str("deep");
        set_path(&mut n, vec![Key("x".into()), Key("y".into()), Idx(0), Key("z".into())], deep);
        let got = n.get(&[Key("x".into()), Key("y".into()), Idx(0), Key("z".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Str("deep".into()));

        // Existing key keeps position on replace.
        let seven = n.new_num("7");
        set_path(&mut n, vec![Key("a".into()), Key("b".into())], seven);
        let Value::Object(members) = n.value(n.root) else { panic!() };
        assert_eq!(members[0].key, "a");
        let got = n.get(&[Key("a".into()), Key("b".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Num("7".into()));

        // Append creates and grows arrays.
        let one = n.new_str("one");
        let two = n.new_str("two");
        n.append(&[Key("list".into())], one).unwrap();
        n.append(&[Key("list".into())], two).unwrap();
        let arr = n.get(&[Key("list".into())]).unwrap();
        assert_eq!(match n.value(arr) { Value::Array(a) => a.len(), _ => panic!() }, 2);

        // Remove middle array element and object key.
        assert!(n.remove(&[Key("list".into()), Idx(0)]));
        let arr = n.get(&[Key("list".into())]).unwrap();
        match n.value(arr) {
            Value::Array(a) => {
                assert_eq!(a.len(), 1);
                assert_eq!(n.value(a[0]), &Value::Str("two".into()));
            }
            _ => panic!(),
        }
        assert!(n.remove(&[Key("a".into()), Key("b".into())]));
        assert!(n.get(&[Key("a".into()), Key("b".into())]).is_none());
        assert!(!n.remove(&[Key("a".into()), Key("b".into())]));
    }

    fn summary_arr(d: &Doc, id: NodeId) -> String {
        let Value::Array(items) = d.value(id) else { panic!() };
        items
            .iter()
            .map(|&v| match d.value(v) {
                Value::Str(s) => s.clone(),
                Value::Num(l) => l.clone(),
                _ => "?".to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn member_key_list(d: &Doc, id: NodeId) -> String {
        let Value::Object(members) = d.value(id) else { panic!() };
        members.iter().map(|m| m.key.as_str()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn insert_and_insert_member() {
        let mut n = doc(r#"{"list": ["a", "b", "c"]}"#);
        // splice mid-array shifts the tail
        let x = n.new_str("x");
        n.insert(&[Key("list".into())], 1, x).unwrap();
        let arr = n.get(&[Key("list".into())]).unwrap();
        assert_eq!(summary_arr(&n, arr), "a x b c");
        // idx == len appends; out-of-range clamps
        let end = n.new_str("end");
        let clamped = n.new_str("clamped");
        n.insert(&[Key("list".into())], 4, end).unwrap();
        n.insert(&[Key("list".into())], 99, clamped).unwrap();
        let arr = n.get(&[Key("list".into())]).unwrap();
        assert_eq!(summary_arr(&n, arr), "a x b c end clamped");
        // auto-creates the array, like Append
        let one = n.new_num("1");
        n.insert(&[Key("fresh".into())], 0, one).unwrap();
        let arr = n.get(&[Key("fresh".into())]).unwrap();
        match n.value(arr) {
            Value::Array(a) => {
                assert_eq!(a.len(), 1);
                assert!(matches!(n.value(a[0]), Value::Num(_)));
            }
            _ => panic!(),
        }

        // member splice at idx; existing key keeps position, value replaced
        let root = n.new_str("root");
        n.insert_member(&[], "top", 0, root).unwrap();
        let got = n.get(&[Key("top".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Str("root".into()));
        let zz = n.new_num("1");
        let mm = n.new_num("2");
        n.insert_member(&[], "zz", 99, zz).unwrap();
        n.insert_member(&[], "mm", 0, mm).unwrap();
        // doc so far: top, list, fresh — zz appends, mm splices in at 0
        assert_eq!(member_key_list(&n, n.root), "mm top list fresh zz");
        let nine = n.new_num("9");
        n.insert_member(&[], "top", 0, nine).unwrap();
        let got = n.get(&[Key("top".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Num("9".into()));
        assert_eq!(member_key_list(&n, n.root), "mm top list fresh zz");
        let k = n.new_num("1");
        assert!(n.insert_member(&[Key("list".into())], "k", 0, k).is_err());

        // a target the document never created builds its parent chain, like Set
        let mut fresh = Doc::new();
        let bash = fresh.new_str("bash");
        fresh.insert_member(&[Key("defaults".into()), Key("run".into())], "shell", 0, bash).unwrap();
        let got = fresh.get(&[Key("defaults".into()), Key("run".into()), Key("shell".into())]).unwrap();
        assert_eq!(fresh.value(got), &Value::Str("bash".into()));
        // null ancestors and missing array elements resolve to containers too
        let one = fresh.new_num("1");
        fresh.insert_member(&[Key("items".into()), Idx(0)], "k", 0, one).unwrap();
        let got = fresh.get(&[Key("items".into()), Idx(0), Key("k".into())]).unwrap();
        assert_eq!(fresh.value(got), &Value::Num("1".into()));
        // a real scalar in the chain is still a structural conflict
        let mut conflict = Doc::new();
        let one = conflict.new_num("1");
        let a = conflict.new_object();
        if let Value::Object(m) = &mut conflict.nodes[a as usize].value {
            m.push(Member { key: "a".into(), val: one });
        }
        conflict.set(&[], a).unwrap();
        let k = conflict.new_num("1");
        assert!(conflict.insert_member(&[Key("a".into()), Key("b".into())], "k", 0, k).is_err());
    }

    #[test]
    fn move_test() {
        let mut n = doc(r#"{"list": ["a", "b", "c", "d"], "obj": {}}"#);
        let list = |n: &Doc| {
            let arr = n.get(&[Key("list".into())]).unwrap();
            summary_arr(n, arr)
        };
        assert!(n.move_(&[Key("list".into())], 1, 2));
        assert_eq!(list(&n), "a c b d");
        assert!(n.move_(&[Key("list".into())], 3, 0));
        assert_eq!(list(&n), "d a c b");
        assert!(n.move_(&[Key("list".into())], 0, 3));
        assert_eq!(list(&n), "a c b d");
        assert!(n.move_(&[Key("list".into())], 2, 2));
        assert_eq!(list(&n), "a c b d");
        // out of range: fail without changing the list (Go also tests -1,
        // which the usize API cannot express)
        assert!(!n.move_(&[Key("list".into())], 4, 0));
        assert!(!n.move_(&[Key("list".into())], 0, 4));
        assert_eq!(list(&n), "a c b d");
        // empty object and missing path: false, no panic
        assert!(!n.move_(&[Key("obj".into())], 0, 0));
        assert!(!n.move_(&[Key("nope".into())], 0, 0));

        // nested array paths
        let mut n2 = doc(r#"{"outer": {"list": ["x", "y", "z"]}}"#);
        assert!(n2.move_(&[Key("outer".into()), Key("list".into())], 0, 2));
        let inner = n2.get(&[Key("outer".into()), Key("list".into())]).unwrap();
        assert_eq!(summary_arr(&n2, inner), "y z x");
    }

    #[test]
    fn set_panics() {
        let mut n = doc(r#"{"s": "text"}"#);
        let one = n.new_num("1");
        assert!(n.set(&[Key("s".into()), Key("k".into())], one).is_err());
    }

    #[test]
    fn set_out_of_range_panics() {
        let mut n = doc(r#"{"a": [1]}"#);
        let x = n.new_str("x");
        assert!(n.set(&[Key("a".into()), Idx(5)], x).is_err());
    }

    #[test]
    fn set_replaces_null_with_container() {
        // JSON null carries no value: the first write through it replaces it
        // with the container the path demands.
        let mut n = doc(r#"{"map": {"Host": null}, "obj": null}"#);
        let x = n.new_str("x");
        n.set(&[Key("map".into()), Key("Host".into()), Idx(0)], x).unwrap();
        let got = n.get(&[Key("map".into()), Key("Host".into()), Idx(0)]).unwrap();
        assert_eq!(n.value(got), &Value::Str("x".into()));
        let arr = n.get(&[Key("map".into()), Key("Host".into())]).unwrap();
        match n.value(arr) {
            Value::Array(a) => assert_eq!(a.len(), 1),
            _ => panic!("Host should be a 1-element array"),
        }

        let seven = n.new_num("7");
        n.set(&[Key("obj".into()), Key("sub".into())], seven).unwrap();
        let obj = n.get(&[Key("obj".into())]).unwrap();
        assert!(matches!(n.value(obj), Value::Object(_)));
    }

    #[test]
    fn to_from_any() {
        let n = doc(r#"{"i": 3, "f": 1.5, "s": "x", "b": false, "n": null, "a": [1, "y"]}"#);
        let v = n.to_value(n.root);
        assert_eq!(v["i"], json!(3));
        assert_eq!(v["f"], json!(1.5));
        assert_eq!(v["s"], json!("x"));
        let mut d2 = Doc::new();
        let back = d2.from_value(&v);
        d2.set(&[], back).unwrap(); // Go's test calls Get on the returned node
        let got = d2.get(&[Key("i".into())]).unwrap();
        assert_eq!(d2.value(got), &Value::Num("3".into()));
        let r = d2.to_value(back);
        assert!(r.is_object());
    }

    #[test]
    fn clone_test() {
        let n = doc(r#"{"b":1.0,"a":[true,null,"x"]}"#);
        let want = n.write();
        let mut c = n.clone();
        assert_eq!(c.write(), want);
        // Mutating the clone must not touch the original.
        let y = c.new_str("y");
        c.set(&[Key("b".into())], y).unwrap();
        assert_eq!(n.write(), want);
        let got = n.get(&[Key("b".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Num("1.0".into()));
    }

    #[test]
    fn summary() {
        let cases = [
            (r#"{"a": "hello"}"#, "hello"),
            (r#"{"a": 8080}"#, "8080"),
            (r#"{"a": true}"#, "true"),
            (r#"{"a": null}"#, "null"),
            (r#"{"a": {"title": "T", "x": 1}}"#, "T"),
            (r#"{"a": {"x": 1}}"#, "{…}"),
            (r#"{"a": [1, 2, 3]}"#, "[3 items]"),
        ];
        for (src, want) in cases {
            let n = doc(src);
            let got = n.get(&[Key("a".into())]).unwrap();
            assert_eq!(n.summary(got), want, "Summary({src})");
        }
    }

    #[test]
    fn resolve_json_pointer() {
        let n = doc(
            r#"{
  "$defs": {"srv": {"type": "string"}},
  "a/b": 1,
  "m~n": 2,
  "arr": [{"k": "v"}]
}"#,
        );
        let cases = [
            ("", true, "object"),
            ("/$defs/srv/type", true, "string"),
            ("/a~1b", true, "1"),
            ("/m~0n", true, "2"),
            ("/arr/0/k", true, "v"),
            ("/arr/9", false, ""),
            ("/missing", false, ""),
            ("/m%7E0n", true, "2"), // %7E decodes to ~, then ~0 escape applies
            ("/arr/-", false, ""),
            ("/a%2Fb", false, ""), // %2F decodes before split → a separator, not a key char
        ];
        for (ptr, ok, want) in cases {
            let got = n.resolve_json_pointer(ptr);
            assert_eq!(got.is_some(), ok, "pointer {ptr:?} ok");
            if let (Some(g), true, w) = (got, ok, want) {
                match n.value(g) {
                    Value::Object(_) => assert_eq!(w, "object", "pointer {ptr:?}"),
                    Value::Str(s) => assert_eq!(s, w, "pointer {ptr:?}"),
                    Value::Num(l) => assert_eq!(l, w, "pointer {ptr:?}"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn path_string_and_parse() {
        let cases: Vec<(Vec<PathSeg>, &str)> = vec![
            (vec![], "$"),
            (vec![PathSeg::key("server")], "$.server"),
            (vec![PathSeg::key("server"), PathSeg::key("port")], "$.server.port"),
            (vec![PathSeg::key("mounts"), Idx(0), PathSeg::key("src")], "$.mounts[0].src"),
            (vec![Idx(0), PathSeg::key("x")], "$[0].x"),
            (vec![PathSeg::key("a/b"), PathSeg::key("m~n")], "$.a/b.m~n"),
            (vec![PathSeg::key("a[b]")], "$.a[b]"),
        ];
        for (path, s) in cases {
            assert_eq!(path_string(&path), s, "PathString({s:?})");
            let got = parse_path(s).unwrap_or_else(|| panic!("ParsePath({s}) failed"));
            assert_eq!(path_string(&got), s, "ParsePath({s}) roundtrip");
        }
        for s in ["", "server", "$.", "$..a", "$[0", "$[x]", "$[0]x", "$.a."] {
            assert!(parse_path(s).is_none(), "ParsePath({s:?}) should fail");
        }
    }

    #[test]
    fn parse_errors() {
        for src in [r#"{"a":}"#, "[1,", "{} {}"] {
            assert!(parse(src).is_err(), "Parse({src:?}) should fail");
        }
    }

    /// sharedDoc builds {"base":&x {"p":1},"copy":<same node>}: the shape
    /// yamldoc.Parse produces for "base: &x {p: 1}\ncopy: *x".
    fn shared_doc() -> Doc {
        let mut n = doc(r#"{"base":{"p":1}}"#);
        let base = n.get(&[Key("base".into())]).unwrap();
        n.node_mut(base).anchor = "x".to_string();
        n.set(&[Key("copy".into())], base).unwrap();
        n
    }

    #[test]
    fn clone_preserves_alias_sharing() {
        let n = shared_doc();
        let want = n.write();
        let mut c = n.clone();
        assert_eq!(c.write(), want);
        let cb = c.get(&[Key("base".into())]).unwrap();
        let cc = c.get(&[Key("copy".into())]).unwrap();
        assert_eq!(cb, cc, "Clone broke alias sharing");
        // Writing through the occurrence updates the anchor and vice versa;
        // the source tree stays untouched.
        let two = c.new_num("2");
        c.set(&[Key("copy".into()), Key("p".into())], two).unwrap();
        let got = c.get(&[Key("base".into()), Key("p".into())]).unwrap();
        assert_eq!(c.value(got), &Value::Num("2".into()), "shared write did not reach the anchor");
        assert_eq!(n.write(), want, "original mutated");
    }

    #[test]
    fn set_writes_through_anchored_node() {
        let mut n = shared_doc();
        let base = n.get(&[Key("base".into())]).unwrap();
        let copy_n = n.get(&[Key("copy".into())]).unwrap();
        assert_eq!(base, copy_n, "fixture is not shared");
        let replaced = n.new_str("replaced");
        n.set(&[Key("copy".into())], replaced).unwrap();
        assert_eq!(n.value(base), &Value::Str("replaced".into()));
        assert_eq!(n.node(base).anchor, "x", "anchor lost");
    }

    #[test]
    fn detach_breaks_alias_sharing() {
        let mut n = shared_doc();
        let cp = n.clone_detached(n.get(&[Key("copy".into())]).unwrap());
        assert!(n.detach(&[Key("copy".into())], cp));
        let base = n.get(&[Key("base".into())]).unwrap();
        assert_ne!(base, cp, "Detach did not break the reference");
        assert!(n.node(cp).anchor.is_empty(), "CloneDetached kept the anchor");
        let nine = n.new_num("9");
        n.set(&[Key("base".into()), Key("p".into())], nine).unwrap();
        let got = n.get(&[Key("copy".into()), Key("p".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Num("1".into()), "detached copy followed the anchor");
        assert!(!n.detach(&[Key("missing".into())], cp));
    }

    /// mergeDoc builds {"base":&b1 {env,out}, "b2":&b2 {env2, extra}, "child":
    /// {<<: [*b1,*b2], own}} with the merge spec pointing at the anchor nodes.
    fn merge_doc() -> Doc {
        let mut d = Doc::new();
        let base = d.new_object();
        d.node_mut(base).anchor = "b1".into();
        let e1 = d.new_str("e1");
        let o2 = d.new_str("o2");
        if let Value::Object(m) = &mut d.nodes[base as usize].value {
            m.push(Member { key: "env".into(), val: e1 });
            m.push(Member { key: "out".into(), val: o2 });
        }
        let b2 = d.new_object();
        d.node_mut(b2).anchor = "b2".into();
        let e2 = d.new_str("e2");
        let x = d.new_str("x");
        if let Value::Object(m) = &mut d.nodes[b2 as usize].value {
            m.push(Member { key: "env".into(), val: e2 });
            m.push(Member { key: "extra".into(), val: x });
        }
        let merge_val = d.push(NodeData {
            value: Value::Null,
            anchor: String::new(),
            merge: vec![base, b2],
        });
        let own = d.new_str("yes");
        let child = d.new_object();
        if let Value::Object(m) = &mut d.nodes[child as usize].value {
            m.push(Member { key: "<<".into(), val: merge_val });
            m.push(Member { key: "own".into(), val: own });
        }
        let root = d.new_object();
        for (k, v) in [("base", base), ("b2", b2), ("child", child)] {
            if let Value::Object(m) = &mut d.nodes[root as usize].value {
                m.push(Member { key: k.into(), val: v });
            }
        }
        d.set(&[], root).unwrap();
        d
    }

    #[test]
    fn merge_refs_and_resolution() {
        let n = merge_doc();
        let child = n.get(&[Key("child".into())]).unwrap();
        let refs = n.merge_refs(child);
        assert_eq!(refs.len(), 2);
        assert_eq!(n.node(refs[0]).anchor, "b1");
        assert_eq!(n.node(refs[1]).anchor, "b2");
        // ToAny applies YAML precedence: own wins, earlier anchor wins
        let got = n.to_value(n.root);
        let child_map = &got["child"];
        assert_eq!(child_map["own"], json!("yes"));
        assert_eq!(child_map["env"], json!("e1"));
        assert_eq!(child_map["out"], json!("o2"));
        assert_eq!(child_map["extra"], json!("x"));
        assert_eq!(child_map.as_object().unwrap().len(), 4);
    }

    #[test]
    fn merge_json_output_resolves() {
        let n = merge_doc();
        // pretty JSON and Compact both resolve merges inline
        assert_eq!(
            n.compact(n.root),
            r#"{"base":{"env":"e1","out":"o2"},"b2":{"env":"e2","extra":"x"},"child":{"own":"yes","env":"e1","out":"o2","extra":"x"}}"#
        );
    }

    #[test]
    fn merge_clone_keeps_spec_and_sharing() {
        let n = merge_doc();
        let mut cp = n.clone();
        let child = cp.get(&[Key("child".into())]).unwrap();
        let base = cp.get(&[Key("base".into())]).unwrap();
        let refs = cp.merge_refs(child);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0], base, "cloned merge must point at the cloned anchor");
        // a write through the cloned anchor is what the merged view resolves
        let changed = cp.new_str("changed");
        cp.set(&[Key("base".into()), Key("env".into())], changed).unwrap();
        let got = cp.to_value(cp.root)["child"]["env"].clone();
        assert_eq!(got, json!("changed"), "merged view did not follow the anchor");
    }

    #[test]
    fn merge_clone_detached_flattens() {
        let mut n = merge_doc();
        let child = n.get(&[Key("child".into())]).unwrap();
        let detached = n.clone_detached(child);
        assert!(n.merge_refs(detached).is_empty(), "detached copy must carry no merge refs");
        let keys: Vec<&str> = match n.value(detached) {
            Value::Object(m) => m.iter().map(|m| m.key.as_str()).collect(),
            _ => panic!(),
        };
        assert_eq!(keys.join(","), "own,env,out,extra");
        // edits to the detached copy never reach the anchor: replace env in
        // the detached copy (Set's plain replace — the old value is not
        // anchored)
        let mine = n.new_str("mine");
        n.overwrite_member(detached, "env", mine);
        let base = n.get(&[Key("base".into())]).unwrap();
        let base_env = match n.value(base) {
            Value::Object(m) => m.iter().find(|m| m.key == "env").unwrap().val,
            _ => panic!(),
        };
        assert_eq!(
            n.value(base_env),
            &Value::Str("e1".into()),
            "anchor changed through the detached copy"
        );
    }

    #[test]
    fn float_formats_match_go() {
        // encoding/json and strconv 'g' behavior pins
        assert_eq!(go_json_float(1.5), "1.5");
        assert_eq!(go_json_float(80.0), "80");
        assert_eq!(go_json_float(-0.5), "-0.5");
        assert_eq!(go_json_float(0.1), "0.1");
        assert_eq!(go_json_float(100000.0), "100000");
        assert_eq!(go_json_float(1e21), "1e+21");
        assert_eq!(go_json_float(1e-7), "1e-7");
        assert_eq!(go_json_float(1.5e-7), "1.5e-7");
        assert_eq!(go_format_g(100000.0), "100000");
        assert_eq!(go_format_g(1000000.0), "1e+06");
        assert_eq!(go_format_g(1e-5), "1e-05");
        assert_eq!(go_format_g(0.0001), "0.0001");
        assert_eq!(go_format_g(8080.0), "8080");
    }
}
