//! Row text: title column sizing, diff/dirty badges, severity glyphs,
//! merge/anchor provenance marks and value styling.

use super::*;

impl App {
    /// menuTitleColWidth returns the adaptive title column width.
    pub(in crate::ui::keys) fn menu_title_col_width(&self, rows: &[Row]) -> usize {
        let mut width = crate::ui::menu::TITLE_COL_MIN;
        for r in rows {
            let n = match r.kind {
                RowKind::Field | RowKind::Drill => {
                    if r.fn_.is_none() {
                        continue;
                    }
                    let mut w = display_width(&self.row_title_text(r));
                    if self.field_required(r) {
                        w += 1; // required marker
                    }
                    w
                }
                RowKind::Unknown => display_width(&r.label),
                RowKind::Variant => {
                    let title = r
                        .variant
                        .map(|v| display_title(&self.fs().schema, v))
                        .unwrap_or_default();
                    display_width("○ ") + display_width(&title)
                }
                _ => continue,
            };
            if n > width {
                width = n;
            }
        }
        width.min(crate::ui::menu::TITLE_COL_MAX)
    }

    /// titleCol pads the left column to width cells.
    pub(in crate::ui::keys) fn title_col(&self, left: &str, width: usize) -> String {
        let w = display_width(left);
        if w < width {
            format!("{left}{}", " ".repeat(width - w + 1))
        } else {
            format!("{left} ")
        }
    }

    /// rowTitleText is the left-column label of a field or drill row.
    pub(in crate::ui::keys) fn row_title_text(&self, r: &Row) -> String {
        if !r.label.is_empty() {
            return r.label.clone();
        }
        match r.fn_ {
            Some(f) => self.fs().schema.nodes[f as usize].title.clone(),
            None => String::new(),
        }
    }

    /// newItemTitle labels the standalone input of the append-only flow.
    pub(in crate::ui::keys) fn new_item_title(&self, st: &MenuState) -> String {
        let mut key = self.fs().schema.nodes[st.node as usize].key.clone();
        if let Some(PathSeg::Key(s)) = st.path.last() {
            key = s.clone();
        }
        format!("{}[{}]", key, self.doc().array_len(&st.path))
    }

    /// inlineEditRow reports whether the active editor renders on r.
    pub(in crate::ui::keys) fn inline_edit_row(&self, r: &Row, e: &EditState) -> bool {
        if r.path != e.path {
            return false;
        }
        if r.kind == RowKind::Unknown {
            return true;
        }
        match (r.kind, r.fn_, e.fn_) {
            (RowKind::Field, Some(_), Some(_)) if r.fn_ == e.fn_ => true,
            (RowKind::Field, Some(f), _) | (RowKind::Drill, Some(f), _) => {
                self.fs().schema.nodes[f as usize].kind == Kind::OneOf
                    && e.synth.is_none()
                    && self.variant_contains_edit(f, e)
            }
            _ => false,
        }
    }

    pub(in crate::ui::keys) fn variant_contains_edit(
        &self,
        one_of: FormNodeId,
        e: &EditState,
    ) -> bool {
        let Some(target) = e.fn_ else { return false };
        variant_contains(&self.fs().schema, one_of, target)
    }

    pub(in crate::ui::keys) fn has_inline_edit_row(&self, rows: &[Row], e: &EditState) -> bool {
        rows.iter().any(|r| self.inline_edit_row(r, e))
    }

    /// rowLine renders one menu line (Go rowLine).
    /// drill_marker reports whether the row shows the "--->" drill marker.
    /// A oneOf field whose current variant resolved to a filled-in leaf
    /// value has nothing to drill into; the marker is kept only for mapping
    /// variants (objects/maps/arrays) and undetermined variants, where
    /// entering opens a real submenu.
    pub(in crate::ui::keys) fn drill_marker(&self, r: &Row) -> bool {
        if r.kind == RowKind::Issues {
            return true;
        }
        if r.kind != RowKind::Drill {
            return false;
        }
        let Some(f) = r.fn_ else { return true };
        let schema = &self.fs().schema;
        if schema.nodes[f as usize].kind != Kind::OneOf {
            return true;
        }
        let doc = self.doc();
        let Some(v) = doc.get(&r.path) else {
            return true;
        };
        if is_null(doc, v) {
            return true;
        }
        let Some(leaf) = matched_leaf_variant(schema, f, doc, Some(v)) else {
            return true;
        };
        matches!(
            schema.nodes[leaf as usize].kind,
            Kind::Object | Kind::Map | Kind::Array
        )
    }

