use super::*;

impl App {
    // --- jumps (search / issues / anchors) ----------------------------------

    /// jumpToRow rebuilds the navigation stack down to the menu that
    /// contains path and puts the cursor on the row itself.
    pub fn jump_to_row(&mut self, path: &[PathSeg]) {
        if path.is_empty() {
            self.jump_to_path(path);
            return;
        }
        self.reveal_hidden_empty(path);
        let parent = path[..path.len() - 1].to_vec();
        let stack = {
            let f = &mut self.files[self.cur];
            rebuild_stack(&mut f.schema, &f.doc, &parent)
        };
        self.fs_mut().stack = stack;
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
        if let Some(st) = self.top_mut() {
            st.cursor = cursor_by_path(&rows, path);
            st.offset = 0;
        }
    }

    /// jumpToPath rebuilds the navigation stack down to path (search-jump
    /// semantics). A oneOf key with a known value lands on its row in the
    /// parent menu instead of rebuilding the old variant picker beneath it.
    pub fn jump_to_path(&mut self, path: &[PathSeg]) {
        self.reveal_hidden_empty(path);
        if !path.is_empty() {
            let parent = path[..path.len() - 1].to_vec();
            let stack = {
                let f = &mut self.files[self.cur];
                rebuild_stack(&mut f.schema, &f.doc, &parent)
            };
            self.fs_mut().stack = stack;
            let rows = self.menu_rows();
            let variant_key = {
                let f = &self.fs();
                rows.iter().any(|r| {
                    r.path == path
                        && r.fn_.map_or(false, |fn_| {
                            f.schema.nodes[fn_ as usize].kind == Kind::OneOf
                        })
                })
            };
            if variant_key {
                self.clamp_cursor(&rows);
                if let Some(st) = self.top_mut() {
                    st.cursor = cursor_by_path(&rows, path);
                    st.offset = 0;
                }
                return;
            }
        }
        let stack = {
            let f = &mut self.files[self.cur];
            rebuild_stack(&mut f.schema, &f.doc, path)
        };
        self.fs_mut().stack = stack;
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
        if let Some(st) = self.top_mut() {
            st.cursor = cursor_by_path(&rows, path);
            st.offset = 0;
        }
    }

    /// revealHiddenEmpty turns the z filter off when path names an object
    /// key with no document value.
    pub(super) fn reveal_hidden_empty(&mut self, path: &[PathSeg]) {
        if !self.hide_empty || path.is_empty() {
            return;
        }
        if !matches!(path.last(), Some(PathSeg::Key(_))) {
            return;
        }
        if is_null_opt(self.doc(), self.doc().get(path)) {
            self.hide_empty = false;
            self.notice = "empty fields shown".to_string();
        }
    }

    /// openIssues shows the load-time issues overlay, recomputed against
    /// the current document when save-time validation is available.
    pub(super) fn open_issues(&mut self) {
        let mut items = self.fs().issues_now.clone();
        if let Some(validate) = self.opts().validate.clone() {
            items = self.live_issues(validate(&self.doc()));
            self.set_issues(items.clone());
        }
        self.fs_mut().issues_parked = None; // reopening resets the parked list
        self.overlay = Some(Overlay::Issues(IssuesState {
            items,
            cursor: 0,
            filter: Default::default(),
        }));
    }

    /// issuesReturnTo reopens the parked issue list at the jump target.
    pub(super) fn issues_return_to(&mut self) {
        let leaving = self.nav_snapshot();
        let mut items = Vec::new();
        let mut cur = 0usize;
        if let Some(parked) = self.fs().issues_parked.clone() {
            items = parked.items;
            cur = parked.cursor;
        }
        if let Some(validate) = self.opts().validate.clone() {
            items = self.live_issues(validate(&self.doc()));
            self.set_issues(items.clone());
        }
        self.fs_mut().issues_parked = None;
        if let Some(origin) = self.fs_mut().issues_origin.take() {
            self.fs_mut().stack = origin;
        }
        self.nav_back_step(leaving);
        if !items.is_empty() && cur > items.len() - 1 {
            cur = items.len() - 1;
        }
        self.overlay = Some(Overlay::Issues(IssuesState {
            items,
            cursor: cur,
            filter: ListFilter {
                restore: cur,
                ..Default::default()
            },
        }));
    }

