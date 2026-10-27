use super::*;

impl App {
    // --- macros ------------------------------------------------------------

    /// macroControl consumes the macro keys (@ records, . plays) before
    /// normal dispatch (Go macroControl).
    pub fn macro_control(&mut self, msg: &Msg) -> bool {
        if self.typing() {
            return false; // typing: @/. are ordinary characters
        }
        match msg.key_str().as_str() {
            "@" => {
                if self.recording {
                    if self.rec_buf.is_empty() {
                        // cancel: the previous macro survives
                        self.rec_buf.clear();
                        self.recording = false;
                        self.notice = "recording cancelled (no keys captured)".to_string();
                        return true;
                    }
                    self.macro_buf = std::mem::take(&mut self.rec_buf);
                    self.recording = false;
                    self.notice =
                        format!("macro recorded: {} keys — . plays it", self.macro_buf.len());
                    return true;
                }
                self.rec_buf = Vec::with_capacity(16);
                self.recording = true;
                self.notice = "recording — @ stops, . is ignored while recording".to_string();
                true
            }
            "." => {
                if self.recording {
                    return true; // swallowed, not recorded
                }
                if self.macro_buf.is_empty() {
                    self.notice = "no macro recorded (@ records one)".to_string();
                    return true;
                }
                self.play_macro();
                true
            }
            _ => false,
        }
    }

    /// playMacro replays the recorded keys through dispatch_key (Go
    /// playMacro).
    pub(super) fn play_macro(&mut self) {
        let n = self.macro_buf.len();
        for (i, k) in self.macro_buf.clone().into_iter().enumerate() {
            self.dispatch_key(k);
            if self.aborted || self.exiting {
                self.notice = format!("macro stopped at key {}/{} (session ended)", i + 1, n);
                return;
            }
        }
        self.notice = format!("played {} keys", n);
    }

    // --- overlay key handlers ----------------------------------------------

