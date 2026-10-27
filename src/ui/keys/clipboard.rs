use super::*;

impl App {
    // --- copy / cut --------------------------------------------------------

    /// copyRow copies the row under the cursor (Go copyRow).
    pub(super) fn copy_row(&mut self, r: &Row, pair: bool) {
        let (text, ok, refusal) = self.row_copy_text(r, pair, "y copies object keys only");
        if !ok {
            self.notice = refusal;
            return;
        }
        if let Err(e) = self.write_clipboard_text(&text) {
            self.notice = format!("copy failed: {e}");
            return;
        }
        self.notice = if pair {
            "Copied key-value pair".to_string()
        } else {
            "Copied value".to_string()
        };
    }

    pub(super) fn write_clipboard_text(&self, text: &str) -> Result<(), String> {
        match &self.opts().copy {
            Some(w) => w(text),
            None => write_clipboard(text),
        }
    }

    pub(crate) fn read_clipboard_text(&self) -> Result<String, String> {
        match &self.opts().paste {
            Some(r) => r(),
            None => read_clipboard(),
        }
    }

    /// pasteIntoFocusedInput inserts paste text into the text input that owns
    /// the keyboard (ctrl+v and terminal bracketed paste). Reports whether an
    /// input consumed it; list-mode keys are left alone.
    pub(crate) fn paste_into_focused_input(&mut self, text: &str) -> bool {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        // The file switcher and the oneOf/enum-array search live outside
        // `overlay`; their inputs follow the same reset-on-change rule as
        // typed characters.
        if let Some(Overlay::FilePick(p)) = &mut self.overlay {
            if p.filter.active {
                let before = p.filter.ti.value().to_string();
                p.filter.ti.insert_str(&text);
                if p.filter.ti.value() != before {
                    p.cursor = 0;
                }
                return true;
            }
        }
        if let Some(st) = self.top_mut() {
            if st.filter.active && matches!(st.kind, MenuKind::OneOf | MenuKind::EnumArray) {
                let before = st.filter.ti.value().to_string();
                st.filter.ti.insert_str(&text);
                if st.filter.ti.value() != before {
                    st.cursor = 0;
                    st.offset = 0;
                }
                return true;
            }
        }
        let Some(ov) = self.overlay.take() else {
            return false;
        };
        match ov {
            Overlay::Edit(mut e) => {
                e.ti.insert_str(&text);
                self.overlay = Some(Overlay::Edit(e));
                self.refresh_edit_suggestions();
            }
            Overlay::Text(mut s) => {
                s.ta.insert_paste(&text);
                self.overlay = Some(Overlay::Text(s));
            }
            Overlay::CommentEdit(mut s) => {
                s.ta.insert_paste(&text);
                self.overlay = Some(Overlay::CommentEdit(s));
            }
            Overlay::Add(mut s) => {
                s.ti.insert_str(&text);
                self.overlay = Some(Overlay::Add(s));
            }
            Overlay::Write(mut s) => {
                s.ti.insert_str(&text);
                self.overlay = Some(Overlay::Write(s));
                self.refresh_write_suggestions();
            }
            Overlay::Command(mut s) => {
                // the prompt is one line: pasted newlines become spaces
                s.ti.insert_str(&text.replace('\n', " "));
                let path = self.opts().path.clone();
                cmd_sug_refresh(&mut s, &path);
                self.overlay = Some(Overlay::Command(s));
            }
            Overlay::Anc(mut s) => {
                s.ti.insert_str(&text);
                s.sug = -1;
                s.sug_prefix = s.ti.value().to_string();
                s.sugs = Self::anchor_filtered(&s);
                s.sug_line.clear();
                self.overlay = Some(Overlay::Anc(s));
            }
            Overlay::Mrg(mut s) => {
                s.ti.insert_str(&text);
                s.sug = -1;
                self.overlay = Some(Overlay::Mrg(s));
            }
            Overlay::Search(mut s) => {
                if s.replace && s.replace_focused {
                    s.rti.insert_str(&text);
                } else {
                    s.ti.insert_str(&text);
                }
                self.refilter(&mut s);
                self.overlay = Some(Overlay::Search(s));
            }
            Overlay::Sel(mut s) if s.filter.active => {
                let before = s.filter.ti.value().to_string();
                s.filter.ti.insert_str(&text);
                if s.filter.ti.value() != before {
                    s.cursor = 0;
                }
                self.overlay = Some(Overlay::Sel(s));
            }
            Overlay::Issues(mut s) if s.filter.active => {
                let before = s.filter.ti.value().to_string();
                s.filter.ti.insert_str(&text);
                if s.filter.ti.value() != before {
                    s.cursor = 0;
                }
                self.overlay = Some(Overlay::Issues(s));
            }
            Overlay::Errs(mut s) if s.filter.active => {
                let before = s.filter.ti.value().to_string();
                s.filter.ti.insert_str(&text);
                if s.filter.ti.value() != before {
                    s.cursor = 0;
                }
                self.overlay = Some(Overlay::Errs(s));
            }
            Overlay::RefPick(mut s) if s.filter.active => {
                let before = s.filter.ti.value().to_string();
                s.filter.ti.insert_str(&text);
                if s.filter.ti.value() != before {
                    s.cursor = 0;
                }
                self.overlay = Some(Overlay::RefPick(s));
            }
            other => {
                self.overlay = Some(other);
                return false;
            }
        }
        true
    }