    // --- search ------------------------------------------------------------

    /// openSearch arms the scoped search overlay (Go openSearch).
    pub fn open_search(&mut self, replace: bool) {
        let st = self.top().cloned();
        let all = match &st {
            Some(st) => {
                let f = &mut self.files[self.cur];
                search_index(&mut f.schema, &f.doc, st)
            }
            None => Vec::new(),
        };
        let mut ti = TextInput::new();
        ti.set_value("");
        let rti = TextInput::new();
        let mut s = SearchState {
            replace,
            fold_case: true,
            ti,
            rti,
            replace_focused: false,
            all,
            results: Vec::new(),
            cursor: 0,
            regex: true, // regex queries by default; alt+x falls back to literal
            re: None,
            re_err: String::new(),
            parked: false,
            return_depth: 0,
            origin: None,
        };
        if replace {
            self.refilter(&mut s);
        } else {
            s.results = s.all.clone();
        }
        self.fs_mut().search_parked = None; // a fresh search replaces a parked one
        self.overlay = Some(Overlay::Search(s));
    }

    /// closeSearch tears the overlay down, returning to the menu the search
    /// was opened from.
    pub(super) fn close_search(&mut self, s: SearchState) {
        if let Some(origin) = s.origin {
            self.fs_mut().stack = origin;
        }
        self.fs_mut().search_parked = None;
    }

    /// searchReturnTo reopens the parked search overlay from its jump
    /// target.
    pub(super) fn search_return_to(&mut self) {
        let leaving = self.nav_snapshot();
        let Some(mut s) = self.fs_mut().search_parked.take() else {
            return;
        };
        let cur = s.cursor;
        s.parked = false;
        s.return_depth = 0;
        if let Some(origin) = s.origin.take() {
            self.fs_mut().stack = origin;
        }
        self.nav_back_step(leaving);
        {
            let st = self.top().cloned();
            if let Some(st) = st {
                let f = &mut self.files[self.cur];
                s.all = search_index(&mut f.schema, &f.doc, &st);
            }
        }
        self.refilter(&mut s);
        let mut cur = cur;
        if cur > s.results.len().saturating_sub(1) {
            cur = s.results.len().saturating_sub(1);
        }
        s.cursor = cur;
        self.fs_mut().search_parked = None;
        self.overlay = Some(Overlay::Search(s));
    }

    /// refilter recomputes the result list from the current inputs (Go
    /// refilter).
    pub(super) fn refilter(&mut self, s: &mut SearchState) {
        let q = s.ti.value().to_string();
        s.re = None;
        s.re_err = String::new();
        if s.regex && !q.is_empty() {
            let mut pattern = q.clone();
            if !s.replace && (is_path_query(&q) || is_dot_query(&q)) {
                pattern = path_pattern(&q);
            }
            if s.fold_case {
                pattern = format!("(?i){pattern}");
            }
            match Regex::new(&pattern) {
                Err(err) => {
                    // report the error against the typed query, not our wrappers
                    if let Err(raw_err) = Regex::new(&q) {
                        s.re_err = raw_err.to_string();
                    } else {
                        s.re_err = err.to_string();
                    }
                }
                Ok(re) => s.re = Some(re),
            }
        }
        if !s.re_err.is_empty() {
            s.results = Vec::new();
            s.cursor = 0;
            return;
        }
        let ctx = self.ctx();
        let rep = s.rti.value().to_string();
        if s.replace {
            s.results = replace_hits(&ctx, &s.all, &q, &rep, s.fold_case, s.re.as_ref());
        } else {
            s.results = filter_search(&ctx, &s.all, &q, s.fold_case, s.re.as_ref());
        }
        s.cursor = 0;
    }