    /// keyConfirm drives the y/n overlay (Go keyConfirm).
    pub fn key_confirm(&mut self, msg: Msg, c: ConfirmState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "y" | "enter" | " " => {
                if let Some(on_yes) = c.on_yes {
                    on_yes(self);
                }
                // the callback may have opened another surface (the enum
                // picker of a chosen variant, the save-errors overlay):
                // keep whatever it installed, like Go's model fields.
                self.overlay.take()
            }
            "n" => {
                if let Some(on_no) = c.on_no {
                    on_no(self);
                }
                self.overlay.take()
            }
            "esc" => None,
            _ => Some(Overlay::Confirm(c)),
        }
    }

    /// keyHelp drives the "?" keyboard-help overlay.
    pub fn key_help(&mut self, msg: &Msg, mut s: HelpState) -> Option<Overlay> {
        // title + blank line + bottom hint leave height-3 entry rows.
        let page = self.height.saturating_sub(3).max(1);
        let max_scroll = crate::ui::menu::HELP_ENTRIES.len().saturating_sub(page);
        match msg.key_str().as_str() {
            "esc" | "q" | "?" => return None,
            "up" | "k" => s.scroll = s.scroll.saturating_sub(1),
            "down" | "j" => s.scroll = (s.scroll + 1).min(max_scroll),
            "pgup" | "ctrl+b" => s.scroll = s.scroll.saturating_sub(page),
            "pgdown" | "ctrl+f" => s.scroll = (s.scroll + page).min(max_scroll),
            _ => {}
        }
        Some(Overlay::Help(s))
    }

    /// keyErrs drives the save-time errors overlay (Go keyErrs).
    pub fn key_errs(&mut self, msg: Msg, mut s: ErrsState) -> Option<Overlay> {
        if s.filter.active {
            return self.key_errs_search(msg, s);
        }
        if self.jump_keys(&msg, &mut s.cursor, s.lines.len()) {
            return Some(Overlay::Errs(s));
        }
        match msg.key_str().as_str() {
            "/" => {
                s.filter.restore = s.cursor;
                s.filter.ti = TextInput::default();
                s.filter.active = true;
            }
            "up" | "k" => s.cursor = wrap_cursor(s.cursor, -1, s.lines.len()),
            "down" | "j" => s.cursor = wrap_cursor(s.cursor, 1, s.lines.len()),
            "pgdown" | "ctrl+f" => {
                let page = self.overlay_page_size(0, false);
                s.cursor = page_cursor(s.cursor, 1, page, s.lines.len());
            }
            "pgup" | "ctrl+b" => {
                let page = self.overlay_page_size(0, false);
                s.cursor = page_cursor(s.cursor, -1, page, s.lines.len());
            }
            "enter" => {
                if let Some(Some(path)) = s.paths.get(s.cursor) {
                    let path = path.clone();
                    self.fs_mut().issues_parked = None; // a fresh jump replaces a parked list
                    self.nav_forward();
                    self.jump_to_path(&path);
                    return None;
                }
            }
            "y" => {
                if self.opts().force {
                    let exit = s.exit;
                    let target = s.target.clone();
                    self.write_to(exit, true, &target);
                    return None;
                }
            }
            "n" | "esc" => return None,
            _ => {}
        }
        Some(Overlay::Errs(s))
    }

    /// keyErrsSearch consumes keys while the save-error filter is active.
    pub(super) fn key_errs_search(&mut self, msg: Msg, mut s: ErrsState) -> Option<Overlay> {
        let n = err_indices(&s).count();
        let page = self.overlay_page_size(0, true);
        if matches!(
            s.filter.key(&msg, &mut s.cursor, n, s.lines.len(), page),
            FilterAction::Commit
        ) {
            let path = err_indices(&s)
                .nth(s.cursor)
                .and_then(|i| s.paths.get(i))
                .and_then(|p| p.clone());
            if let Some(path) = path {
                self.fs_mut().issues_parked = None; // a fresh jump replaces a parked list
                self.nav_forward();
                self.jump_to_path(&path);
                return None;
            }
        }
        Some(Overlay::Errs(s))
    }

    /// keyIssues drives the load-time issues overlay (Go keyIssues).
    pub fn key_issues(&mut self, msg: Msg, mut s: IssuesState) -> Option<Overlay> {
        if s.filter.active {
            return self.key_issues_search(msg, s);
        }
        if self.jump_keys(&msg, &mut s.cursor, s.items.len()) {
            return Some(Overlay::Issues(s));
        }
        match msg.key_str().as_str() {
            "/" if !s.items.is_empty() => s.filter.open(s.cursor),
            "up" | "k" | "ctrl+k" => s.cursor = wrap_cursor(s.cursor, -1, s.items.len()),
            "down" | "j" | "ctrl+j" => s.cursor = wrap_cursor(s.cursor, 1, s.items.len()),
            "pgdown" | "ctrl+f" => {
                let extra = usize::from(!self.notice.is_empty());
                let page = self.overlay_page_size(extra, false);
                s.cursor = page_cursor(s.cursor, 1, page, s.items.len());
            }
            "pgup" | "ctrl+b" => {
                let extra = usize::from(!self.notice.is_empty());
                let page = self.overlay_page_size(extra, false);
                s.cursor = page_cursor(s.cursor, -1, page, s.items.len());
            }
            "enter" => {
                let items = s.items.clone();
                if self.issues_jump(&items, s.cursor) {
                    return None;
                }
            }
            "esc" => return None,
            _ => {}
        }
        Some(Overlay::Issues(s))
    }

    /// keyIssuesSearch consumes keys while the issue-list filter is active.
    pub(super) fn key_issues_search(&mut self, msg: Msg, mut s: IssuesState) -> Option<Overlay> {
        let n = issue_indices(&s).count();
        let total = s.items.len();
        let page = self.overlay_page_size(0, true);
        if matches!(
            s.filter.key(&msg, &mut s.cursor, n, total, page),
            FilterAction::Commit
        ) {
            if let Some(idx) = issue_indices(&s).nth(s.cursor) {
                let items = s.items.clone();
                if self.issues_jump(&items, idx) {
                    return None;
                }
            }
        }
        Some(Overlay::Issues(s))
    }

    /// issuesJump parks the list and jumps to one issue's document path.
    pub(super) fn issues_jump(&mut self, items: &[Issue], idx: usize) -> bool {
        let Some(path) = items.get(idx).and_then(|it| it.path.clone()) else {
            return false;
        };
        // closing the list (this park cycle or a later one) returns here
        self.fs_mut().issues_parked = Some(IssuesState {
            items: items.to_vec(),
            cursor: idx,
            filter: ListFilter {
                restore: idx,
                ..Default::default()
            },
        });
        self.fs_mut().issues_origin = Some(self.fs().stack.clone());
        self.fs_mut().search_parked = None; // a fresh jump replaces a parked search
        self.fs_mut().jump_origin = None;
        self.fs_mut().jump_return = 0;
        self.nav_forward();
        self.jump_to_path(&path);
        self.fs_mut().issues_return = self.fs().stack.len();
        true
    }

    /// keyRefPick drives the gr picker (Go keyRefPick).
    pub fn key_ref_pick(&mut self, msg: Msg, mut s: RefPickState) -> Option<Overlay> {
        if s.filter.active {
            return self.key_ref_search(msg, s);
        }
        if self.jump_keys(&msg, &mut s.cursor, s.paths.len()) {
            return Some(Overlay::RefPick(s));
        }
        match msg.key_str().as_str() {
            "/" => s.filter.open(s.cursor),
            "up" | "k" | "ctrl+k" => s.cursor = wrap_cursor(s.cursor, -1, s.paths.len()),
            "down" | "j" | "ctrl+j" => s.cursor = wrap_cursor(s.cursor, 1, s.paths.len()),
            "pgdown" | "ctrl+f" => {
                let page = self.overlay_page_size(0, false);
                s.cursor = page_cursor(s.cursor, 1, page, s.paths.len());
            }
            "pgup" | "ctrl+b" => {
                let page = self.overlay_page_size(0, false);
                s.cursor = page_cursor(s.cursor, -1, page, s.paths.len());
            }
            "enter" => {
                if let Some(path) = s.paths.get(s.cursor).cloned() {
                    self.anchor_jump(path);
                    return None;
                }
            }
            "esc" => return None,
            _ => {}
        }
        Some(Overlay::RefPick(s))
    }

    /// keyRefSearch consumes keys while the reference-picker filter is
    /// active.
    pub(super) fn key_ref_search(&mut self, msg: Msg, mut s: RefPickState) -> Option<Overlay> {
        let n = self.ref_indices(&s).count();
        let total = s.paths.len();
        let page = self.overlay_page_size(0, true);
        if matches!(
            s.filter.key(&msg, &mut s.cursor, n, total, page),
            FilterAction::Commit
        ) {
            let idx = self.ref_indices(&s).nth(s.cursor);
            if let Some(i) = idx {
                let path = s.paths[i].clone();
                self.anchor_jump(path);
                return None;
            }
        }
        Some(Overlay::RefPick(s))
    }

    /// ref_label renders one reference row: path plus value summary.
    pub(super) fn ref_label(&self, p: &[PathSeg]) -> String {
        format!(
            "{}  {}",
            crate::doc::jsondoc::path_string(p),
            raw_value_summary(&self.fs().schema, self.doc(), p)
        )
    }

    /// ref_indices lists the reference paths matching the picker query.
    pub(super) fn ref_indices<'a>(
        &'a self,
        s: &'a RefPickState,
    ) -> impl Iterator<Item = usize> + 'a {
        let q = s.filter.ti.value().to_lowercase();
        s.paths
            .iter()
            .enumerate()
            .filter(move |(_, p)| q.is_empty() || self.ref_label(p).to_lowercase().contains(&q))
            .map(|(i, _)| i)
    }

    /// keyText drives the raw textarea overlay (Go keyText).
    pub fn key_text(&mut self, msg: Msg, mut s: TextState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "ctrl+s" => match self.apply_text(&s) {
                Ok(()) => return None,
                Err(e) => {
                    s.err = e;
                    return Some(Overlay::Text(s));
                }
            },
            "esc" => return None,
            _ => {}
        }
        if let Some(k) = msg_key(&msg) {
            s.ta.handle_key(k);
        }
        Some(Overlay::Text(s))
    }

    /// applyText parses the textarea and applies it (Go applyText). An error
    /// leaves the overlay open with the message.
    pub(super) fn apply_text(&mut self, s: &TextState) -> Result<(), String> {
        let text = s.ta.value();
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            match &s.path {
                None => return Err("cannot be empty".to_string()),
                Some(p) => {
                    self.fs_mut().doc.remove(p);
                    return Ok(());
                }
            }
        }
        if s.verbatim_str {
            if let Some(p) = &s.path {
                let n = self.fs_mut().doc.new_str(text);
                let _ = self.fs_mut().doc.set(p, n);
            }
            return Ok(());
        }
        let codec = self.fs().codec;
        let label = codec.label();
        match &s.path {
            // The whole-document editor goes through the document codec so
            // the freshly parsed format metadata (comments/directives) is
            // adopted with the new body.
            None => match codec.parse(&trimmed) {
                Err(e) => Err(format!("invalid {label}: {e}")),
                Ok(crate::doc::Parsed { doc, meta }) => {
                    let schema_less = self.fs().schema.schema_less;
                    let f = self.fs_mut();
                    f.doc = doc;
                    f.meta = meta;
                    if schema_less {
                        self.rebuild_schema_less_root();
                    }
                    Ok(())
                }
            },
            Some(p) => match codec.parse_value(&trimmed) {
                Err(e) => Err(format!("invalid {label}: {e}")),
                Ok(nd) => {
                    let n = self
                        .fs_mut()
                        .doc
                        .copy_into(&nd, nd.root, CopyMode::Preserve);
                    let _ = self.fs_mut().doc.set(p, n);
                    Ok(())
                }
            },
        }
    }

    /// keySel drives the enum option picker (Go keySel).
    pub fn key_sel(&mut self, msg: Msg, mut s: SelState) -> Option<Overlay> {
        if s.filter.active {
            return self.key_sel_search(msg, s);
        }
        let matches = sel_match_count(&s);
        if self.jump_keys(&msg, &mut s.cursor, matches) {
            return Some(Overlay::Sel(s));
        }
        match msg.key_str().as_str() {
            "/" => s.filter.open(s.cursor),
            "t" => {
                if let Some(one_of) = s.one_of {
                    let path = s.path.clone();
                    self.push(one_of, path);
                    return None;
                }
                return Some(Overlay::Sel(s));
            }
            "up" | "k" => s.cursor = wrap_cursor(s.cursor, -1, sel_match_count(&s)),
            "down" | "j" => s.cursor = wrap_cursor(s.cursor, 1, sel_match_count(&s)),
            "enter" => {
                if self.apply_sel(&s) {
                    return None;
                }
            }
            "esc" => return None,
            _ => {}
        }
        Some(Overlay::Sel(s))
    }

    /// keySelSearch consumes keys while the enum picker's filter is active.
    pub(super) fn key_sel_search(&mut self, msg: Msg, mut s: SelState) -> Option<Overlay> {
        let n = sel_match_count(&s);
        let total = s.opts.len();
        let page = self.overlay_page_size(0, true);
        if matches!(
            s.filter.key(&msg, &mut s.cursor, n, total, page),
            FilterAction::Commit
        ) {
            // no match: stay in the filter so backspace can undo it
            if self.apply_sel(&s) {
                return None;
            }
        }
        Some(Overlay::Sel(s))
    }

    /// applySel writes the option under the picker cursor. It reports
    /// whether a (matching) option was selected.
    pub(super) fn apply_sel(&mut self, s: &SelState) -> bool {
        let Some(opt) = sel_option_at(s, s.cursor) else {
            return false;
        };
        let val = opt.val;
        let path = s.path.clone();
        let fi = self.cur;
        let f = &mut self.files[fi];
        let n = f.doc.copy_into(&f.schema.doc, val, CopyMode::Preserve);
        let _ = f.doc.set(&path, n);
        true
    }

    /// keyEdit drives the in-place scalar input (Go keyEdit).
    pub fn key_edit(&mut self, msg: Msg, mut e: EditState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "enter" => {
                let node: FormNode = match (&e.fn_, &e.synth) {
                    (Some(_), _) | (_, Some(_)) => {
                        e.synth.clone().unwrap_or_else(|| FormNode::default())
                    }
                    _ => FormNode::default(),
                };
                // resolve the schema node when the edit targets one
                let node = match e.fn_ {
                    Some(id) => self.fs().schema.nodes[id as usize].clone(),
                    None => node,
                };
                let val = e.ti.value().to_string();
                let path = e.path.clone();
                let res = {
                    let f = &mut self.files[self.cur];
                    apply_scalar(&mut f.doc, &node, &f.schema.doc, &path, &val)
                };
                match res {
                    Err(err) => {
                        e.err = err;
                        return Some(Overlay::Edit(e));
                    }
                    Ok(()) => return None,
                }
            }
            "esc" => return None,
            "up" | "down" if !e.sugs.is_empty() => {
                cycle_text_sugs(
                    &mut e.ti,
                    &e.sugs,
                    &mut e.sug_idx,
                    if msg.key_str() == "up" { -1 } else { 1 },
                );
                return Some(Overlay::Edit(e));
            }
            "tab" | "ctrl+n" | "shift+tab" | "ctrl+p" => {
                if e.sugs.is_empty() {
                    return Some(Overlay::Edit(e));
                }
                // Same cycle as the arrows: -1 is "nothing selected", so
                // wrapCursor(-1, 1, n) starts at the first candidate.
                let delta = if matches!(msg.key_str().as_str(), "shift+tab" | "ctrl+p") {
                    -1
                } else {
                    1
                };
                cycle_text_sugs(&mut e.ti, &e.sugs, &mut e.sug_idx, delta);
                return Some(Overlay::Edit(e));
            }
            _ => {}
        }
        if let Some(k) = msg_key(&msg) {
            e.ti.handle_key(k);
        }
        self.overlay = Some(Overlay::Edit(e));
        self.refresh_edit_suggestions();
        // refresh_edit_suggestions mutates the overlay we just set in place;
        // hand it back to route_key (which assigns the return value).
        self.overlay.take()
    }

    /// keyAdd drives the key-name input (Go keyAdd).
    pub fn key_add(&mut self, msg: Msg, mut s: AddState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "enter" => {
                let k = s.ti.value().trim().to_string();
                if k.is_empty() {
                    s.err = "key required".to_string();
                    return Some(Overlay::Add(s));
                }
                let node = &self.fs().schema.nodes[s.node as usize].clone();
                for c in &node.children {
                    if self.fs().schema.nodes[*c as usize].key == k {
                        s.err = "key defined by the schema — edit its row instead".to_string();
                        return Some(Overlay::Add(s));
                    }
                }
                if let Some(cur) = self.doc().get(&s.path) {
                    if let Value::Object(mems) = self.doc().value(cur) {
                        for m in mems {
                            if m.key == k {
                                s.err = "key already exists".to_string();
                                return Some(Overlay::Add(s));
                            }
                        }
                    }
                }
                if node.kind == Kind::Map && !node.key_pattern.is_empty() {
                    match Regex::new(&node.key_pattern) {
                        Ok(re) if re.is_match(&k) => {}
                        _ => {
                            s.err = format!("key must match {}", node.key_pattern);
                            return Some(Overlay::Add(s));
                        }
                    }
                }
                let path = append_key(&s.path, &k);
                let idx = s.idx;
                // the key exists from here on: insert a null placeholder so
                // it keeps the requested position while the type is chosen
                let n = self.fs_mut().doc.new_null();
                let _ = self.fs_mut().doc.insert_member(&s.path, &k, idx, n);
                if node.kind == Kind::Map {
                    // the new value edits against the map's value schema
                    let items = node.items;
                    match items.map(|i| self.fs().schema.nodes[i as usize].kind) {
                        None => self.open_any_type(path),
                        Some(Kind::Raw) => {
                            self.open_field(items.expect("checked above"), path, true)
                        }
                        Some(Kind::OneOf) => {
                            let items = items.expect("checked above");
                            if dynamic_variants(&self.fs().schema, items).is_some() {
                                self.nav_forward();
                                let st = self.dynamic_menu(items, path);
                                self.fs_mut().stack.push(st);
                            } else {
                                self.push(items, path);
                            }
                        }
                        Some(Kind::Object) => {
                            let n = self.fs_mut().doc.new_object();
                            let _ = self.fs_mut().doc.set(&path, n);
                            self.push(items.expect("checked above"), path);
                        }
                        Some(Kind::Array) => {
                            let n = self.fs_mut().doc.new_array();
                            let _ = self.fs_mut().doc.set(&path, n);
                            self.push(items.expect("checked above"), path);
                        }
                        Some(Kind::Boolean) => {
                            let mut b = false;
                            if let Some(d) = node
                                .items
                                .and_then(|i| self.fs().schema.nodes[i as usize].default_)
                            {
                                if let Value::Bool(v) = self.fs().schema.doc.value(d) {
                                    b = *v;
                                }
                            }
                            let n = self.fs_mut().doc.new_bool(b);
                            let _ = self.fs_mut().doc.set(&path, n);
                        }
                        Some(_) => {
                            self.open_field(items.expect("checked above"), path, true);
                        }
                    }
                    return None;
                }
                // schema-less object member: choose the value type first
                self.open_any_type(path);
                None
            }
            "esc" => None,
            _ => {
                if let Some(k) = msg_key(&msg) {
                    s.ti.handle_key(k);
                }
                Some(Overlay::Add(s))
            }
        }
    }

    /// openShell arms the `!` command input at the bottom of the screen.
    pub(super) fn open_shell(&mut self) {
        let mut s = ShellState {
            ti: TextInput::new(),
            hist_idx: None,
            draft: String::new(),
            sug_prefix: String::new(),
            sug: Vec::new(),
            sug_idx: None,
            sug_line: String::new(),
        };
        shell_sug_refresh(&mut s);
        self.overlay = Some(Overlay::Shell(s));
    }

    /// keyShell drives the `!` line: enter runs the command and writes its
    /// output at the cursor row; up/down walk the session history. The prompt
    /// already shows one `!`, so enter on `!` (visible `!!`) or `!!` reruns
    /// the previous input; esc cancels.
    pub fn key_shell(&mut self, msg: Msg, mut s: ShellState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "enter" => {
                let cmd = s.ti.value().trim().to_string();
                if cmd.is_empty() {
                    return Some(Overlay::Shell(s));
                }
                // The prompt itself already shows one `!`: typing one more
                // (visible `!!`) or two (`!!!`) both mean "repeat the last
                // input" on enter, never a literal `!` shell command.
                if cmd == "!" || cmd == "!!" {
                    return self.shell_rerun_last();
                }
                self.shell_history.push(cmd.clone());
                if self.shell_history.len() > MAX_SHELL_HISTORY {
                    self.shell_history.remove(0);
                }
                self.execute_shell_input(&cmd);
                // a bare command's multi-line output installs its page here
                self.overlay.take()
            }
            "esc" => None,
            "up" => {
                if shell_completion_active(&s) {
                    shell_complete_step(&mut s, false);
                } else {
                    self.shell_history_up(&mut s);
                }
                Some(Overlay::Shell(s))
            }
            "down" => {
                if shell_completion_active(&s) {
                    shell_complete_step(&mut s, true);
                } else {
                    self.shell_history_down(&mut s);
                }
                Some(Overlay::Shell(s))
            }
            "tab" | "ctrl+n" => {
                shell_complete(&mut s, true);
                Some(Overlay::Shell(s))
            }
            "shift+tab" | "ctrl+p" => {
                shell_complete(&mut s, false);
                Some(Overlay::Shell(s))
            }
            _ => {
                if let Some(k) = msg_key(&msg) {
                    s.hist_idx = None;
                    s.ti.handle_key(k);
                    shell_sug_refresh(&mut s);
                }
                Some(Overlay::Shell(s))
            }
        }
    }

    /// shell_rerun_last repeats the most recent `!` input line, mode
    /// included (`r`/`a`/`w`/bare), like vi's `!!`.
    pub(super) fn shell_rerun_last(&mut self) -> Option<Overlay> {
        match self.shell_history.last().cloned() {
            Some(line) => {
                self.execute_shell_input(&line);
                self.overlay.take()
            }
            None => {
                self.notice = "! no previous command".to_string();
                None
            }
        }
    }

    /// shell_history_up recalls the previous command, remembering the draft
    /// that was being typed before the first recall.
    pub(super) fn shell_history_up(&self, s: &mut ShellState) {
        let len = self.shell_history.len();
        if len == 0 {
            return;
        }
        let idx = match s.hist_idx {
            None => {
                s.draft = s.ti.value().to_string();
                len - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        s.hist_idx = Some(idx);
        s.ti.set_value(self.shell_history[idx].clone());
        shell_sug_refresh(s);
    }

    /// shell_history_down moves toward the draft; stepping past the newest
    /// entry restores the original input.
    pub(super) fn shell_history_down(&self, s: &mut ShellState) {
        let Some(i) = s.hist_idx else { return };
        if i + 1 < self.shell_history.len() {
            s.hist_idx = Some(i + 1);
            s.ti.set_value(self.shell_history[i + 1].clone());
        } else {
            s.hist_idx = None;
            s.ti.set_value(s.draft.clone());
        }
        shell_sug_refresh(s);
    }

    /// execute_shell_input applies one `!` input line: `r <cmd>` inserts the
    /// command's stdout at the cursor value, `w <cmd>` streams the document
    /// to the command's stdin, and a bare command only runs (vi's `:!`).
    pub(super) fn execute_shell_input(&mut self, line: &str) {
        let (mode, cmd) = match shell::parse_input(line) {
            Ok(parsed) => parsed,
            Err(e) => {
                self.notice = format!("! {e}");
                return;
            }
        };
        match mode {
            shell::ShellMode::Read => self.shell_read(&cmd),
            shell::ShellMode::AppendLines => self.shell_append_lines(&cmd),
            shell::ShellMode::Write => self.shell_write(&cmd),
            shell::ShellMode::Execute => self.shell_execute_only(&cmd),
        }
    }

    /// shell_read is vi's `:r !cmd` scoped to the cursor value: the command's
    /// stdout replaces a string or appends to a string array.
    pub(super) fn shell_read(&mut self, cmd: &str) {
        let stdout = match shell::run_capture(cmd) {
            Ok(out) => out,
            Err(e) => {
                self.notice = format!("! {e}");
                return;
            }
        };
        let target = match self.shell_target() {
            Some(t) => t,
            None => {
                self.notice = "! cursor is not on a string or an array of strings".to_string();
                return;
            }
        };
        let output = shell::output_text(&stdout);
        if let Err(e) = shell::apply(&mut self.fs_mut().doc, &target, &output) {
            self.notice = format!("! {e}");
        }
    }

    /// shell_append_lines is the `a <cmd>` mode: split stdout into lines and
    /// append each one as a new item of the focused string array.
    pub(super) fn shell_append_lines(&mut self, cmd: &str) {
        let stdout = match shell::run_capture(cmd) {
            Ok(out) => out,
            Err(e) => {
                self.notice = format!("! {e}");
                return;
            }
        };
        let target = match self.shell_array_target() {
            Some(t) => t,
            None => {
                self.notice = "! cursor is not on a string array".to_string();
                return;
            }
        };
        let lines = shell::output_lines(&stdout);
        if let Err(e) = shell::apply_lines(&mut self.fs_mut().doc, &target, &lines) {
            self.notice = format!("! {e}");
        }
    }

    /// shell_write is vi's `:w !cmd`: pipe the whole live document (comments
    /// included, in its own syntax) to the command's stdin. The document is
    /// never modified; command output goes to the bottom notice.
    pub(super) fn shell_write(&mut self, cmd: &str) {
        let text = match self.fs().codec.write(self.doc(), &self.fs().meta) {
            Ok(text) => text,
            Err(e) => {
                self.notice = format!("! cannot serialize document: {e}");
                return;
            }
        };
        match shell::run_with_input(cmd, &text) {
            Ok(out) => {
                let shown = shell::compact_output(&out);
                self.notice = if shown.is_empty() {
                    format!("! piped {} bytes to command", text.len())
                } else {
                    shown
                };
            }
            Err(e) => self.notice = format!("! {e}"),
        }
    }

    /// shell_execute_only is vi's `:!cmd`: run the command and show its
    /// output without touching the document. A multi-line stdout opens the
    /// scrolling page; one line (or none) keeps the bottom notice.
    pub(super) fn shell_execute_only(&mut self, cmd: &str) {
        match shell::run_capture(cmd) {
            Ok(out) => {
                let text = shell::output_text(&out);
                if text.contains('\n') {
                    self.notice.clear();
                    self.overlay = Some(Overlay::ShellOut(ShellOutState {
                        command: cmd.to_string(),
                        lines: wrap_text(&text, self.width.saturating_sub(1)),
                        scroll: 0,
                    }));
                } else {
                    self.notice = shell::compact_output(&out);
                }
            }
            Err(e) => self.notice = format!("! {e}"),
        }
    }

    /// shell_target resolves the value slot under the cursor: a string row,
    /// an array-of-strings row (append), or the array of an empty item menu.
    pub(super) fn shell_target(&mut self) -> Option<ShellTarget> {
        let st = self.top().cloned()?;
        let rows = self.menu_rows_state(&st);
        let cursor = st.cursor.min(rows.len().saturating_sub(1));
        let r = rows.get(cursor)?;
        let (node, path) =
            if st.kind == MenuKind::Items && matches!(r.kind, RowKind::Add | RowKind::NoItems) {
                // an empty string array has only its append row: the cursor
                // still denotes the array value itself
                (Some(st.node), st.path.clone())
            } else if matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
                (r.fn_, r.path.clone())
            } else {
                return None;
            };
        let f = self.fs();
        shell::resolve_target(&f.schema, &f.doc, node, &path)
    }

    /// shell_array_target resolves the `a` mode slot. A row that is itself a
    /// string array wins; while browsing an item list, a scalar item falls
    /// back to the containing array, so appending works without leaving the
    /// array menu.
    pub(super) fn shell_array_target(&mut self) -> Option<ShellTarget> {
        let st = self.top().cloned()?;
        let rows = self.menu_rows_state(&st);
        let cursor = st.cursor.min(rows.len().saturating_sub(1));
        let r = rows.get(cursor)?;
        if matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            let f = self.fs();
            if let Some(t) = shell::resolve_array_target(&f.schema, &f.doc, r.fn_, &r.path) {
                return Some(t);
            }
        }
        if st.kind == MenuKind::Items {
            let f = self.fs();
            return shell::resolve_array_target(&f.schema, &f.doc, Some(st.node), &st.path);
        }
        None
    }

    /// keyWrite drives the w/W write-to-path input (Go keyWrite).
    pub fn key_write(&mut self, msg: Msg, mut w: WriteState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "enter" => {
                let path = w.ti.value().trim().to_string();
                if path.is_empty() {
                    w.err = "path required".to_string();
                    return Some(Overlay::Write(w));
                }
                if path == self.opts().path {
                    // unedited: a plain save of the session file
                    self.validate_and_write(false, "");
                } else {
                    self.validate_and_write(false, &path);
                }
                // validate_and_write may have opened the errors overlay;
                // keep whatever it installed (Go's m.errs survives).
                self.overlay.take()
            }
            "up" | "down" if !w.sugs.is_empty() => {
                cycle_text_sugs(
                    &mut w.ti,
                    &w.sugs,
                    &mut w.sug_idx,
                    if msg.key_str() == "up" { -1 } else { 1 },
                );
                return Some(Overlay::Write(w));
            }
            "tab" | "ctrl+n" | "shift+tab" | "ctrl+p" => {
                if w.sugs.is_empty() {
                    return Some(Overlay::Write(w));
                }
                let delta = if matches!(msg.key_str().as_str(), "shift+tab" | "ctrl+p") {
                    -1
                } else {
                    1
                };
                cycle_text_sugs(&mut w.ti, &w.sugs, &mut w.sug_idx, delta);
                Some(Overlay::Write(w))
            }
            "esc" => None,
            _ => {
                w.err = String::new();
                if let Some(k) = msg_key(&msg) {
                    w.ti.handle_key(k);
                }
                self.overlay = Some(Overlay::Write(w));
                self.refresh_write_suggestions();
                // refresh_write_suggestions edits the overlay in place;
                // hand it back so route_key's assignment keeps the input
                // open (Go returns the model with m.write still set).
                self.overlay.take()
            }
        }
    }

    /// refreshWriteSuggestions rebuilds the path candidates for the
    /// write-to value (Go refreshWriteSuggestions).
    pub fn refresh_write_suggestions(&mut self) {
        let Some(Overlay::Write(w)) = &mut self.overlay else {
            return;
        };
        w.sug_idx = -1;
        w.sugs.clear();
        let value = w.ti.value().to_string();
        w.sug_prefix = value.clone();
        let (dir, prefix) = match value.rfind('/') {
            Some(i) => (value[..=i].to_string(), value[i + 1..].to_string()),
            None => (String::new(), value.clone()),
        };
        let read = if dir.is_empty() {
            ".".to_string()
        } else {
            dir.clone()
        };
        let Ok(entries) = std::fs::read_dir(&read) else {
            return;
        };
        let lower = prefix.to_lowercase();
        for e in entries.flatten() {
            let mut name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && !prefix.starts_with('.') {
                continue;
            }
            if !name.to_lowercase().starts_with(&lower) {
                continue;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                || std::fs::metadata(e.path())
                    .map(|m| m.is_dir())
                    .unwrap_or(false);
            if is_dir {
                name.push('/');
            }
            w.sugs.push(format!("{}{}", dir, name));
        }
        // read_dir order is filesystem-dependent; tab cycling and the candidate
        // window need a stable order (shell.rs sorts its candidates the same way).
        w.sugs.sort_by_key(|s| s.to_lowercase());
    }

    /// keyAnchor drives the & / * name input (Go keyAnchor).
    pub fn key_anchor(&mut self, msg: Msg, mut s: AnchorState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "enter" => return self.anchor_commit(s),
            "esc" => return None,
            "up" | "down" => {
                if s.mode == AnchorMode::RefSet && !s.sugs.is_empty() {
                    self.cycle_anchor_sug(&mut s, if msg.key_str() == "up" { -1 } else { 1 });
                    return Some(Overlay::Anc(s));
                }
            }
            "tab" | "ctrl+n" => {
                self.cycle_anchor_sug(&mut s, 1);
                return Some(Overlay::Anc(s));
            }
            "shift+tab" | "ctrl+p" => {
                self.cycle_anchor_sug(&mut s, -1);
                return Some(Overlay::Anc(s));
            }
            _ => {}
        }
        s.sug = -1;
        if let Some(k) = msg_key(&msg) {
            s.ti.handle_key(k);
            s.sug_prefix = s.ti.value().to_string();
            s.sugs = Self::anchor_filtered(&s);
            s.sug_line.clear();
        }
        Some(Overlay::Anc(s))
    }

    /// keyMerge drives the merge-keys input (Go keyMerge).
    pub fn key_merge(&mut self, msg: Msg, mut s: MergeState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "enter" => return self.merge_commit(s),
            "esc" => return None,
            "tab" | "ctrl+n" => {
                self.cycle_merge_sug(&mut s, 1);
                return Some(Overlay::Mrg(s));
            }
            "shift+tab" | "ctrl+p" => {
                self.cycle_merge_sug(&mut s, -1);
                return Some(Overlay::Mrg(s));
            }
            _ => {}
        }
        s.sug = -1;
        if let Some(k) = msg_key(&msg) {
            s.ti.handle_key(k);
        }
        Some(Overlay::Mrg(s))
    }

    /// keySearch drives the scoped search overlay (Go keySearch).
    pub fn key_search(&mut self, msg: Msg, mut s: SearchState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "up" | "ctrl+k" => {
                s.cursor = wrap_cursor(s.cursor, -1, s.results.len());
                return Some(Overlay::Search(s));
            }
            "down" | "ctrl+j" => {
                s.cursor = wrap_cursor(s.cursor, 1, s.results.len());
                return Some(Overlay::Search(s));
            }
            "pgdown" | "ctrl+f" => {
                let page = self.search_page_size(&s);
                s.cursor = page_cursor(s.cursor, 1, page, s.results.len());
                return Some(Overlay::Search(s));
            }
            "pgup" | "ctrl+b" => {
                let page = self.search_page_size(&s);
                s.cursor = page_cursor(s.cursor, -1, page, s.results.len());
                return Some(Overlay::Search(s));
            }
            "tab" | "shift+tab" => {
                s.replace = true;
                s.replace_focused = !s.replace_focused;
                self.refilter(&mut s);
                return Some(Overlay::Search(s));
            }
            "alt+i" => {
                s.fold_case = !s.fold_case;
                self.refilter(&mut s);
                return Some(Overlay::Search(s));
            }
            "alt+x" => {
                s.regex = !s.regex;
                self.refilter(&mut s);
                return Some(Overlay::Search(s));
            }
            "ctrl+a" => {
                if s.replace {
                    self.replace_search_all(s);
                    // replace_search_all clears the overlay on a real
                    // replace and reinstalls it when there is nothing to
                    // do; keep that state (Go returns m unchanged).
                    return self.overlay.take();
                }
            }
            "enter" => {
                if s.replace && s.replace_focused {
                    self.replace_search_current(&mut s);
                    return Some(Overlay::Search(s));
                }
                if s.results.is_empty() {
                    return Some(Overlay::Search(s));
                }
                let res = s.results[s.cursor].clone();
                // park the overlay: esc at the landing reopens it
                s.parked = true;
                s.origin = Some(self.fs().stack.clone());
                self.fs_mut().issues_parked = None; // a fresh jump replaces a parked list
                self.fs_mut().jump_origin = None;
                self.fs_mut().jump_return = 0;
                self.nav_forward();
                // A variant key is a row in its parent menu: jumping to it
                // must stop there, not rebuild the oneOf picker underneath.
                let jump_path = {
                    let f = &self.files[self.cur];
                    match res.fn_ {
                        Some(fn_)
                            if f.schema.nodes[fn_ as usize].kind == Kind::OneOf
                                && matches!(res.kind, RowKind::Field | RowKind::Drill) =>
                        {
                            res.path[..res.path.len().saturating_sub(1)].to_vec()
                        }
                        _ => res.path.clone(),
                    }
                };
                let stack = {
                    let f = &mut self.files[self.cur];
                    rebuild_stack(&mut f.schema, &f.doc, &jump_path)
                };
                self.fs_mut().stack = stack;
                let rows = self.menu_rows();
                self.clamp_cursor(&rows);
                let mut target =
                    cursor_for_row(&rows, &res).or_else(|| self.enum_array_cursor(&rows, &res));
                // A variant picker is seeded on the value's variant; its rows
                // all sit at the picker path, so the ancestor fallback would
                // reset that cursor to row 0.
                if target.is_none() && self.top().map(|st| st.kind) != Some(MenuKind::OneOf) {
                    target = cursor_ancestor(&rows, &res.path);
                }
                if let Some(idx) = target {
                    if let Some(st) = self.top_mut() {
                        st.cursor = idx;
                        st.offset = 0;
                    }
                }
                s.return_depth = self.fs().stack.len();
                self.fs_mut().search_parked = Some(s);
                return None;
            }
            "esc" => {
                // replace mode is one level deeper: esc drops back to the
                // plain search list; the next esc closes the overlay.
                if s.replace {
                    s.replace = false;
                    s.replace_focused = false;
                    let cur = s.cursor;
                    self.refilter(&mut s);
                    s.cursor = cur.min(s.results.len().saturating_sub(1));
                    return Some(Overlay::Search(s));
                }
                self.close_search(s);
                return None;
            }
            _ => {}
        }
        if let Some(k) = msg_key(&msg) {
            if s.replace_focused {
                s.rti.handle_key(k);
            } else {
                s.ti.handle_key(k);
            }
        }
        self.refilter(&mut s);
        Some(Overlay::Search(s))
    }

    /// enum_array_cursor finds the toggle row a search hit points at: an
    /// enum-array menu has one toggle per schema option, so the document
    /// element's value picks the row (not the element's index).
    pub(super) fn enum_array_cursor(&self, rows: &[Row], res: &Row) -> Option<usize> {
        let st = self.top()?;
        if st.kind != MenuKind::EnumArray {
            return None;
        }
        let schema = &self.fs().schema;
        let items = schema.nodes[st.node as usize].items?;
        let doc = self.doc();
        let val = doc.get(&res.path)?;
        let oi = schema.nodes[items as usize]
            .enum_
            .iter()
            .position(|o| nodes_equal(&schema.doc, Some(o.val), doc, Some(val)))?;
        rows.iter()
            .position(|r| r.kind == RowKind::Toggle && r.idx == oi as i64)
    }

    /// search_page_size mirrors render_search_body's result area.
    pub(super) fn search_page_size(&self, s: &SearchState) -> usize {
        let inputs = if s.replace { 2 } else { 1 };
        self.height
            .saturating_sub(2 + inputs + self.status_lines().len() + 1)
            .max(1)
    }

    // --- external $EDITOR ---------------------------------------------------

    /// editRawExternal writes the whole document to a temp file and arms
    /// $EDITOR (Go editRawExternal). Used by E at the root.
    pub(super) fn edit_raw_external(&mut self) {
        let codec = self.fs().codec;
        match codec.write(self.doc(), &self.fs().meta) {
            Err(e) => self.notice = format!("editor: {e}"),
            Ok(data) => {
                self.launch_external_edit(data, None, true, -1);
            }
        }
    }

    /// editValueExternal edits one value in $EDITOR (Go editValueExternal).
    pub(super) fn edit_value_external(&mut self, path: Vec<PathSeg>, insert_idx: i64) {
        let mut buf = String::new();
        if let Some(cur) = self.doc().get(&path) {
            if !is_null_val(self.doc(), cur) {
                let codec = self.fs().codec;
                match codec.write_value(self.doc(), cur) {
                    Err(e) => {
                        self.notice = format!("editor: {e}");
                        return;
                    }
                    Ok(s) => buf = s,
                }
            }
        }
        self.launch_external_edit(buf, Some(path), false, insert_idx);
    }

    /// run_pending_editor runs the armed $EDITOR and applies its output (Go
    /// applyExternalEdit + the exec path). In tests (exec_editor=false) the
    /// pending edit is stashed so `finish_external_value` can deliver the
    /// content instead.
    pub fn run_pending_editor(&mut self) {
        if !self.exec_editor {
            return;
        }
        let Some(edit) = self.pending_edit.clone() else {
            return;
        };
        self.pending_edit = None;
        let editor = std::env::var("EDITOR")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("VISUAL").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "vi".to_string());
        // sh -c keeps EDITOR values with arguments ("code --wait") working
        // while the temp path is passed as a quoted positional argument.
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!("{} \"$1\"", editor))
            .arg("sh")
            .arg(&edit.path);
        let edit_path = edit.path.clone();
        // suspend the terminal for the editor
        {
            let mut stdout = std::io::stdout();
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = crossterm::execute!(
                stdout,
                crossterm::cursor::Show,
                crossterm::event::DisableBracketedPaste,
                crossterm::terminal::LeaveAlternateScreen
            );
        }
        let status = cmd.status();
        {
            let mut stdout = std::io::stdout();
            let _ = crossterm::terminal::enable_raw_mode();
            let _ = crossterm::execute!(
                stdout,
                crossterm::terminal::EnterAlternateScreen,
                crossterm::event::EnableBracketedPaste,
                crossterm::cursor::Hide
            );
        }
        match status {
            Err(e) => self.notice = format!("editor: {e}"),
            Ok(st) if !st.success() => self.notice = format!("editor: exited with {st}"),
            Ok(_) => {
                if let Ok(data) = std::fs::read(&edit.path) {
                    self.apply_external(edit, &data);
                } else {
                    self.notice = "editor: temp file unreadable".to_string();
                }
            }
        }
        let _ = std::fs::remove_file(&edit_path);
    }

    /// editExternalJSON writes data to a temp file and arms the editor.
    pub(super) fn launch_external_edit(
        &mut self,
        data: String,
        target: Option<Vec<PathSeg>>,
        whole_doc: bool,
        insert_idx: i64,
    ) {
        let ext = self.fs().codec.raw_ext();
        // uniquify: pid + a counter would race across processes; use nanos
        let path = std::env::temp_dir().join(format!(
            "jse-{}-{}{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            ext
        ));
        if let Err(e) = std::fs::write(&path, data) {
            self.notice = format!("editor: {e}");
            return;
        }
        let edit = ExternalEdit {
            target: target.unwrap_or_default(),
            insert_idx,
            path: path.to_string_lossy().to_string(),
            whole_doc,
        };
        self.pending_edit = Some(edit);
    }

    /// rebuildSchemaLessRoot re-derives the runtime root after a whole-document
    /// raw edit swapped the root type (object ↔ array ↔ scalar): the
    /// schema-less model is a snapshot of the loaded shape, so a stale root
    /// would offer the wrong menu. Synth nodes and the stack are rebuilt
    /// against the new model.
    pub(super) fn rebuild_schema_less_root(&mut self) {
        let f = self.fs_mut();
        f.schema = crate::smodel::untyped(&f.doc);
        let root = f.schema.root;
        let kind = crate::ui::rows::menu_kind_for(&f.schema, root);
        f.menu_cache.clear();
        f.stack.truncate(1);
        if let Some(st) = f.stack.first_mut() {
            st.kind = kind;
            st.node = root;
            st.path.clear();
            st.cursor = 0;
            st.offset = 0;
            st.variant_of = None;
            st.one_of = None;
            st.sel.clear();
            st.visual = false;
            st.vanchor = 0;
            st.filter.ti = Default::default();
            st.filter.active = false;
            st.filter.restore = 0;
        }
    }

    /// applyExternalEdit parses the editor's temp file and applies it (Go
    /// applyExternalEdit / applyExternalValue).
    pub(super) fn apply_external(&mut self, edit: ExternalEdit, data: &[u8]) {
        if edit.whole_doc {
            let pre = self.doc().clone();
            let codec = self.fs().codec;
            let label = codec.label();
            match codec.parse(&String::from_utf8_lossy(data)) {
                Err(e) => {
                    self.notice = format!("editor: invalid {}: {}", label, e);
                }
                Ok(crate::doc::Parsed { doc: nd, meta }) => {
                    if nodes_equal_doc(&pre, &nd) {
                        self.notice = format!("Raw {} unchanged", label);
                    } else {
                        let f = self.fs_mut();
                        f.doc = nd;
                        f.meta = meta;
                        f.history.push_undo(pre);
                        if self.fs().schema.schema_less {
                            self.rebuild_schema_less_root();
                        }
                        self.refresh_variant_menu();
                        self.refresh_issues();
                        let rows = self.menu_rows();
                        self.clamp_cursor(&rows);
                        self.notice = format!("Raw {} edited", label);
                    }
                }
            }
            return;
        }
        // one value
        let s = String::from_utf8_lossy(data).trim().to_string();
        let pre = self.doc().clone();
        if s.is_empty() {
            if edit.insert_idx < 0 {
                self.fs_mut().doc.remove(&edit.target);
            }
            if nodes_equal_doc(&pre, &self.fs().doc) {
                self.notice = "Value unchanged".to_string();
                return;
            }
            self.fs_mut().history.push_undo(pre);
            self.after_external_value_edit();
            self.notice = "Value removed".to_string();
            return;
        }
        let codec = self.fs().codec;
        let label = codec.label();
        match codec.parse_value(&s) {
            Err(e) => {
                self.notice = format!("editor: invalid {}: {}", label, e);
            }
            Ok(nd) => {
                let n = nd.root;
                if edit.insert_idx >= 0 {
                    // positioned add (a/A): splice the new member at its index
                    let parent = edit.target[..edit.target.len() - 1].to_vec();
                    let Some(PathSeg::Key(key)) = edit.target.last() else {
                        return;
                    };
                    let val = self.fs_mut().doc.copy_into(&nd, n, CopyMode::Preserve);
                    let _ = self.fs_mut().doc.insert_member(
                        &parent,
                        key,
                        edit.insert_idx as usize,
                        val,
                    );
                } else {
                    let val = self.fs_mut().doc.copy_into(&nd, n, CopyMode::Preserve);
                    let _ = self.fs_mut().doc.set(&edit.target, val);
                }
                if nodes_equal_doc(&pre, &self.fs().doc) {
                    self.notice = "Value unchanged".to_string();
                    return;
                }
                self.fs_mut().history.push_undo(pre);
                self.after_external_value_edit();
                self.notice = "Value edited".to_string();
            }
        }
    }

    pub(super) fn after_external_value_edit(&mut self) {
        self.refresh_variant_menu();
        self.refresh_issues();
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
    }

    /// The test path of the external editor: write content to the pending
    /// temp file and apply it like the editor would (Go tests'
    /// finishExternalValue + Update(rawEditorDoneMsg)).
    #[cfg(test)]
    pub fn finish_external_value(&mut self, content: &str) {
        let Some(edit) = self.pending_edit.clone() else {
            panic!("no pending external edit");
        };
        self.pending_edit = None;
        self.apply_external(edit, content.as_bytes());
    }

    /// The editor run failed (test path: the rawEditorDoneMsg error case).
    #[cfg(test)]
    pub fn external_edit_failed(&mut self, err: &str) {
        self.pending_edit = None;
        self.notice = format!("editor: {err}");
    }
}