    /// rowCopyText builds the y/X clipboard text for the row (Go
    /// rowCopyText).
    pub(super) fn row_copy_text(
        &self,
        r: &Row,
        pair: bool,
        key_refusal: &str,
    ) -> (String, bool, String) {
        if !matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            return (String::new(), false, "nothing to copy".to_string());
        }
        let Some(v) = self.doc().get(&r.path) else {
            return (String::new(), false, "no value to copy".to_string());
        };
        if pair {
            match copy_row_key(&r.path) {
                Some(key) => (self.copy_pair_text(&key, v), true, String::new()),
                None => (String::new(), false, key_refusal.to_string()),
            }
        } else {
            (self.copy_value_text(v), true, String::new())
        }
    }

    /// copyValueText is the Y text (Go copyValueText).
    pub(super) fn copy_value_text(&self, v: NodeId) -> String {
        if let Value::Str(s) = self.doc().value(v) {
            return s.clone();
        }
        let codec = self.fs().codec;
        if let Ok(s) = codec.write_value(self.doc(), v) {
            return s.trim_end_matches('\n').to_string();
        }
        self.doc().compact(v)
    }

    /// copyPairText is the y text (Go copyPairText).
    pub(super) fn copy_pair_text(&self, key: &str, v: NodeId) -> String {
        let codec = self.fs().codec;
        // a synthetic one-member object goes through the session codec
        let mut tmp = Doc::new();
        let val = tmp.copy_into(self.doc(), v, CopyMode::Preserve);
        let obj = tmp.new_object();
        if let Value::Object(m) = &mut tmp.node_mut(obj).value {
            m.push(Member {
                key: key.to_string(),
                val,
            });
        }
        if let Ok(s) = codec.write_value(&tmp, obj) {
            return s.trim_end_matches('\n').to_string();
        }
        let raw_key = serde_json::to_string(key).unwrap_or_default();
        format!("{}: {}", raw_key, self.doc().compact(v))
    }

    /// selectionMembers collects the marked rows' key-value pairs in display
    /// order.
    pub(super) fn selection_members(&mut self) -> Vec<(String, NodeId)> {
        let Some(st) = self.top().cloned() else {
            return Vec::new();
        };
        let rows = self.menu_rows_snapshot();
        let mut members = Vec::new();
        for sel in rows {
            if sel.path.is_empty() || !st.sel.contains(&sel_key(&sel.path)) {
                continue;
            }
            let Some(PathSeg::Key(key)) = sel.path.last() else {
                continue;
            };
            if let Some(v) = self.doc().get(&sel.path) {
                members.push((key.clone(), v));
            }
        }
        members
    }

    /// selectionValues collects the marked rows' document values in display
    /// order — the array-menu counterpart of selectionMembers (array items
    /// have no object key to copy).
    pub(super) fn selection_values(&mut self) -> Vec<NodeId> {
        let Some(st) = self.top().cloned() else {
            return Vec::new();
        };
        let rows = self.menu_rows_snapshot();
        let doc = self.doc();
        rows.iter()
            .filter(|r| !r.path.is_empty() && st.sel.contains(&sel_key(&r.path)))
            .filter_map(|r| doc.get(&r.path))
            .collect()
    }

    /// menuRows for the current (top) state (read-only flows reuse the same
    /// warm cache path as menu_rows).
    pub(super) fn menu_rows_snapshot(&mut self) -> Vec<Row> {
        self.menu_rows()
    }

    /// copyRowOrSelection is the y handler (Go copyRowOrSelection).
    pub(super) fn copy_row_or_selection(&mut self, r: &Row) {
        let has_sel = self.top().map(|st| !st.sel.is_empty()).unwrap_or(false);
        // array pages copy element values, not key-value pairs
        let items = self.top().map(|st| st.kind) == Some(MenuKind::Items);
        if !has_sel {
            self.copy_row(r, !items);
            return;
        }
        if items {
            let values = self.selection_values();
            if values.is_empty() {
                self.notice = "no value to copy".to_string();
                return;
            }
            let text = values
                .iter()
                .map(|v| self.copy_value_text(*v))
                .collect::<Vec<_>>()
                .join("\n");
            if let Err(e) = self.write_clipboard_text(&text) {
                self.notice = format!("copy failed: {e}");
                return;
            }
            self.notice = format!("Copied {} values", values.len());
            if let Some(st) = self.top_mut() {
                st.sel.clear();
            }
            return;
        }
        let members = self.selection_members();
        if members.is_empty() {
            self.notice = "no value to copy".to_string();
            return;
        }
        let text = self.copy_value_text_of_members(&members);
        if let Err(e) = self.write_clipboard_text(&text) {
            self.notice = format!("copy failed: {e}");
            return;
        }
        self.notice = format!("Copied {} key-value pairs", members.len());
        if let Some(st) = self.top_mut() {
            st.sel.clear();
        }
    }

    pub(super) fn copy_value_text_of_members(&self, members: &[(String, NodeId)]) -> String {
        let mut tmp = Doc::new();
        let obj = tmp.new_object();
        let mut vals: Vec<(String, NodeId)> = Vec::new();
        for (k, v) in members {
            let val = tmp.copy_into(self.doc(), *v, CopyMode::Preserve);
            vals.push((k.clone(), val));
        }
        if let Value::Object(m) = &mut tmp.node_mut(obj).value {
            for (k, val) in vals {
                m.push(Member { key: k, val });
            }
        }
        let codec = self.fs().codec;
        if let Ok(s) = codec.write_value(&tmp, obj) {
            return s.trim_end_matches('\n').to_string();
        }
        String::new()
    }

    /// cutRowOrSelection is the x/X handler (Go cutRowOrSelection).
    pub(super) fn cut_row_or_selection(&mut self, r: &Row, pair: bool) {
        let batch = self.top().map(|st| !st.sel.is_empty()).unwrap_or(false);
        let items = self.top().map(|st| st.kind) == Some(MenuKind::Items);
        let (text, count) = if batch {
            if items {
                let vals = self.selection_values();
                if vals.is_empty() {
                    self.notice = "no value to copy".to_string();
                    return;
                }
                let values: Vec<String> = vals.iter().map(|v| self.copy_value_text(*v)).collect();
                (values.join("\n"), vals.len())
            } else if pair {
                let members = self.selection_members();
                if members.is_empty() {
                    self.notice = "no value to copy".to_string();
                    return;
                }
                let t = self.copy_value_text_of_members(&members);
                (t, members.len())
            } else {
                let members = self.selection_members();
                let mut values: Vec<String> = Vec::new();
                for (_, v) in &members {
                    values.push(self.copy_value_text(*v));
                }
                (values.join("\n"), members.len())
            }
        } else {
            let (t, ok, refusal) = self.row_copy_text(r, pair, "x cuts object keys only");
            if !ok {
                self.notice = refusal;
                return;
            }
            (t, 1)
        };
        if let Err(e) = self.write_clipboard_text(&text) {
            self.notice = format!("cut failed: {e}");
            return;
        }
        self.delete_row(r); // clears the selection; refusals surface their notice
        if self.notice.is_empty() {
            if !batch && pair {
                self.notice = "Cut key-value pair".to_string();
            } else if !batch {
                self.notice = "Cut value".to_string();
            } else if pair {
                self.notice = format!("Cut {} key-value pairs", count);
            } else {
                self.notice = format!("Cut {} values", count);
            }
        }
    }

    // --- paste -------------------------------------------------------------

    /// pastePlainStringDoc accepts unstructured clipboard text as a string
    /// when P targets a string value (or a selection containing one). Other
    /// targets keep the parse error: this is not a general plain-text paste.
    pub(super) fn paste_plain_string_doc(
        &mut self,
        text: &str,
        r: &Row,
        under_cursor: bool,
    ) -> Option<Doc> {
        if !under_cursor || !self.paste_has_plain_string_target(r) {
            return None;
        }
        plain_paste_doc(text)
    }

    /// pasteHasPlainStringTarget reports whether the cursor row (or one of
    /// the marked rows) is a string value slot.
    pub(super) fn paste_has_plain_string_target(&mut self, r: &Row) -> bool {
        if self.top().map(|st| !st.sel.is_empty()).unwrap_or(false) {
            let sel = self.top().map(|st| st.sel.clone()).unwrap_or_default();
            let rows = self.menu_rows();
            rows.iter()
                .any(|row| sel.contains(&sel_key(&row.path)) && self.paste_plain_string_target(row))
        } else {
            self.paste_plain_string_target(r)
        }
    }

    /// pastePlainStringTarget reports whether a row's value can take plain
    /// clipboard text as a string: the current runtime value is a string, or
    /// the row's schema admits an arbitrary string (text scalar, enum, raw
    /// any-JSON, oneOf with such a leaf). Schema-less keys have no type, so
    /// only a string (or missing) value counts.
    pub(super) fn paste_plain_string_target(&self, r: &Row) -> bool {
        if !paste_value_row_ok(r) {
            return false;
        }
        if let Some(n) = self.doc().get(&r.path) {
            if matches!(self.doc().value(n), Value::Str(_)) {
                return true;
            }
        }
        match r.fn_ {
            Some(f) => schema_accepts_string(&self.fs().schema, f, &mut HashSet::new()),
            // A oneOf picker row has no node of its own; the group is the
            // menu's schema node.
            None if r.kind == RowKind::Variant => self
                .top()
                .filter(|st| st.kind == MenuKind::OneOf)
                .is_some_and(|st| {
                    schema_accepts_string(&self.fs().schema, st.node, &mut HashSet::new())
                }),
            // A schema-less row has no declared type: only a string (handled
            // above) or a missing value is a safe plain-text target.
            None => matches!(
                self.doc().get(&r.path).map(|n| self.doc().value(n)),
                None | Some(Value::Null)
            ),
        }
    }

    /// pasteClipboard implements p/P smart paste (Go pasteClipboard).
    pub(super) fn paste_clipboard(&mut self, r: &Row, under_cursor: bool) {
        let text = match self.read_clipboard_text() {
            Ok(t) => t,
            Err(e) => {
                self.notice = format!("paste failed: {e}");
                return;
            }
        };
        let mode = self.fs().codec.label();
        let (members, items, doc) = match parse_paste(&text, mode) {
            Ok(x) => x,
            Err(e) => match self.paste_plain_string_doc(&text, r, under_cursor) {
                Some(doc) => (None, None, Some(doc)),
                None => {
                    self.notice = e;
                    return;
                }
            },
        };
        // a single value replaces values
        if members.is_none() && items.is_none() {
            let Some(pdoc) = doc.as_ref() else {
                self.notice = "paste failed: empty clipboard".to_string();
                return;
            };
            let single = pdoc.root;
            if under_cursor {
                let has_sel = self.top().map(|st| !st.sel.is_empty()).unwrap_or(false);
                let mut targets: Vec<Row> = Vec::new();
                if has_sel {
                    let rows = self.menu_rows();
                    for sel in rows {
                        if self
                            .top()
                            .map(|st| st.sel.contains(&sel_key(&sel.path)))
                            .unwrap_or(false)
                            && paste_value_row_ok(&sel)
                        {
                            targets.push(sel);
                        }
                    }
                } else if paste_value_row_ok(r) {
                    targets.push(r.clone());
                }
                if !targets.is_empty() {
                    let pre = self.doc().clone();
                    for t in &targets {
                        let n = self
                            .fs_mut()
                            .doc
                            .copy_into(pdoc, single, CopyMode::Detached);
                        let _ = self.fs_mut().doc.set(&t.path, n);
                    }
                    if has_sel {
                        if let Some(st) = self.top_mut() {
                            st.sel.clear();
                        }
                    }
                    if nodes_equal_doc(&pre, &self.fs().doc) {
                        self.notice = "paste made no changes".to_string();
                    } else if targets.len() == 1 {
                        self.notice = format!(
                            "pasted value into {}",
                            crate::doc::jsondoc::path_string(&targets[0].path)
                        );
                    } else {
                        self.notice = format!("pasted value into {} keys", targets.len());
                    }
                    return;
                }
            }
            self.notice = "a single value replaces a row's value — press P on the row".to_string();
            return;
        }
        let (path, is_array, notice) = self.paste_target(r, under_cursor);
        if !notice.is_empty() {
            self.notice = notice;
            return;
        }
        let st_kind = self.top().map(|st| st.kind);
        if members.is_some() && is_array {
            // a complete mapping lands in an items page as ONE object item
            if st_kind != Some(MenuKind::Items) {
                self.notice = "clipboard holds key-value pairs, the target is an array".to_string();
                return;
            }
            let members = members.unwrap();
            let pre = self.doc().clone();
            let idx = self.paste_insert_index(r, &path, true, under_cursor);
            let item = self.members_as_item(doc.as_ref().unwrap(), &members);
            let _ = self.fs_mut().doc.insert(&path, idx, item);
            if nodes_equal_doc(&pre, &self.fs().doc) {
                self.notice = "paste made no changes".to_string();
                return;
            }
            self.notice = format!(
                "pasted 1 items into {}",
                crate::doc::jsondoc::path_string(&path)
            );
            return;
        }
        if items.is_some() && !is_array {
            self.notice = "clipboard holds array items, the target is a mapping".to_string();
            return;
        }
        let pre = self.doc().clone();
        if let Some(members) = members {
            let idx = self.paste_insert_index(r, &path, false, under_cursor);
            self.paste_into_object(doc.as_ref().unwrap(), &path, &members, idx);
            if nodes_equal_doc(&pre, &self.fs().doc) {
                self.notice = "paste made no changes".to_string();
                return;
            }
            self.notice = format!(
                "pasted {} keys into {}",
                members.len(),
                crate::doc::jsondoc::path_string(&path)
            );
            return;
        }
        let items = items.unwrap();
        let idx = self.paste_insert_index(r, &path, true, under_cursor);
        self.paste_into_array(&path, doc.as_ref().unwrap(), &items, idx);
        if nodes_equal_doc(&pre, &self.fs().doc) {
            self.notice = "paste made no changes".to_string();
            return;
        }
        self.notice = format!(
            "pasted {} items into {}",
            items.len(),
            crate::doc::jsondoc::path_string(&path)
        );
    }

    /// pasteTarget resolves the container the paste merges into.
    pub(super) fn paste_target(&self, r: &Row, under_cursor: bool) -> (Vec<PathSeg>, bool, String) {
        let Some(st) = self.top() else {
            return (Vec::new(), false, "nothing to paste into here".to_string());
        };
        if under_cursor && r.kind == RowKind::Drill {
            if let Some(f) = r.fn_ {
                // a drill row inside an items menu is an array item: paste
                // targets the parent array
                if st.kind == MenuKind::Items && !r.path.is_empty() {
                    if matches!(r.path.last(), Some(PathSeg::Idx(_))) {
                        let parent = r.path[..r.path.len() - 1].to_vec();
                        if !self.paste_container_ok(&parent, true) {
                            return (
                                Vec::new(),
                                false,
                                "the value under the cursor is not an array".to_string(),
                            );
                        }
                        return (parent, true, String::new());
                    }
                }
                let kind = self.fs().schema.nodes[f as usize].kind;
                match kind {
                    Kind::Object | Kind::Map => {
                        if !self.paste_container_ok(&r.path, false) {
                            return (
                                Vec::new(),
                                false,
                                "the value under the cursor is not a map".to_string(),
                            );
                        }
                        return (r.path.clone(), false, String::new());
                    }
                    Kind::Array => {
                        if !self.paste_container_ok(&r.path, true) {
                            return (
                                Vec::new(),
                                false,
                                "the value under the cursor is not an array".to_string(),
                            );
                        }
                        return (r.path.clone(), true, String::new());
                    }
                    // A oneOf row's container is whatever the current
                    // variant holds: merge keys/items into that value.
                    Kind::OneOf => {
                        if let Some(n) = self.doc().get(&r.path) {
                            match self.doc().value(n) {
                                Value::Object(_) => return (r.path.clone(), false, String::new()),
                                Value::Array(_) => return (r.path.clone(), true, String::new()),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                // a scalar/unset oneOf drill row falls through to the menu
                // container
            }
        }
        // schema-less keys: only a container value is a paste target
        if under_cursor && r.kind == RowKind::Unknown && !r.path.is_empty() {
            if let Some(n) = self.doc().get(&r.path) {
                match self.doc().value(n) {
                    Value::Object(_) => return (r.path.clone(), false, String::new()),
                    Value::Array(_) => return (r.path.clone(), true, String::new()),
                    _ => {}
                }
            }
        }
        match st.kind {
            MenuKind::Object => {
                if self.paste_container_ok(&st.path, false) {
                    return (st.path.clone(), false, String::new());
                }
                (
                    Vec::new(),
                    false,
                    "the current menu is not a map".to_string(),
                )
            }
            MenuKind::Items | MenuKind::EnumArray => {
                if self.paste_container_ok(&st.path, true) {
                    return (st.path.clone(), true, String::new());
                }
                (
                    Vec::new(),
                    false,
                    "the current menu is not an array".to_string(),
                )
            }
            MenuKind::OneOf => {
                if let Some(n) = self.doc().get(&st.path) {
                    match self.doc().value(n) {
                        Value::Object(_) => return (st.path.clone(), false, String::new()),
                        Value::Array(_) => return (st.path.clone(), true, String::new()),
                        _ => {}
                    }
                }
                (
                    Vec::new(),
                    false,
                    "a oneOf value must be set before pasting into it".to_string(),
                )
            }
        }
    }

    /// pasteContainerOK reports whether the value at path can receive
    /// container content of the given shape.
    pub(super) fn paste_container_ok(&self, path: &[PathSeg], array: bool) -> bool {
        match self.doc().get(path) {
            None => true,
            Some(n) => {
                if paste_empty_node(self.doc(), n) {
                    return true;
                }
                if array {
                    matches!(self.doc().value(n), Value::Array(_))
                } else {
                    matches!(self.doc().value(n), Value::Object(_))
                }
            }
        }
    }

    /// pasteInsertIndex is where the contiguous block of pasted content
    /// starts (Go pasteInsertIndex).
    pub(super) fn paste_insert_index(
        &self,
        r: &Row,
        path: &[PathSeg],
        array: bool,
        under_cursor: bool,
    ) -> usize {
        let Some(st) = self.top() else {
            return 0;
        };
        if st.path != path {
            // pasting into a container under the cursor: append
            if array {
                return self.doc().array_len(path);
            }
            if let Some(cur) = self.doc().get(path) {
                if let Value::Object(mems) = self.doc().value(cur) {
                    return mems.len();
                }
            }
            return 0;
        }
        if array {
            if matches!(r.kind, RowKind::Field | RowKind::Drill) && r.idx >= 0 {
                return if under_cursor {
                    r.idx as usize
                } else {
                    r.idx as usize + 1
                };
            }
            return self.doc().array_len(path);
        }
        if under_cursor {
            self.member_insert_index(r, false)
        } else {
            self.member_insert_index(r, true)
        }
    }

    /// pasteIntoObject merges the members (Go pasteIntoObject).
    pub(super) fn paste_into_object(
        &mut self,
        src: &Doc,
        path: &[PathSeg],
        members: &[PasteMember],
        mut idx: usize,
    ) {
        for mem in members {
            let exists = self
                .doc()
                .get(path)
                .and_then(|n| match self.doc().value(n) {
                    Value::Object(mems) => Some(paste_has_key(mems, &mem.key)),
                    _ => None,
                })
                .unwrap_or(false);
            let val = self
                .fs_mut()
                .doc
                .copy_into(src, mem.val, CopyMode::Detached);
            let _ = self.fs_mut().doc.insert_member(path, &mem.key, idx, val);
            if !exists {
                idx += 1;
            }
        }
    }

    /// pasteIntoArray splices the items as one contiguous block.
    pub(super) fn paste_into_array(
        &mut self,
        path: &[PathSeg],
        src: &Doc,
        items: &[NodeId],
        mut idx: usize,
    ) {
        for &it in items {
            let val = self.fs_mut().doc.copy_into(src, it, CopyMode::Detached);
            let _ = self.fs_mut().doc.insert(path, idx, val);
            idx += 1;
        }
    }

    /// membersAsItem wraps pasted members into one detached object node.
    pub(super) fn members_as_item(&mut self, src: &Doc, members: &[PasteMember]) -> NodeId {
        let doc = &mut self.fs_mut().doc;
        let obj = doc.new_object();
        let mut vals: Vec<(String, NodeId)> = Vec::new();
        for mem in members {
            let val = doc.copy_into(src, mem.val, CopyMode::Detached);
            vals.push((mem.key.clone(), val));
        }
        if let Value::Object(m) = &mut doc.node_mut(obj).value {
            for (k, val) in vals {
                m.push(Member { key: k, val });
            }
        }
        obj
    }
}