    /// replaceSearchCurrent rewrites the highlighted hit's string value.
    pub(super) fn replace_search_current(&mut self, s: &mut SearchState) {
        let q = s.ti.value().to_string();
        if q.is_empty() || s.results.is_empty() {
            return;
        }
        let res = s.results[s.cursor].clone();
        let Some(n) = self.doc().get(&res.path) else {
            return;
        };
        let Value::Str(old) = self.doc().value(n).clone() else {
            return;
        };
        let rep = s.rti.value().to_string();
        let (new, c) = replace_all(&old, &q, &rep, s.fold_case, s.re.as_ref());
        if c == 0 {
            return;
        }
        let v = self.fs_mut().doc.new_str(new);
        let _ = self.fs_mut().doc.set(&res.path, v);
        let cur = s.cursor;
        self.refilter(s);
        let mut cur = cur;
        if cur >= s.results.len() {
            cur = s.results.len().saturating_sub(1);
        }
        s.cursor = cur;
    }

    /// replaceSearchAll rewrites every occurrence in every string value of
    /// the scoped index, closes the overlay and leaves a count notice.
    pub(super) fn replace_search_all(&mut self, s: SearchState) {
        let q = s.ti.value().to_string();
        if q.is_empty() || s.results.is_empty() {
            self.overlay = Some(Overlay::Search(s));
            return;
        }
        let rep = s.rti.value().to_string();
        let mut occ = 0usize;
        let mut vals = 0usize;
        let paths: Vec<Vec<PathSeg>> = s.results.iter().map(|r| r.path.clone()).collect();
        for p in paths {
            let Some(n) = self.doc().get(&p) else {
                continue;
            };
            let Value::Str(old) = self.doc().value(n).clone() else {
                continue;
            };
            let (new, c) = replace_all(&old, &q, &rep, s.fold_case, s.re.as_ref());
            if c == 0 {
                continue;
            }
            let v = self.fs_mut().doc.new_str(new);
            let _ = self.fs_mut().doc.set(&p, v);
            occ += c;
            vals += 1;
        }
        self.close_search(s);
        self.overlay = None;
        self.notice = format!("replaced {} occurrence(s) in {} value(s)", occ, vals);
    }

    // --- anchors -----------------------------------------------------------

    /// gotoAnchor performs ga: from a reference row, jump to its anchor
    /// carrier (Go gotoAnchor).
    pub(super) fn goto_anchor(&mut self, r: &Row) {
        if !r.merged.is_empty() {
            // a merged key has no anchor identity of its own: ga lands on
            // the field that defines the value inside the anchor's mapping
            self.anchor_jump_row(r.path.clone());
            return;
        }
        let Some((name, is_ref)) = anchor_role(self.doc(), &r.path) else {
            self.notice = "not on an anchor or reference".to_string();
            return;
        };
        if !is_ref {
            self.notice = format!("already at the anchor &{}", name);
            return;
        }
        let Some(n) = self.doc().get(&r.path) else {
            return;
        };
        let target = anchor_holder_path(self.doc(), n).unwrap_or_default();
        self.anchor_jump(target);
    }

    /// gotoRefs performs gr (Go gotoRefs).
    pub(super) fn goto_refs(&mut self, r: &Row) {
        let name;
        let n;
        if !r.merged.is_empty() {
            name = r.merged.clone();
            match anchor_holder(self.doc(), &name) {
                Some(node) => n = node,
                None => {
                    self.notice = format!("the anchor &{} is no longer in the document", name);
                    return;
                }
            }
        } else {
            match anchor_role(self.doc(), &r.path) {
                Some((nm, _)) => {
                    name = nm;
                    n = self
                        .doc()
                        .get(&r.path)
                        .expect("anchor role checked the value");
                }
                None => {
                    self.notice = "not on an anchor or reference".to_string();
                    return;
                }
            }
        }
        let holder = anchor_holder_path(self.doc(), n).unwrap_or_default();
        let here = self.top().map(|st| st.path.clone()).unwrap_or_default();
        let mut targets: Vec<Vec<PathSeg>> = Vec::new();
        for p in anchor_merge_use_paths(self.doc(), n) {
            if p == holder || p == r.path || p == here {
                continue;
            }
            targets.push(p);
        }
        match targets.len() {
            0 => self.notice = format!("no other references to &{}", name),
            1 => self.anchor_jump(targets.remove(0)),
            _ => {
                self.overlay = Some(Overlay::RefPick(RefPickState {
                    name,
                    paths: targets,
                    cursor: 0,
                    filter: Default::default(),
                }));
            }
        }
    }

