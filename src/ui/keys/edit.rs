use super::*;

impl App {
    // --- field editing -----------------------------------------------------

    /// adjustNumber implements - / = / + on a numeric field: bump the current
    /// number by delta, or write delta ("-1"/"1") when a schema-typed field
    /// has no value. Numbers the schema does not define (unknown keys,
    /// schema-less documents, raw fields) work by their runtime value.
    pub(super) fn adjust_number(&mut self, r: &Row, delta: i64) {
        // Field: a scalar row. Drill: an array item/schema-less value whose
        // schema node is a container or oneOf but whose runtime value is a
        // number (generic arrays). Unknown: a schema-less object member.
        if !matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            return;
        }
        let typed_numeric = r.fn_.is_some_and(|f| {
            let node = &self.fs().schema.nodes[f as usize];
            node.kind == Kind::Scalar && matches!(node.json_type.as_str(), "integer" | "number")
        });
        let path = r.path.clone();
        let literal = match self.doc().get(&path).map(|n| self.doc().value(n)) {
            Some(Value::Num(lit)) => match bump_number_literal(lit, delta) {
                Some(next) => next,
                None => return, // unrepresentable literal: leave it untouched
            },
            // Only a schema-typed numeric field can seed a missing value: for
            // a runtime-typed key there is nothing that says it is a number.
            None | Some(Value::Null) if typed_numeric => delta.to_string(),
            _ => return,
        };
        let n = self.fs_mut().doc.new_num(literal);
        let _ = self.fs_mut().doc.set(&path, n);
    }

    /// editValueExternalRow opens $EDITOR on the value under the cursor.
    pub(super) fn edit_value_external_row(&mut self, r: &Row) {
        if matches!(r.kind, RowKind::Field | RowKind::Unknown | RowKind::Drill) {
            self.edit_value_external(r.path.clone(), -1);
        } else {
            self.notice = "no value to edit".to_string();
        }
    }

    /// activateUnknown edits a schema-less key like a oneOf value: a known
    /// runtime type edits directly, while an absent/null value has no type
    /// to infer and opens the type picker.
    pub(super) fn activate_unknown(&mut self, path: Vec<PathSeg>) {
        self.open_runtime_value(path);
    }

    /// openRuntimeValue opens the editor for a value with no schema type,
    /// dispatching on its JSON runtime type. Null/missing values cannot
    /// determine a type, so they open the picker instead.
    pub(super) fn open_runtime_value(&mut self, path: Vec<PathSeg>) {
        let Some(n) = self.doc().get(&path) else {
            self.open_any_type(path);
            return;
        };
        match self.doc().value(n) {
            Value::Null => self.open_any_type(path),
            Value::Bool(_) => self.toggle_unknown_bool(&path),
            Value::Num(_) => self.open_unknown_scalar(path, "number"),
            Value::Str(_) => self.open_unknown_scalar(path, "string"),
            Value::Object(_) => self.open_generic_container(path, Kind::Object),
            Value::Array(_) => self.open_generic_container(path, Kind::Array),
        }
    }

    /// openGenericContainer pushes the schema-less Object/Array editor from
    /// the any-type picker: members/items are edited like unknown keys.
    pub(super) fn open_generic_container(&mut self, path: Vec<PathSeg>, kind: Kind) {
        let f = self.fs_mut();
        let mut cache = std::mem::take(&mut f.menu_cache);
        let any = any_type_menu(&mut f.schema, &mut cache);
        let variant = f.schema.nodes[any as usize]
            .variants
            .iter()
            .copied()
            .find(|v| {
                let n = &f.schema.nodes[*v as usize];
                n.key.is_empty() && n.kind == kind
            });
        f.menu_cache = cache;
        if let Some(v) = variant {
            self.push(v, path);
        }
    }

    /// openUnknownScalar opens the in-place input for a schema-less scalar.
    pub(super) fn open_unknown_scalar(&mut self, path: Vec<PathSeg>, json_type: &str) {
        let key = match path.last() {
            Some(PathSeg::Key(k)) => k.clone(),
            _ => String::new(),
        };
        let synth = FormNode {
            key: key.clone(),
            title: key,
            kind: Kind::Scalar,
            json_type: json_type.to_string(),
            ..FormNode::default()
        };
        self.open_field_synth(synth, path, false, None);
    }

    /// startMenuSearch arms the inline "/"-search of a oneOf picker or an
    /// enum-array toggle list. The query always starts empty and the cursor
    /// keeps the position it had (the current value's row when the picker
    /// opened on a value).
    pub(super) fn start_menu_search(&mut self) {
        let Some(st) = self.top().cloned() else {
            return;
        };
        if !matches!(st.kind, MenuKind::OneOf | MenuKind::EnumArray) {
            return;
        }
        if let Some(t) = self.top_mut() {
            t.filter.active = true;
            t.filter.ti = TextInput::default();
            t.filter.restore = st.cursor;
        }
    }

    /// menuSearchKey consumes keys while the inline list search is active.
    /// It supports the schema picker's editing keys; the query filters the
    /// rows as it is typed.
    pub(super) fn menu_search_key(&mut self, msg: &Msg) -> bool {
        let (inline_search, searching) = self
            .top()
            .map(|st| {
                (
                    matches!(st.kind, MenuKind::OneOf | MenuKind::EnumArray),
                    st.filter.active,
                )
            })
            .unwrap_or((false, false));
        if !inline_search || !searching {
            return false;
        }
        match msg {
            Msg::Esc => {
                if let Some(st) = self.top_mut() {
                    st.filter.active = false;
                    st.filter.ti = TextInput::default();
                }
                let n = self.menu_rows().len();
                if let Some(st) = self.top_mut() {
                    st.cursor = st.filter.restore.min(n.saturating_sub(1));
                }
                true
            }
            Msg::Enter => {
                let cur = self.top().map(|st| st.cursor).unwrap_or(0);
                let row = self.menu_rows().get(cur).cloned();
                let Some(r) = row else { return true };
                // no match: stay in the search mode so backspace undoes it
                if r.kind == RowKind::NoItems {
                    return true;
                }
                // toggling an enum-array member keeps the filter: enter is
                // the only toggle key while the query is being typed
                if self.top().map(|st| st.kind) == Some(MenuKind::EnumArray) {
                    self.activate(r);
                    return true;
                }
                if let Some(st) = self.top_mut() {
                    st.filter.active = false;
                    st.filter.ti = TextInput::default();
                }
                self.activate(r);
                true
            }
            Msg::Up | Msg::BackTab | Msg::Ctrl('k') => {
                self.menu_search_move(-1);
                true
            }
            Msg::Down | Msg::Tab | Msg::Ctrl('j') => {
                self.menu_search_move(1);
                true
            }
            _ => {
                let Some(key) = msg_key(msg) else {
                    return true; // search mode swallows unbound keys
                };
                let Some(st) = self.top_mut() else {
                    return true;
                };
                let before = st.filter.ti.value().to_string();
                st.filter.ti.handle_key(key);
                if st.filter.ti.value() != before {
                    st.cursor = 0;
                    st.offset = 0;
                }
                true
            }
        }
    }

    pub(super) fn menu_search_move(&mut self, delta: i64) {
        let n = self.menu_rows().len();
        let cur = self.top().map(|st| st.cursor).unwrap_or(0);
        if let Some(st) = self.top_mut() {
            st.cursor = wrap_cursor(cur, delta, n);
        }
    }

    /// openAnyType pushes the shared arbitrary-JSON type picker at path.
    pub(super) fn open_any_type(&mut self, path: Vec<PathSeg>) {
        let f = self.fs_mut();
        let mut cache = std::mem::take(&mut f.menu_cache);
        let node = any_type_menu(&mut f.schema, &mut cache);
        f.menu_cache = cache;
        self.push(node, path);
    }

    pub(super) fn toggle_unknown_bool(&mut self, path: &[PathSeg]) {
        let synth = FormNode {
            kind: Kind::Boolean,
            ..FormNode::default()
        };
        self.toggle_bool_synth(&synth, path);
    }

    pub(super) fn unknown_bool(&self, path: &[PathSeg]) -> bool {
        self.doc()
            .get(path)
            .map_or(false, |n| matches!(self.doc().value(n), Value::Bool(_)))
    }

    /// openField starts the appropriate editor for a field row.
    pub(super) fn open_field(&mut self, fn_: FormNodeId, path: Vec<PathSeg>, is_new: bool) {
        let f = self.fs().schema.nodes[fn_ as usize].clone();
        self.open_field_synth(f, path, is_new, Some(fn_))
    }

    /// openField with an inlined FormNode: schema node or the synthetic one
    /// of a schema-less scalar edit.
    pub(super) fn open_field_synth(
        &mut self,
        f: FormNode,
        path: Vec<PathSeg>,
        is_new: bool,
        fn_id: Option<FormNodeId>,
    ) {
        match f.kind {
            Kind::Scalar => {
                let cur = self.doc().get(&path);
                if let Some(n) = cur {
                    if let Value::Str(s) = self.doc().value(n) {
                        if s.contains('\r') || s.contains('\n') {
                            // multiline text cannot live in the one-line input
                            self.edit_value_external(path, -1);
                            return;
                        }
                    }
                }
                let mut ti = TextInput::new();
                let mut pre = cur
                    .and_then(|id| self.doc().scalar_text(id))
                    .unwrap_or_default()
                    .to_string();
                if pre.is_empty() {
                    if let Some(d) = f.default_ {
                        pre = self
                            .fs()
                            .schema
                            .doc
                            .scalar_text(d)
                            .unwrap_or_default()
                            .to_string();
                    }
                }
                ti.set_value(pre);
                ti.cursor_end();
                let cands = self.completions(&f, &path);
                self.overlay = Some(Overlay::Edit(EditState {
                    // a schema field keeps its real node id: the inline
                    // renderer matches rows by it (Go editState.fn); only a
                    // schema-less scalar edit carries a synthetic node
                    fn_: fn_id,
                    synth: if fn_id.is_none() { Some(f) } else { None },
                    path,
                    ti,
                    err: String::new(),
                    is_new,
                    cands,
                    sug_prefix: String::new(),
                    sugs: Vec::new(),
                    sug_idx: -1,
                }));
                self.refresh_edit_suggestions();
            }
            Kind::Enum => {
                let cur = self.doc().get(&path);
                let Some(fn_id) = fn_id else {
                    // synthetic nodes are never enums; fall back to raw edit
                    self.open_raw_at(None, Some(path));
                    return;
                };
                let cursor = enum_index(&self.fs().schema, fn_id, self.doc(), cur);
                self.overlay = Some(Overlay::Sel(SelState {
                    fn_: fn_id,
                    path,
                    opts: f.enum_,
                    cursor,
                    filter: Default::default(),
                    one_of: None,
                }));
            }
            Kind::Boolean => {
                // resolve against a schema id when possible (default lookup)
                if let Some(id) = fn_id {
                    self.toggle_bool(id, &path);
                } else {
                    self.toggle_bool_synth(&f, &path);
                }
            }
            // a field whose schema has no concrete JSON type edits by its
            // runtime type; only a null/missing value opens the type picker
            Kind::Raw if !f.null_only && !f.raw_editor => {
                self.open_runtime_value(path);
            }
            _ => {
                self.open_raw_at(fn_id, Some(path));
            }
        }
    }

    /// editRawCurrent opens the external $EDITOR with the raw JSON/YAML/TOML
    /// of the current node: the row under the cursor when it is a value row,
    /// otherwise (root scalar, action/status row) the whole document. E.
    /// The built-in raw textarea remains for raw-degraded fields and the
    /// any-type JSON variant.
    pub(super) fn edit_raw_current(&mut self, r: &Row) {
        if !r.path.is_empty()
            && matches!(
                r.kind,
                RowKind::Field | RowKind::Drill | RowKind::Unknown | RowKind::Variant
            )
        {
            self.edit_value_external(r.path.clone(), -1);
        } else {
            self.edit_raw_external();
        }
    }

    /// openRawAt arms the built-in raw textarea overlay.
    pub(super) fn open_raw_at(&mut self, fn_: Option<FormNodeId>, path: Option<Vec<PathSeg>>) {
        let mut ta = TextArea::new();
        let w = self.width.saturating_sub(4).clamp(20, 100);
        let h = self.height.saturating_sub(6).clamp(3, 24);
        ta.set_width(w as u16);
        ta.set_height(h as u16);
        let codec = self.fs().codec;
        let mut verbatim_str = false;
        match &path {
            Some(p) => {
                // A string value is text, not a document: encoding it as YAML
                // turned multiline strings into "|" blocks whose alignment
                // spaces landed in the editor and quoted values into quotes.
                if let Some(cur) = self.doc().get(p) {
                    if !is_null_val(self.doc(), cur) {
                        match self.doc().value(cur) {
                            Value::Str(text) => {
                                ta.set_value(text);
                                verbatim_str = true;
                            }
                            _ => {
                                if let Ok(s) = codec.write_value(self.doc(), cur) {
                                    ta.set_value(&s);
                                }
                            }
                        }
                    }
                }
            }
            None => {
                if let Ok(s) = codec.write(self.doc(), &self.fs().meta) {
                    ta.set_value(&s);
                }
            }
        }
        self.overlay = Some(Overlay::Text(TextState {
            path,
            fn_,
            ta,
            err: String::new(),
            verbatim_str,
        }));
    }

    /// toggleBool flips a boolean field between its current/effective value
    /// and the opposite, writing through.
    pub(super) fn toggle_bool(&mut self, fn_: FormNodeId, path: &[PathSeg]) {
        let f = self.fs().schema.nodes[fn_ as usize].clone();
        self.toggle_bool_synth(&f, path);
    }

    pub(super) fn toggle_bool_synth(&mut self, f: &FormNode, path: &[PathSeg]) {
        let mut b = false;
        if let Some(n) = self.doc().get(path) {
            if let Value::Bool(v) = self.doc().value(n) {
                b = *v;
            } else if let Some(d) = f.default_ {
                if let Value::Bool(v) = self.fs().schema.doc.value(d) {
                    b = *v;
                }
            }
        } else if let Some(d) = f.default_ {
            if let Value::Bool(v) = self.fs().schema.doc.value(d) {
                b = *v;
            }
        }
        let nb = !b;
        let n = self.fs_mut().doc.new_bool(nb);
        let _ = self.fs_mut().doc.set(path, n);
    }

    /// cycleEnum steps an enum field through its options.
    pub(super) fn cycle_enum(&mut self, r: &Row, delta: i64) {
        let Some(f) = r.fn_ else { return };
        let opts = self.fs().schema.nodes[f as usize].enum_.clone();
        if opts.is_empty() {
            return;
        }
        let cur = self.doc().get(&r.path);
        let idx = enum_index(&self.fs().schema, f, self.doc(), cur);
        let next = ((idx as i64 + delta + opts.len() as i64) % opts.len() as i64) as usize;
        let fi = self.cur;
        let f = &mut self.files[fi];
        let n = f
            .doc
            .copy_into(&f.schema.doc, opts[next].val, CopyMode::Preserve);
        let _ = f.doc.set(&r.path, n);
    }

    // --- arrays ------------------------------------------------------------

    /// addItem appends a new array item (< Add item > row).
    pub(super) fn add_item(&mut self) {
        let path = self.top().map(|st| st.path.clone()).unwrap_or_default();
        let idx = self.doc().array_len(&path);
        self.insert_item_at(idx, false);
    }

    /// insertItem inserts a fresh item before (after=false) or after the
    /// item under the cursor (Go insertItem).
    pub(super) fn insert_item(&mut self, r: &Row, after: bool) {
        let mut idx = 0usize;
        let path = self.top().map(|st| st.path.clone()).unwrap_or_default();
        if matches!(r.kind, RowKind::Field | RowKind::Drill) && r.idx >= 0 {
            idx = r.idx as usize;
            if after {
                idx += 1;
            }
        } else if after {
            idx = self.doc().array_len(&path);
        }
        self.insert_item_at(idx, true);
    }

    /// insertItemAt splices a fresh item at idx and opens its editor or
    /// drill (Go insertItemAt).
    pub(super) fn insert_item_at(&mut self, idx: usize, splice: bool) {
        let st = self.top().cloned();
        let Some(st) = st else { return };
        let items = match self.fs().schema.nodes[st.node as usize].items {
            Some(i) => i,
            None => return,
        };
        let ipath = append_idx(&st.path, idx);
        let items_kind = self.fs().schema.nodes[items as usize].kind;
        let ins_null = |m: &mut App, splice: bool| {
            if splice {
                let n = m.fs_mut().doc.new_null();
                let _ = m.fs_mut().doc.insert(&st.path, idx, n);
            }
        };
        match items_kind {
            Kind::OneOf => {
                // keep a null placeholder even for < Add item >: the type
                // picker edits this slot, and the inline editor needs a row
                let n = self.fs_mut().doc.new_null();
                if splice {
                    let _ = self.fs_mut().doc.insert(&st.path, idx, n);
                } else {
                    let _ = self.fs_mut().doc.set(&ipath, n);
                }
                if dynamic_variants(&self.fs().schema, items).is_some() {
                    self.nav_forward();
                    let st2 = self.dynamic_menu(items, ipath);
                    self.fs_mut().stack.push(st2);
                } else {
                    self.push(items, ipath);
                }
            }
            Kind::Object => {
                let n = self.fs_mut().doc.new_object();
                if splice {
                    let _ = self.fs_mut().doc.insert(&st.path, idx, n);
                }
                self.push(items, ipath);
            }
            Kind::Array => {
                let n = self.fs_mut().doc.new_array();
                if splice {
                    let _ = self.fs_mut().doc.insert(&st.path, idx, n);
                }
                self.push(items, ipath);
            }
            Kind::Boolean => {
                let mut b = false;
                if let Some(d) = self.fs().schema.nodes[items as usize].default_ {
                    if let Value::Bool(v) = self.fs().schema.doc.value(d) {
                        b = *v;
                    }
                }
                let n = self.fs_mut().doc.new_bool(b);
                if splice {
                    let _ = self.fs_mut().doc.insert(&st.path, idx, n);
                } else {
                    let _ = self.fs_mut().doc.set(&ipath, n);
                }
            }
            _ => {
                // scalar / enum / raw item
                ins_null(self, splice);
                self.open_field(items, ipath.clone(), true);
                if splice {
                    // the inserted item is the first row when the array was
                    // empty, otherwise row `idx` in the item-only list
                    if let Some(t) = self.top_mut() {
                        t.cursor = idx;
                    }
                }
            }
        }
    }

    /// deleteRow batch-deletes the current selection; without one it is the
    /// plain cursor-row delete.
    pub(super) fn delete_row(&mut self, r: &Row) {
        let has_sel = self.top().map(|st| !st.sel.is_empty()).unwrap_or(false);
        if !has_sel {
            self.delete_pair(r);
            return;
        }
        let rows = self.menu_rows();
        let mut keys: Vec<Vec<PathSeg>> = rows
            .iter()
            .filter(|sel| {
                self.top()
                    .map(|st| st.sel.contains(&sel_key(&sel.path)))
                    .unwrap_or(false)
            })
            .map(|sel| sel.path.clone())
            .collect();
        // array items must go last-to-first: each delete splices the list and
        // would shift the still-marked higher indexes otherwise
        keys.sort_by(|a, b| match (a.last(), b.last()) {
            (Some(PathSeg::Idx(x)), Some(PathSeg::Idx(y))) => y.cmp(x),
            _ => std::cmp::Ordering::Equal,
        });
        for p in keys {
            let tmp = Row {
                path: p,
                ..Row::default()
            };
            self.delete_pair(&tmp);
        }
        if let Some(st) = self.top_mut() {
            st.sel.clear();
        }
    }

    /// deletePair removes the selected key-value pair (Go deletePair).
    pub(super) fn delete_pair(&mut self, r: &Row) -> bool {
        if !matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            return false;
        }
        if r.path.is_empty() {
            return false;
        }
        if matches!(r.path.last(), Some(PathSeg::Idx(_))) {
            self.delete_item(r);
            return true;
        }
        // a merged row deletes the anchor mapping's field; required ones refuse
        let (required, merged, title) = r
            .fn_
            .map(|f| {
                let n = &self.fs().schema.nodes[f as usize];
                (n.required, r.merged.clone(), n.title.clone())
            })
            .unwrap_or((false, r.merged.clone(), String::new()));
        if required && !merged.is_empty() {
            self.notice = format!("{} is required", title);
            return false;
        }
        // deleting the carrier of an anchor a merge key references would
        // leave the merge dangling
        if let Some(n) = self.doc().get(&r.path) {
            let anchor = self.doc().node(n).anchor.clone();
            if !anchor.is_empty()
                && count_anchor_uses(self.doc(), n) <= 1
                && merge_ref_uses(self.doc(), n) > 0
            {
                self.notice = format!("delete the << merge key referencing &{} first", anchor);
                return false;
            }
        }
        self.fs_mut().doc.remove(&r.path);
        true
    }

    /// removeItemShortcut is D: remove the current array item, both from the
    /// items list (cursor on an item row) and from the item's own form (the
    /// menu path itself is an index). It confirms like the old removal row.
    pub(super) fn remove_item_shortcut(&mut self, r: &Row) {
        let Some(st) = self.top().cloned() else {
            return;
        };
        let (path, pop) = if matches!(st.path.last(), Some(PathSeg::Idx(_))) {
            // inside an array item's form: remove that item and go back
            (st.path.clone(), true)
        } else if st.kind == MenuKind::Items
            && matches!(r.kind, RowKind::Field | RowKind::Drill)
            && r.idx >= 0
        {
            (r.path.clone(), false)
        } else {
            self.notice = "D removes an array item".to_string();
            return;
        };
        self.overlay = Some(Overlay::Confirm(ConfirmState {
            title: "Remove this item? (y/n)".to_string(),
            hint: String::new(),
            on_yes: Some(Box::new(move |m| {
                let _ = m.fs_mut().doc.remove(&path);
                if pop {
                    m.fs_mut().stack.pop(); // back to the items menu
                }
            })),
            on_no: None,
        }));
    }

    /// deleteItem removes an array entry row.
    pub(super) fn delete_item(&mut self, r: &Row) {
        let ok = self
            .top()
            .map(|st| st.kind == MenuKind::Items)
            .unwrap_or(false);
        if !ok || r.idx < 0 || !matches!(r.kind, RowKind::Field | RowKind::Drill) {
            return;
        }
        self.fs_mut().doc.remove(&r.path);
    }

    /// moveItem shifts the array item under the cursor by delta.
    pub(super) fn move_item(&mut self, r: &Row, delta: i64) {
        let (kind, path) = match self.top() {
            Some(st) => (st.kind, st.path.clone()),
            None => return,
        };
        if kind != MenuKind::Items
            || r.idx < 0
            || !matches!(r.kind, RowKind::Field | RowKind::Drill)
        {
            return;
        }
        let from = r.idx as usize;
        let to = (r.idx + delta) as usize;
        if !self.fs_mut().doc.move_(&path, from, to) {
            self.notice = if delta < 0 {
                "already first item".to_string()
            } else {
                "already last item".to_string()
            };
            return;
        }
        if let Some(st) = self.top_mut() {
            st.cursor = (st.cursor as i64 + delta).max(0) as usize;
        }
    }

    /// addKey opens the key-name input for a schema-less member (Go addKey).
    pub(super) fn add_key(&mut self, r: &Row, after: bool) {
        let (kind, node_kind, path) = match self.top() {
            Some(st) => (
                st.kind,
                self.fs().schema.nodes[st.node as usize].kind,
                st.path.clone(),
            ),
            None => return,
        };
        if kind != MenuKind::Object || !matches!(node_kind, Kind::Object | Kind::Map) {
            self.notice = "a adds a custom key under an object menu".to_string();
            return;
        }
        let idx = self.member_insert_index(r, after);
        let ti = TextInput::new();
        self.overlay = Some(Overlay::Add(AddState {
            path,
            node: self.top().map(|st| st.node).unwrap(),
            idx,
            ti,
            err: String::new(),
        }));
    }

    /// memberInsertIndex returns the document member index a new key gets.
    pub(super) fn member_insert_index(&self, r: &Row, after: bool) -> usize {
        let Some(st) = self.top() else { return 0 };
        let mut mem_len = 0usize;
        if let Some(cur) = self.doc().get(&st.path) {
            if let Value::Object(mems) = self.doc().value(cur) {
                mem_len = mems.len();
            }
        }
        if matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown)
            && !r.path.is_empty()
        {
            if let Some(PathSeg::Key(key)) = r.path.last() {
                if let Some(cur) = self.doc().get(&st.path) {
                    if let Value::Object(mems) = self.doc().value(cur) {
                        for (i, m) in mems.iter().enumerate() {
                            if &m.key == key {
                                return if after { i + 1 } else { i };
                            }
                        }
                    }
                }
            }
        }
        mem_len
    }

    /// toggleEnumMember flips one option of an enum-valued array (Go
    /// toggleEnumMember).
    pub(super) fn toggle_enum_member(&mut self, r: &Row) {
        let Some(f) = r.fn_ else { return };
        let Some(items) = self.fs().schema.nodes[f as usize].items else {
            return;
        };
        let opts = self.fs().schema.nodes[items as usize].enum_.clone();
        if r.idx < 0 || (r.idx as usize) >= opts.len() {
            return;
        }
        let required = self.fs().schema.nodes[f as usize].required;
        let mut on: HashSet<String> = HashSet::new();
        if let Some(cur) = self.doc().get(&r.path) {
            if let Value::Array(arr) = self.doc().value(cur) {
                for &it in arr {
                    on.insert(self.doc().encode_value(it));
                }
            }
        }
        let enc = opts[r.idx as usize].encoded.clone();
        if on.contains(&enc) {
            on.remove(&enc);
        } else {
            on.insert(enc);
        }
        if on.is_empty() && !required {
            self.fs_mut().doc.remove(&r.path);
            return;
        }
        let fi = self.cur;
        let f = &mut self.files[fi];
        let mut items_v: Vec<NodeId> = Vec::new();
        for o in &opts {
            if on.contains(&o.encoded) {
                items_v.push(f.doc.copy_into(&f.schema.doc, o.val, CopyMode::Preserve));
            }
        }
        let arr = f.doc.push(NodeData {
            value: Value::Array(items_v),
            anchor: String::new(),
            merge: Vec::new(),
            raw: String::new(),
        });
        let _ = f.doc.set(&r.path, arr);
    }

    // --- oneOf -------------------------------------------------------------

    pub(super) fn enter_variant(&mut self, r: &Row) {
        let Some(st) = self.top().cloned() else {
            return;
        };
        // the any-type picker's JSON variant edits the existing value raw;
        // it is not a type switch, so it must not replace the value
        if let Some(v) = r.variant {
            if self.fs().schema.nodes[v as usize].raw_editor {
                self.enter_variant_editor(v, r.path.clone(), Some(st.node));
                return;
            }
        }
        let v = self.doc().get(&r.path);
        let cur = current_variant(&self.fs().schema, st.node, self.doc(), v);
        if cur >= 0 {
            let matched = self.fs().schema.nodes[st.node as usize].variants[cur as usize];
            if Some(matched) == r.variant {
                self.enter_variant_editor(matched, r.path.clone(), Some(st.node));
                return;
            }
        }
        if !is_null_opt(self.doc(), v) {
            let (variant, path, one_of) = (r.variant, r.path.clone(), st.node);
            // the any-type picker replaces a value's type in place: a null
            // placeholder keeps the object-member order / array slot
            let keep_slot = self.fs().menu_cache.get(&SynthKey::AnyType) == Some(&st.node);
            self.overlay = Some(Overlay::Confirm(ConfirmState {
                title: "Replace current value? (y/n)".to_string(),
                hint: String::new(),
                on_yes: Some(Box::new(move |m| {
                    let has_default = variant
                        .map(|vv| m.fs().schema.nodes[vv as usize].default_)
                        .flatten();
                    match has_default {
                        Some(d) if !is_null_val(&m.fs().schema.doc, d) => {
                            let fi = m.cur;
                            let f = &mut m.files[fi];
                            let n = f.doc.copy_into(&f.schema.doc, d, CopyMode::Preserve);
                            let _ = f.doc.set(&path, n);
                        }
                        _ if keep_slot || matches!(path.last(), Some(PathSeg::Idx(_))) => {
                            // keep the slot while the new type's editor
                            // fills it in
                            let n = m.fs_mut().doc.new_null();
                            let _ = m.fs_mut().doc.set(&path, n);
                        }
                        _ => {
                            m.fs_mut().doc.remove(&path);
                        }
                    }
                    m.enter_variant_editor(variant.expect("variant row"), path, Some(one_of));
                })),
                on_no: None,
            }));
            return;
        }
        let variant = r.variant.expect("variant row without node");
        self.enter_variant_editor(variant, r.path.clone(), Some(st.node));
    }

    pub(super) fn enter_variant_editor(
        &mut self,
        variant: FormNodeId,
        path: Vec<PathSeg>,
        one_of: Option<FormNodeId>,
    ) {
        let (kind, null_only) = {
            let n = &self.fs().schema.nodes[variant as usize];
            (n.kind, n.null_only)
        };
        // Every variant editor replaces the picker it was chosen from, so the
        // picker never lingers above the parent menu. A root oneOf picker has
        // no parent and stays.
        if self.top().map(|st| st.kind) == Some(MenuKind::OneOf) && self.fs().stack.len() > 1 {
            self.fs_mut().stack.pop();
        }
        match kind {
            // choosing the null type writes JSON null; there is nothing to
            // edit, so return to the parent menu
            Kind::Raw if null_only => {
                let n = self.fs_mut().doc.new_null();
                let _ = self.fs_mut().doc.set(&path, n);
            }
            Kind::OneOf => {
                // a schema-defined type may itself be a variant group
                if dynamic_variants(&self.fs().schema, variant).is_some() {
                    self.nav_forward();
                    let st = self.dynamic_menu(variant, path);
                    self.fs_mut().stack.push(st);
                } else {
                    self.push(variant, path);
                }
                if let Some(st) = self.top_mut() {
                    st.one_of = one_of;
                }
            }
            Kind::Object | Kind::Map | Kind::Array => {
                self.push(variant, path);
                if let Some(st) = self.top_mut() {
                    st.one_of = one_of;
                }
            }
            Kind::Boolean => self.toggle_bool(variant, &path),
            Kind::Enum => {
                let cur = self.doc().get(&path);
                let cursor = enum_index(&self.fs().schema, variant, self.doc(), cur);
                let opts = self.fs().schema.nodes[variant as usize].enum_.clone();
                self.overlay = Some(Overlay::Sel(SelState {
                    fn_: variant,
                    path,
                    opts,
                    cursor,
                    filter: Default::default(),
                    one_of,
                }));
            }
            _ => {
                // scalar / raw: edit at the field row in the parent menu
                self.open_field(variant, path.clone(), false);
            }
        }
    }

    // --- value completion ---------------------------------------------------

    /// completions collects the candidate values for fn at path (Go
    /// completions).
    pub(super) fn completions(&self, fn_node: &FormNode, path: &[PathSeg]) -> Vec<String> {
        if fn_node.kind == Kind::Scalar && !fn_node.tag_ref.is_empty() {
            return self.tag_ref_completions(&fn_node.tag_ref);
        }
        let want_num = match fn_node.json_type.as_str() {
            "string" | "" => false,
            "integer" | "number" => true,
            _ => return Vec::new(),
        };
        let doc = self.doc();
        let mut same: Vec<String> = Vec::new();
        let mut other: Vec<String> = Vec::new();
        let mut same_seen: HashSet<String> = HashSet::new();
        let mut other_seen: HashSet<String> = HashSet::new();
        fn walk(
            doc: &Doc,
            n: Option<NodeId>,
            p: &[PathSeg],
            key: &str,
            fn_key: &str,
            path: &[PathSeg],
            want_num: bool,
            same: &mut Vec<String>,
            same_seen: &mut HashSet<String>,
            other: &mut Vec<String>,
            other_seen: &mut HashSet<String>,
        ) {
            let Some(n) = n else { return };
            match doc.value(n) {
                Value::Object(mems) => {
                    for m in mems {
                        if p.is_empty() && m.key == "$schema" {
                            continue; // the editor's own schema pointer
                        }
                        let mp = append_key(p, &m.key);
                        if let Some(s) = value_text(doc, Some(m.val), want_num) {
                            if mp != path {
                                if m.key == fn_key {
                                    if completion_candidate(&s) && same_seen.insert(s.clone()) {
                                        same.push(s);
                                    }
                                } else if completion_candidate(&s) && other_seen.insert(s.clone()) {
                                    other.push(s);
                                }
                            }
                            continue;
                        }
                        walk(
                            doc,
                            Some(m.val),
                            &mp,
                            &m.key,
                            fn_key,
                            path,
                            want_num,
                            same,
                            same_seen,
                            other,
                            other_seen,
                        );
                    }
                }
                Value::Array(items) => {
                    for (i, &it) in items.iter().enumerate() {
                        let ip = append_idx(p, i);
                        if let Some(s) = value_text(doc, Some(it), want_num) {
                            if ip != path {
                                if key == fn_key {
                                    if completion_candidate(&s) && same_seen.insert(s.clone()) {
                                        same.push(s);
                                    }
                                } else if completion_candidate(&s) && other_seen.insert(s.clone()) {
                                    other.push(s);
                                }
                            }
                            continue;
                        }
                        walk(
                            doc,
                            Some(it),
                            &ip,
                            key,
                            fn_key,
                            path,
                            want_num,
                            same,
                            same_seen,
                            other,
                            other_seen,
                        );
                    }
                }
                v => {
                    if let Some(s) = value_text(doc, Some(n), want_num) {
                        if p != path {
                            let _ = v;
                            if key == fn_key {
                                if completion_candidate(&s) && same_seen.insert(s.clone()) {
                                    same.push(s);
                                }
                            } else if completion_candidate(&s) && other_seen.insert(s.clone()) {
                                other.push(s);
                            }
                        }
                    }
                }
            }
        }
        let fn_key = fn_node.key.clone();
        walk(
            doc,
            Some(doc.root),
            &[],
            "",
            &fn_key,
            path,
            want_num,
            &mut same,
            &mut same_seen,
            &mut other,
            &mut other_seen,
        );
        let mut out = same.clone();
        for v in other {
            if !same_seen.contains(&v) {
                out.push(v);
            }
        }
        out
    }

    /// tagRefCompletions collects the "tag" values of the structures a
    /// reference domain names (Go tagRefCompletions).
    pub(super) fn tag_ref_completions(&self, ref_: &str) -> Vec<String> {
        let words: Vec<&str> = ref_.split('_').collect();
        let (leads, tail) = words.split_at(words.len() - 1);
        let leads: Vec<&str> = leads.to_vec();
        let tail = tail[0];
        let doc = self.doc();
        let mut out: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        fn collect(doc: &Doc, n: NodeId, out: &mut Vec<String>, seen: &mut HashSet<String>) {
            if let Value::Array(items) = doc.value(n) {
                for &it in items {
                    collect(doc, it, out, seen);
                }
                return;
            }
            let tag = tag_of_node(doc, n);
            if !tag.is_empty() && seen.insert(tag.clone()) {
                out.push(tag);
            }
        }
        fn walk(
            doc: &Doc,
            n: NodeId,
            ancestors: &str,
            ref_: &str,
            leads: &[&str],
            tail: &str,
            out: &mut Vec<String>,
            seen: &mut HashSet<String>,
        ) {
            match doc.value(n) {
                Value::Object(mems) => {
                    for m in mems {
                        if m.key == ref_
                            || m.key == format!("{}s", ref_)
                            || (!leads.is_empty()
                                && (m.key == tail || m.key == format!("{}s", tail))
                                && ref_words_above(leads, ancestors))
                        {
                            collect(doc, m.val, out, seen);
                        }
                        let child_anc = format!("{ancestors}\0{}\0", m.key);
                        walk(doc, m.val, &child_anc, ref_, leads, tail, out, seen);
                    }
                }
                Value::Array(items) => {
                    for &it in items {
                        walk(doc, it, ancestors, ref_, leads, tail, out, seen);
                    }
                }
                _ => {}
            }
        }
        walk(doc, doc.root, "\0", ref_, &leads, tail, &mut out, &mut seen);
        out
    }

    /// refreshEditSuggestions re-filters the candidates by the current
    /// input (Go refreshEditSuggestions).
    pub(super) fn refresh_edit_suggestions(&mut self) {
        // schema suggestions win over document candidates (Go reads the
        // field's own node; synthetic nodes only for schema-less edits)
        let src = match &self.overlay {
            Some(Overlay::Edit(e)) => match e.fn_ {
                Some(id) => Some(self.fs().schema.nodes[id as usize].clone()),
                None => e.synth.clone(),
            },
            _ => None,
        };
        let Some(Overlay::Edit(e)) = &mut self.overlay else {
            return;
        };
        let prefix = e.ti.value().to_string();
        e.sug_prefix = prefix.clone();
        if let Some(src) = src {
            if src.tag_ref.is_empty() && !src.suggestions.is_empty() {
                let opts = filter_completions(&src.suggestions, &prefix);
                if !opts.is_empty() {
                    e.sugs = opts;
                    e.sug_idx = -1;
                    return;
                }
            }
        }
        e.sugs = filter_completions(&e.cands, &prefix);
        e.sug_idx = -1;
    }
}