    pub(in crate::ui::keys) fn row_line(
        &self,
        r: &Row,
        cursor: bool,
        title_width: usize,
    ) -> String {
        let st = self.top();
        // in-place input replaces the edited row
        if let Some(Overlay::Edit(e)) = &self.overlay {
            if self.inline_edit_row(r, e) {
                return self.edit_lines(e, &self.title_col(&self.row_title_text(r), title_width));
            }
        }
        let sel = self.row_selected(r);
        let visual = st.map(|s| s.visual).unwrap_or(false);
        let left;
        let mut right = String::new();
        match r.kind {
            RowKind::Variant => {
                let v = self.doc().get(&r.path);
                let cur = st.and_then(|s| {
                    let cur = current_variant(&self.fs().schema, s.node, self.doc(), v);
                    if cur < 0 {
                        None
                    } else {
                        Some(self.fs().schema.nodes[s.node as usize].variants[cur as usize])
                    }
                });
                let mut mark = "○";
                let mut mark_styled = style::faint(mark);
                if cur == r.variant {
                    mark = "●";
                    mark_styled = style::accent(mark);
                }
                let mut l = mark_styled;
                if let Some(vn) = r.variant {
                    // untitled variants stand in their structural signature,
                    // matching the any-type picker's name column instead of
                    // leaving the title cell blank
                    let title = display_title(&self.fs().schema, vn);
                    if !title.is_empty() {
                        l += &format!(" {title}");
                    }
                }
                left = l;
                // a titled variant keeps the signature in the summary
                // column; the untitled case already shows it as the title
                right = match r.variant {
                    Some(vn) if !self.fs().schema.nodes[vn as usize].title.is_empty() => {
                        style::faint(&variant_signature(&self.fs().schema, r.variant))
                    }
                    _ => String::new(),
                };
                if mark == "●" {
                    if let Some(v) = self.doc().get(&r.path) {
                        if !is_null(self.doc(), v) {
                            if let Some(vn) = r.variant {
                                let s = variant_value_summary(
                                    &self.fs().schema,
                                    vn,
                                    self.doc(),
                                    Some(v),
                                );
                                if !s.is_empty() {
                                    right += &format!(" {}", style::val_enum(&s));
                                }
                            }
                        }
                    }
                }
            }
            RowKind::Toggle => {
                let on = self.enum_member_on(r);
                let (box_, box_style) = if on {
                    ("[x]", style::accent("[x]"))
                } else {
                    ("[ ]", style::faint("[ ]"))
                };
                let label = r
                    .fn_
                    .and_then(|f| self.fs().schema.nodes[f as usize].items)
                    .and_then(|it| {
                        self.fs().schema.nodes[it as usize]
                            .enum_
                            .get(r.idx.max(0) as usize)
                    })
                    .map(|o| o.display.clone())
                    .unwrap_or_default();
                left = format!("{} {}", box_style, label);
                let _ = box_;
            }
            RowKind::Add | RowKind::Expand => {
                left = style::accent(&r.label);
            }
            RowKind::NoItems => {
                // An empty object menu's "< Add Key >" is an action row and
                // carries the accent color; the oneOf/enum-array placeholders
                // are informational and stay faint.
                let placeholder = matches!(
                    st.map(|s| s.kind),
                    Some(MenuKind::OneOf) | Some(MenuKind::EnumArray)
                );
                left = if placeholder {
                    style::faint(&r.label)
                } else {
                    style::accent(&r.label)
                };
            }
            RowKind::Unknown => {
                let title = r.label.clone();
                left = if sel && cursor {
                    style::bold(&style::fg9(&title))
                } else if sel {
                    style::fg9(&title)
                } else {
                    style::fg3(&title)
                };
                let v = self.doc().get(&r.path);
                right = format!(
                    "{}{}{}",
                    self.merge_badge(r),
                    self.anchor_badge(&r.path),
                    self.raw_value_styled(
                        v,
                        &raw_value_summary(&self.fs().schema, self.doc(), &r.path)
                    )
                );
            }
            RowKind::Issues => {
                left = self.issues_severity().render_str(&r.label);
            }
            _ => {
                // rowField / rowDrill
                let mut title = self.row_title_text(r);
                if self.field_required(r) {
                    title += &style::fg1("*");
                }
                left = if sel && cursor {
                    style::bold(&style::fg9(&title))
                } else if sel {
                    style::fg9(&title)
                } else {
                    title
                };
                let summ = match r.fn_ {
                    Some(f) => summary(&self.ctx(), f, &r.path),
                    None => String::new(),
                };
                right = format!(
                    "{}{}{}",
                    self.merge_badge(r),
                    self.anchor_badge(&r.path),
                    (self.value_style(r)).render_str(&summ)
                );
            }
        }
        let content = match r.kind {
            RowKind::Field | RowKind::Drill | RowKind::Unknown | RowKind::Variant => {
                format!("{}{}", self.title_col(&left, title_width), right)
            }
            _ => format!("{left}{right}"),
        };
        let mut prefix = cursor_prefix(cursor).to_string();
        if sel && (!cursor || visual) {
            prefix = style::fg9("> "); // selected rows carry a red > marker
        }
        let mut body = format!("{prefix}{content}");
        if r.kind == RowKind::Unknown && !r.in_schema && !self.fs().schema.schema_less {
            body += &format!(" {}", style::faint("(not in schema)"));
        }
        if self.drill_marker(r) {
            body += &format!(" {}", style::accent("--->"));
        }
        // The diff badge and the severity glyph are status markers: on a
        // narrow terminal the body is cut (marked with `…`) so they stay
        // visible at the end of the row.
        let mut tail = String::new();
        if matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            if self.has_err(&r.path) {
                tail = format!(" {}", style::fg1(ERR));
            } else if (r.kind == RowKind::Unknown && !r.in_schema && !self.fs().schema.schema_less)
                || self.has_warn(&r.path)
            {
                tail = format!(" {}", style::fg3(WARN));
            }
            // the LCS-aligned diff badge replaces the plain change dot:
            // one colored glyph per change kind in the row's subtree
            // (`+` added green, `-` removed red, `~` modified orange).
            // An unassigned key (no value or an unset null seed) does not
            // count as added: creating an object does not badge its empty
            // members.
            let has_value = self
                .doc()
                .get(&r.path)
                .is_some_and(|n| !matches!(self.doc().value(n), crate::doc::Value::Null));
            let badge = self.diff_badge_of(&r.path, has_value);
            match badge {
                Some(b)
                    if {
                        let w = display_width(&body) + display_width(&tail) + display_width(&b);
                        #[cfg(test)]
                        eprintln!(
                            "DBG row {:?} body={} tail={} badge={} width={}",
                            r.label,
                            display_width(&body),
                            display_width(&tail),
                            display_width(&b),
                            self.width
                        );
                        w > self.width.saturating_sub(1)
                    } =>
                {
                    // not enough room for the per-kind glyphs: one plain dot
                    tail += &format!(" {}", style::diff("●"));
                }
                Some(b) => tail += &b,
                None => {}
            }
        }
        let line = format!(
            "{}{tail}",
            clip_keep_tail(&body, self.width.saturating_sub(1), display_width(&tail))
        );
        if cursor {
            return style::sel(&line);
        }
        line
    }

    /// rawValueStyle colors a schema-less value by its JSON type.
    pub(in crate::ui::keys) fn raw_value_styled(&self, v: Option<NodeId>, s: &str) -> String {
        let styled = match v.map(|n| self.doc().value(n)) {
            None => style::faint(s),
            Some(Value::Null) => style::faint(s),
            Some(Value::Str(_)) => style::val_str(s),
            Some(Value::Num(_)) => style::val_num(s),
            Some(Value::Bool(_)) => style::val_bool(s),
            Some(_) => style::bold(s),
        };
        styled
    }

    pub(in crate::ui::keys) fn has_err(&self, path: &[PathSeg]) -> bool {
        self.fs().err_paths.iter().any(|p| is_path_prefix(path, p))
    }

    pub(in crate::ui::keys) fn has_warn(&self, path: &[PathSeg]) -> bool {
        self.fs().warn_paths.iter().any(|p| is_path_prefix(path, p))
    }

    pub(in crate::ui::keys) fn issues_severity(&self) -> Sev {
        if self.fs().issues_now.iter().any(|it| !it.warn) {
            Sev::Err
        } else {
            Sev::Warn
        }
    }

    pub(in crate::ui::keys) fn severity(&self, it: &Issue) -> String {
        if it.warn {
            style::fg3(&it.msg)
        } else {
            style::fg1(&it.msg)
        }
    }

    /// mergeNames renders the anchor list a mapping merges for its title.
    pub(in crate::ui::keys) fn merge_names(&self, path: &[PathSeg]) -> String {
        let Some(n) = self.doc().get(path) else {
            return String::new();
        };
        let mut parts: Vec<String> = Vec::new();
        for r in self.doc().merge_refs(n) {
            let anchor = self.doc().node(r).anchor.clone();
            if !anchor.is_empty() {
                parts.push(format!("*{anchor}"));
            }
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("<<: {}", parts.join(" "))
    }

    /// mergeBadge renders the "*anchor" provenance marker.
    pub(in crate::ui::keys) fn merge_badge(&self, r: &Row) -> String {
        if r.merged.is_empty() {
            return String::new();
        }
        format!("{} ", style::anchor(&format!("*{}", r.merged)))
    }

    /// anchorBadge renders the "&name" / "*name" marker.
    pub(in crate::ui::keys) fn anchor_badge(&self, path: &[PathSeg]) -> String {
        match anchor_role(self.doc(), path) {
            Some((name, true)) => format!("{} ", style::anchor(&format!("*{name}"))),
            Some((name, false)) => format!("{} ", style::anchor(&format!("&{name}"))),
            None => String::new(),
        }
    }

    /// fieldRequired reports whether the row's field is required in the
    /// current context (Go fieldRequired).
    pub(in crate::ui::keys) fn field_required(&self, r: &Row) -> bool {
        let Some(f) = r.fn_ else { return false };
        let mut required = self.fs().schema.nodes[f as usize].required;
        let Some(st) = self.top() else {
            return required;
        };
        let Some(variant_of) = st.variant_of else {
            return required;
        };
        let variants = dynamic_variants(&self.fs().schema, variant_of).unwrap_or_default();
        if variants.is_empty() {
            return required;
        }
        let key = self.fs().schema.nodes[f as usize].key.clone();
        let (_, defs) = variant_field_defs(&self.fs().schema, &variants);
        let Some(ds) = defs.get(&key) else {
            return required;
        };
        if ds.is_empty() {
            return required;
        }
        let val = self.doc().get(&st.path);
        if match_variant_list(&self.fs().schema, &variants, self.doc(), val).len() != 1 {
            // merged view: required only when every defining variant requires it
            required = true;
            for d in ds {
                if !self.fs().schema.nodes[*d as usize].required {
                    required = false;
                    break;
                }
            }
        }
        required
    }

    /// showsSchemaDefault reports whether the row previews fn.Default.
    pub(in crate::ui::keys) fn shows_schema_default(
        &self,
        f: Option<FormNodeId>,
        path: &[PathSeg],
    ) -> bool {
        let Some(f) = f else { return false };
        let node = &self.fs().schema.nodes[f as usize];
        let Some(d) = node.default_ else { return false };
        if is_null(&self.fs().schema.doc, d) {
            return false;
        }
        is_null_opt(self.doc(), self.doc().get(path))
    }

    /// valueStyle picks the value-summary color.
    pub(in crate::ui::keys) fn value_style(&self, r: &Row) -> ValStyle {
        if self.shows_schema_default(r.fn_, &r.path) {
            return ValStyle::Default;
        }
        let Some(f) = r.fn_ else { return ValStyle::Dim };
        if is_null_opt(self.doc(), self.doc().get(&r.path)) {
            return ValStyle::Dim;
        }
        let mut eff = f;
        if self.fs().schema.nodes[f as usize].kind == Kind::OneOf {
            let v = self.doc().get(&r.path);
            if let Some(leaf) = matched_leaf_variant(&self.fs().schema, f, self.doc(), v) {
                eff = leaf;
            }
        }
        match self.fs().schema.nodes[eff as usize].kind {
            Kind::Enum => ValStyle::Enum,
            Kind::Boolean => ValStyle::Bool,
            Kind::Object | Kind::Array | Kind::Map | Kind::OneOf => ValStyle::Obj,
            _ => match self.doc().get(&r.path).map(|n| self.doc().value(n)) {
                Some(Value::Num(_)) => ValStyle::Num,
                Some(Value::Bool(_)) => ValStyle::Bool,
                _ => ValStyle::Str,
            },
        }
    }

    /// enumMemberOn reports whether toggle row r's option is in the doc.
    pub(in crate::ui::keys) fn enum_member_on(&self, r: &Row) -> bool {
        let Some(cur) = self.doc().get(&r.path) else {
            return false;
        };
        let Value::Array(items) = self.doc().value(cur) else {
            return false;
        };
        let Some(f) = r.fn_ else { return false };
        let Some(it) = self.fs().schema.nodes[f as usize].items else {
            return false;
        };
        let Some(opt) = self.fs().schema.nodes[it as usize]
            .enum_
            .get(r.idx.max(0) as usize)
        else {
            return false;
        };
        items
            .iter()
            .any(|&i| self.doc().encode_value(i) == opt.encoded)
    }
}