    /// gAnchorKeys consumes the armed-g combos ga and gr.
    pub(super) fn g_anchor_keys(&mut self, msg: &Msg, r: &Row) -> bool {
        if self.cmd_buf.is_empty() {
            return false;
        }
        match msg.key_str().as_str() {
            "a" => {
                self.cmd_buf.clear();
                self.goto_anchor(r);
                true
            }
            "r" => {
                self.cmd_buf.clear();
                self.goto_refs(r);
                true
            }
            _ => false,
        }
    }

    /// expandAlias materializes the reference at path (Go expandAlias).
    pub(super) fn expand_alias(&mut self, path: &[PathSeg]) {
        let Some(n) = self.doc().get(path) else {
            return;
        };
        let cloned = self.fs_mut().doc.clone_detached(n);
        let _ = self.fs_mut().doc.detach(path, cloned);
        self.notice = "alias expanded: this value is now an independent copy".to_string();
    }

    /// expandTarget resolves what `e` materializes (Go expandTarget).
    pub(super) fn expand_target(&self, r: &Row) -> Option<Vec<PathSeg>> {
        if matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown)
            && !r.path.is_empty()
        {
            if let Some((_, true)) = anchor_role(self.doc(), &r.path) {
                return Some(r.path.clone());
            }
        }
        let top_path = self.top().map(|st| st.path.clone()).unwrap_or_default();
        if let Some((_, true)) = anchor_role(self.doc(), &top_path) {
            return Some(top_path);
        }
        None
    }

    /// anchorJump parks the current position and jumps to target.
    pub(super) fn anchor_jump(&mut self, target: Vec<PathSeg>) {
        self.anchor_jump_to(target, false);
    }

    /// anchorJumpRow lands on the target row itself.
    pub(super) fn anchor_jump_row(&mut self, target: Vec<PathSeg>) {
        self.anchor_jump_to(target, true);
    }

    pub(super) fn anchor_jump_to(&mut self, target: Vec<PathSeg>, as_row: bool) {
        self.nav_forward();
        self.fs_mut().jump_origin = Some(self.fs().stack.clone());
        self.fs_mut().search_parked = None; // a fresh jump replaces parked overlays
        self.fs_mut().issues_parked = None;
        if as_row {
            self.jump_to_row(&target);
        } else {
            self.jump_to_path(&target);
        }
        self.fs_mut().jump_return = self.fs().stack.len();
        self.notice = "esc returns".to_string();
    }

    /// openAnchor opens the name input behind & and * (Go openAnchor).
    pub(super) fn open_anchor(&mut self, r: &Row, mode: AnchorMode) {
        if !matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            self.notice = "& and * work on value rows".to_string();
            return;
        }
        if r.path.is_empty() {
            self.notice = "no value here to anchor".to_string();
            return;
        }
        let mut ti = TextInput::new();
        let role = anchor_role(self.doc(), &r.path);
        let (name, _is_ref) = role.clone().map(|(n, ir)| (n, ir)).unwrap_or_default();
        match (&mode, role) {
            (AnchorMode::RefSet, Some((_, true))) | (AnchorMode::AnchorSet, Some(_)) => {
                // prefill the current name: rename for anchors, retarget for refs
                ti.set_value(name);
                ti.cursor_end();
            }
            _ => {}
        }
        let mut cands: Vec<String> = Vec::new();
        if mode == AnchorMode::RefSet {
            // completion offers only anchors the field can legally hold
            for d in anchor_defs(self.doc()) {
                if !schema_accepts_value_(&self.fs().schema, r.fn_, self.doc(), Some(d.node)) {
                    continue;
                }
                if ref_would_cycle(self.doc(), &r.path, d.node) {
                    continue;
                }
                cands.push(d.name);
            }
        }
        let mut s = AnchorState {
            path: r.path.clone(),
            mode,
            fn_: r.fn_,
            ti,
            cands,
            sug_prefix: String::new(),
            sugs: Vec::new(),
            sug_line: String::new(),
            sug: -1,
            err: String::new(),
        };
        s.sug_prefix = s.ti.value().to_string();
        s.sugs = Self::anchor_filtered(&s);
        self.overlay = Some(Overlay::Anc(s));
    }

    /// anchorFiltered lists the reference candidates matching the input.
    pub(super) fn anchor_filtered(s: &AnchorState) -> Vec<String> {
        if s.mode != AnchorMode::RefSet {
            return Vec::new();
        }
        let prefix = s.ti.value().trim().to_lowercase();
        if prefix.is_empty() {
            return s.cands.clone();
        }
        s.cands
            .iter()
            .filter(|c| c.to_lowercase().starts_with(&prefix))
            .cloned()
            .collect()
    }

    /// cycleAnchorSug moves the completion cursor over the candidates.
    pub(super) fn cycle_anchor_sug(&mut self, s: &mut AnchorState, delta: i64) {
        if s.mode != AnchorMode::RefSet {
            return;
        }
        // Capture the list under the typed prefix on the first tab. Later
        // tabs/arrows must keep cycling that list: recomputing from the
        // completed name would shrink it to that one candidate.
        if s.sug < 0 || s.ti.value() != s.sug_line {
            s.sug_prefix = s.ti.value().to_string();
            s.sugs = Self::anchor_filtered(s);
            s.sug = -1;
        }
        if s.sugs.is_empty() {
            return;
        }
        s.sug += delta;
        if s.sug < 0 {
            s.sug = s.sugs.len() as i64 - 1;
        }
        if s.sug >= s.sugs.len() as i64 {
            s.sug = 0;
        }
        let candidate = s.sugs[s.sug as usize].clone();
        s.ti.set_value(candidate.clone());
        s.ti.cursor_end();
        s.sug_line = candidate;
    }

    /// anchorCommit applies the & / * input (Go anchorCommit).
    pub(super) fn anchor_commit(&mut self, s: AnchorState) -> Option<Overlay> {
        let name = s.ti.value().trim().to_string();
        if s.mode == AnchorMode::AnchorSet {
            self.commit_anchor_set(s, name)
        } else {
            self.commit_ref_set(s, name)
        }
    }

    pub(super) fn commit_anchor_set(
        &mut self,
        mut s: AnchorState,
        name: String,
    ) -> Option<Overlay> {
        let Some(n) = self.doc().get(&s.path) else {
            s.err = "no value here".to_string();
            return Some(Overlay::Anc(s));
        };
        let anchor = self.doc().node(n).anchor.clone();
        if name.is_empty() {
            if anchor.is_empty() {
                s.err = "this value has no anchor".to_string();
                return Some(Overlay::Anc(s));
            }
            let uses = count_anchor_uses(self.doc(), n);
            if uses > 1 {
                s.err = format!(
                    "&{} is referenced by {} alias(es) — remove them first",
                    anchor,
                    uses - 1
                );
                return Some(Overlay::Anc(s));
            }
            if merge_ref_uses(self.doc(), n) > 0 {
                s.err = format!(
                    "&{} is referenced by a merge key — remove the merge first",
                    anchor
                );
                return Some(Overlay::Anc(s));
            }
            self.fs_mut().doc.node_mut(n).anchor = String::new();
            self.notice = format!("anchor &{} removed", anchor);
            return None;
        }
        if let Some(holder) = anchor_holder(self.doc(), &name) {
            if holder != n {
                s.err = format!("anchor name &{} is already in use", name);
                return Some(Overlay::Anc(s));
            }
        }
        let renamed = !anchor.is_empty() && anchor != name;
        self.fs_mut().doc.node_mut(n).anchor = name.clone();
        if renamed {
            self.notice = format!("anchor renamed to &{name} — references updated");
        } else {
            self.notice = format!("anchor set: &{name}");
        }
        None
    }

    pub(super) fn commit_ref_set(&mut self, mut s: AnchorState, name: String) -> Option<Overlay> {
        if name.is_empty() {
            match anchor_role(self.doc(), &s.path) {
                Some((_, true)) => {}
                _ => {
                    s.err = "this value is not a reference".to_string();
                    return Some(Overlay::Anc(s));
                }
            }
            if let Some(f) = s.fn_ {
                if self.fs().schema.nodes[f as usize].required {
                    s.err = format!("{} is required", self.fs().schema.nodes[f as usize].title);
                    return Some(Overlay::Anc(s));
                }
            }
            self.fs_mut().doc.remove(&s.path);
            self.notice = "reference removed".to_string();
            return None;
        }
        let Some(target) = anchor_holder(self.doc(), &name) else {
            s.err = format!("unknown anchor: &{name}");
            return Some(Overlay::Anc(s));
        };
        if ref_would_cycle(self.doc(), &s.path, target) {
            s.err = "circular reference: the anchor contains this field".to_string();
            return Some(Overlay::Anc(s));
        }
        if !self.fs_mut().doc.detach(&s.path, target) {
            // the field holds no value yet: Set creates the slot
            let _ = self.fs_mut().doc.set(&s.path, target);
        }
        self.notice = format!("value references *{}", name);
        None
    }

    // --- merge keys --------------------------------------------------------

    /// openMerge opens the merge-keys input for the current mapping menu
    /// (Go openMerge).
    pub(super) fn open_merge(&mut self) {
        let Some(st) = self.top().cloned() else {
            return;
        };
        let node_kind = self.fs().schema.nodes[st.node as usize].kind;
        if st.kind != MenuKind::Object || !matches!(node_kind, Kind::Object | Kind::Map) {
            self.notice = "< edits the merge keys of a mapping menu".to_string();
            return;
        }
        // a value of another kind refuses (converting it would lose data)
        if let Some(cur) = self.doc().get(&st.path) {
            if !is_null_val(self.doc(), cur) && !matches!(self.doc().value(cur), Value::Object(_)) {
                self.notice = "no mapping here to merge into".to_string();
                return;
            }
        }
        let mut pre: Vec<String> = Vec::new();
        if let Some(cur) = self.doc().get(&st.path) {
            for r in self.doc().merge_refs(cur) {
                let anchor = self.doc().node(r).anchor.clone();
                if !anchor.is_empty() {
                    pre.push(anchor);
                }
            }
        }
        let mut ti = TextInput::new();
        if !pre.is_empty() {
            ti.set_value(pre.join(" "));
            ti.cursor_end();
        }
        let mut cands: Vec<String> = Vec::new();
        for d in anchor_defs(self.doc()) {
            if !matches!(self.doc().value(d.node), Value::Object(_)) {
                continue;
            }
            if self.merge_would_cycle(&st.path, d.node) {
                continue;
            }
            cands.push(d.name);
        }
        self.overlay = Some(Overlay::Mrg(MergeState {
            path: st.path.clone(),
            ti,
            cands,
            sug: -1,
            err: String::new(),
        }));
    }

    /// cycleMergeSug moves the completion cursor over the filtered
    /// candidates and fills the token being typed.
    pub(super) fn cycle_merge_sug(&mut self, s: &mut MergeState, delta: i64) {
        let cands = merge_filtered(s);
        if cands.is_empty() {
            return;
        }
        s.sug += delta;
        if s.sug < 0 {
            s.sug = cands.len() as i64 - 1;
        }
        if s.sug >= cands.len() as i64 {
            s.sug = 0;
        }
        let (rest, _) = split_last_token(s.ti.value());
        s.ti.set_value(format!("{}{} ", rest, cands[s.sug as usize]));
        s.ti.cursor_end();
    }

    /// mergeCommit applies the `<` input (Go mergeCommit).
    pub(super) fn merge_commit(&mut self, mut s: MergeState) -> Option<Overlay> {
        let names = merge_token_list(s.ti.value());
        if names.is_empty() {
            let has_merges = self
                .doc()
                .get(&s.path)
                .map(|n| !self.doc().merge_refs(n).is_empty())
                .unwrap_or(false);
            if !has_merges {
                s.err = "this mapping merges nothing".to_string();
                return Some(Overlay::Mrg(s));
            }
            self.apply_merges(&s.path, Vec::new());
            self.notice = "merge keys removed".to_string();
            return None;
        }
        let mut dedup: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for n in names {
            if seen.insert(n.clone()) {
                dedup.push(n);
            }
        }
        let self_node = self.doc().get(&s.path);
        let mut refs: Vec<NodeId> = Vec::new();
        for name in &dedup {
            let Some(target) = anchor_holder(self.doc(), name) else {
                s.err = format!("unknown anchor: &{name}");
                return Some(Overlay::Mrg(s));
            };
            if !matches!(self.doc().value(target), Value::Object(_)) {
                s.err = format!("&{name} is not a mapping anchor");
                return Some(Overlay::Mrg(s));
            }
            if Some(target) == self_node {
                s.err = "cannot merge the mapping into itself".to_string();
                return Some(Overlay::Mrg(s));
            }
            if ref_would_cycle(self.doc(), &s.path, target)
                || self.merge_would_cycle(&s.path, target)
            {
                s.err =
                    "circular reference: the anchor would merge back into this mapping".to_string();
                return Some(Overlay::Mrg(s));
            }
            refs.push(target);
        }
        // a not-yet-created mapping becomes an empty object first
        let cur = self.doc().get(&s.path);
        if is_null_opt(self.doc(), cur) {
            let n = self.fs_mut().doc.new_object();
            let _ = self.fs_mut().doc.set(&s.path, n);
        }
        self.apply_merges(&s.path, refs);
        self.notice = format!("merged keys from {}", dedup.join(" "));
        None
    }

    /// applyMerges replaces the mapping's merge-key members with one member
    /// merging refs (empty clears them).
    pub(super) fn apply_merges(&mut self, path: &[PathSeg], refs: Vec<NodeId>) {
        let Some(cur) = self.doc().get(path) else {
            return;
        };
        if !matches!(self.doc().value(cur), Value::Object(_)) {
            return;
        }
        let mut idx: Option<usize> = None;
        if let Value::Object(mems) = self.doc().value(cur) {
            for (i, m) in mems.iter().enumerate() {
                if !self.doc().node(m.val).merge.is_empty() && idx.is_none() {
                    idx = Some(i);
                }
            }
        }
        let member = append_key(path, "<<");
        while self.fs_mut().doc.remove(&member) {}
        if refs.is_empty() {
            return;
        }
        let idx = idx.unwrap_or(0);
        let n = self.fs_mut().doc.push(NodeData {
            value: Value::Null,
            anchor: String::new(),
            merge: refs,
            raw: String::new(),
        });
        let _ = self.fs_mut().doc.insert_member(path, "<<", idx, n);
    }

    /// mergeWouldCycle reports whether merging target into the mapping at
    /// path would recurse (Go mergeWouldCycle).
    pub(super) fn merge_would_cycle(&self, path: &[PathSeg], target: NodeId) -> bool {
        let Some(self_node) = self.doc().get(path) else {
            return false;
        };
        let mut seen: HashSet<NodeId> = HashSet::new();
        fn dfs(doc: &Doc, self_node: NodeId, n: NodeId, seen: &mut HashSet<NodeId>) -> bool {
            if !seen.insert(n) {
                return false;
            }
            if n == self_node {
                return true;
            }
            for r in doc.merge_refs(n) {
                if dfs(doc, self_node, r, seen) {
                    return true;
                }
            }
            false
        }
        dfs(self.doc(), self_node, target, &mut seen)
    }

    /// materializeMerged copies one merged key's value into the current
    /// mapping (Go materializeMerged).
    pub(super) fn materialize_merged(&mut self, r: &Row) -> Row {
        let Some(st) = self.top() else {
            return r.clone();
        };
        if r.path.is_empty() {
            return r.clone();
        }
        let Some(PathSeg::Key(key)) = r.path.last() else {
            return r.clone();
        };
        let Some(val) = self.doc().get(&r.path) else {
            return r.clone();
        };
        let p = append_key(&st.path, key);
        let cloned = self.fs_mut().doc.clone_detached(val);
        let _ = self.fs_mut().doc.set(&p, cloned);
        self.notice = "merged key expanded: this value is now an independent copy".to_string();
        let mut out = r.clone();
        out.path = p;
        out.merged = String::new();
        out
    }

    /// expandMerged materializes the merged key under the cursor (E).
    pub(super) fn expand_merged(&mut self, r: &Row) {
        self.materialize_merged(r);
    }
}
