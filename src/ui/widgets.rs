#![allow(dead_code)] // wired into the menu in later phases
//! Text-input widget layer: handwritten equivalents of the charmbracelet
//! bubbles `textinput`/`textarea` models scoped to the operations the menu
//! uses, plus the shared scalar-validation semantics of Go `widgets.go`
//! (ValidateInput / ShouldOmit / omitOrSet / applyScalar) and the raw-editor
//! codec helpers of Go `raw.go`.

use crate::doc::jsondoc::{self, Doc, DocError, Member, NodeData, NodeId, PathSeg, Value};
use crate::smodel::FormNode;
use crate::ui::{NodeRef, RawCodec};

// --- raw editors (Go raw.go) ----------------------------------------------------

/// Raw-editor text syntax helpers: JSON by default, the file's own syntax for
/// YAML and TOML documents. Those codecs supply their own marshal/unmarshal
/// callbacks because only they know the live format session (comments
/// included).
pub trait RawExt {
    /// rawLabel: the syntax name shown in prompts ("JSON" by default).
    fn raw_label(&self) -> String;
    /// rawExt: temp-file extension (".json" by default).
    fn raw_ext(&self) -> String;
    /// encodeRawDoc writes the whole live document.
    fn encode_raw_doc(&self, doc: &Doc) -> Result<String, String>;
    /// decodeRawDoc parses a whole document (AdoptDoc > UnmarshalDoc >
    /// jsondoc::parse).
    fn decode_raw_doc(&self, data: &str) -> Result<Doc, String>;
    /// encodeRawValue writes one subtree.
    fn encode_raw_value(&self, doc: &Doc, id: NodeId) -> Result<String, String>;
    /// decodeRawValue parses one subtree (UnmarshalValue > jsondoc::parse).
    fn decode_raw_value(&self, data: &str) -> Result<Doc, String>;
}

impl RawExt for RawCodec {
    fn raw_label(&self) -> String {
        if !self.label.is_empty() {
            return self.label.to_string();
        }
        "JSON".to_string()
    }

    fn raw_ext(&self) -> String {
        if !self.ext.is_empty() {
            return self.ext.to_string();
        }
        ".json".to_string()
    }

    fn encode_raw_doc(&self, doc: &Doc) -> Result<String, String> {
        (self.marshal_doc)(doc)
    }

    fn decode_raw_doc(&self, data: &str) -> Result<Doc, String> {
        if let Some(adopt) = &self.adopt_doc {
            return adopt(data).map(|(doc, _meta)| doc);
        }
        (self.unmarshal_doc)(data)
    }

    fn encode_raw_value(&self, doc: &Doc, id: NodeId) -> Result<String, String> {
        (self.marshal_value)(NodeRef { doc, id })
    }

    fn decode_raw_value(&self, data: &str) -> Result<Doc, String> {
        (self.unmarshal_value)(data)
    }
}

// --- scalar validation (Go widgets.go) ------------------------------------------

/// shouldOmit implements the empty-input omission rule: an empty value for a
/// non-required field without a default removes the key.
pub fn should_omit(empty: bool, required: bool, has_default: bool) -> bool {
    empty && !required && !has_default
}

/// fieldDesc renders the field description: schema desc plus extra suffixes.
pub fn field_desc(fn_node: &FormNode, extra: &[&str]) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(3);
    if !fn_node.desc.is_empty() {
        parts.push(fn_node.desc.as_str());
    }
    parts.extend_from_slice(extra);
    parts.join(" ")
}

/// numStr formats a float like Go strconv.FormatFloat(f, 'g', -1, 64).
pub fn num_str(f: f64) -> String {
    jsondoc::go_format_g(f)
}

/// ValidateInput validates a raw input string against scalar constraints.
/// It is the validation closure used by every text input field.
pub fn validate_input(fn_node: &FormNode, s: &str) -> Result<(), String> {
    if s.is_empty() {
        if fn_node.required {
            return Err("required".to_string());
        }
        return Ok(());
    }
    match fn_node.json_type.as_str() {
        "integer" | "number" => {
            let v: f64 = s.parse().map_err(|_| "must be a number".to_string())?;
            if fn_node.json_type == "integer" && v != v.trunc() {
                return Err("must be an integer".to_string());
            }
            if let Some(min) = fn_node.min {
                if fn_node.exclusive_min && v <= min {
                    return Err(format!("must be greater than {}", num_str(min)));
                }
                if !fn_node.exclusive_min && v < min {
                    return Err(format!("must be at least {}", num_str(min)));
                }
            }
            if let Some(max) = fn_node.max {
                if fn_node.exclusive_max && v >= max {
                    return Err(format!("must be less than {}", num_str(max)));
                }
                if !fn_node.exclusive_max && v > max {
                    return Err(format!("must be at most {}", num_str(max)));
                }
            }
            if let Some(m) = fn_node.multiple_of {
                if m != 0.0 {
                    let q = v / m;
                    if (q - q.round()).abs() > 1e-9 {
                        return Err(format!("must be a multiple of {}", num_str(m)));
                    }
                }
            }
        }
        _ => {
            // string
            let n = s.chars().count();
            if let Some(min_len) = fn_node.min_len {
                if n < min_len {
                    return Err(format!("minimum length is {}", min_len));
                }
            }
            if let Some(max_len) = fn_node.max_len {
                if n > max_len {
                    return Err(format!("maximum length is {}", max_len));
                }
            }
            if !fn_node.pattern.is_empty() {
                if let Ok(re) = regex::Regex::new(&fn_node.pattern) {
                    if !re.is_match(s) {
                        return Err(format!("must match {}", fn_node.pattern));
                    }
                }
            }
        }
    }
    Ok(())
}

/// scalarText renders a current/default scalar node as input text.
pub fn scalar_text(doc: &Doc, id: Option<NodeId>) -> String {
    let id = match id {
        Some(id) => id,
        None => return String::new(),
    };
    match doc.value(id) {
        Value::Str(s) => s.clone(),
        Value::Num(lit) => lit.clone(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        _ => String::new(),
    }
}

/// copyInto deep-copies a node from `src` (the schema document enum/default
/// values live in) into `doc`, returning the new id. Go hands the schema's
/// `*jsondoc.Node` to doc.Set directly; the arena model needs an explicit
/// copy. Anchors and merge refs ride along.
fn copy_into(doc: &mut Doc, src: &Doc, id: NodeId) -> NodeId {
    let data = src.node(id);
    let value = match &data.value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|m| Member {
                    key: m.key.clone(),
                    val: copy_into(doc, src, m.val),
                })
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.iter().map(|i| copy_into(doc, src, *i)).collect())
        }
        v => v.clone(),
    };
    let merge = data.merge.iter().map(|m| copy_into(doc, src, *m)).collect();
    doc.push(NodeData {
        value,
        anchor: data.anchor.clone(),
        merge,
        raw: data.raw.clone(),
    })
}

/// omitOrSet applies the empty-input omission rule and sets the value.
/// An empty input removes the key only when it holds a value: a null
/// placeholder (a key seeded by a, a null in the source JSON) stays null —
/// there is nothing to omit, and clearing must not delete the key the user
/// just created (d removes it explicitly).
///
/// `default_doc` is the document the schema node's default value points into
/// (Go dereferences fn.Default directly; the arena model copies it across).
pub fn omit_or_set(
    doc: &mut Doc,
    path: &[PathSeg],
    empty: bool,
    fn_node: &FormNode,
    default_doc: &Doc,
    val: impl FnOnce(&mut Doc) -> NodeId,
) -> Result<(), DocError> {
    if empty {
        if should_omit(empty, fn_node.required, fn_node.default_.is_some()) {
            if let Some(cur) = doc.get(path) {
                if matches!(doc.value(cur), Value::Null) {
                    return Ok(());
                }
            }
            doc.remove(path);
            return Ok(());
        }
        if let Some(dflt) = fn_node.default_ {
            let v = copy_into(doc, default_doc, dflt);
            return doc.set(path, v);
        }
        doc.remove(path);
        return Ok(());
    }
    let v = val(doc);
    doc.set(path, v)
}

/// applyScalar validates a raw scalar input and writes it at path under the
/// empty-input omission rule shared with the menu TUI. It returns the
/// validation error without touching the doc.
pub fn apply_scalar(
    doc: &mut Doc,
    fn_node: &FormNode,
    default_doc: &Doc,
    path: &[PathSeg],
    s: &str,
) -> Result<(), String> {
    validate_input(fn_node, s)?;
    let json_type = fn_node.json_type.clone();
    let text = s.to_string();
    omit_or_set(doc, path, s.is_empty(), fn_node, default_doc, |doc| {
        if json_type == "integer" || json_type == "number" {
            doc.new_num(text)
        } else {
            doc.new_str(text)
        }
    })
    .map_err(|e| e.msg)
}

// --- text input widgets ----------------------------------------------------------

/// Key is the reduced key event the menu layer feeds the widgets, covering
/// exactly the operations the Go code exercises on bubbles textinput /
/// textarea (menu_keys.go).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    CtrlA,
    CtrlE,
    CtrlK,
    CtrlU,
    CtrlW,
    /// readline-style yank of the last ctrl+k/u/w kill
    CtrlY,
    /// alt+f: end of the current/next word (shell forward-word)
    WordForward,
    /// alt+b: start of the current/previous word (shell backward-word)
    WordBackward,
}

/// is_word_delim reports whether `c` separates words for alt+f/alt+b motion
/// and ctrl+w kills: whitespace plus the punctuation common in config/code.
fn is_word_delim(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            ',' | '.'
                | '+'
                | '-'
                | '='
                | '_'
                | '/'
                | '\\'
                | ':'
                | '?'
                | '!'
                | '$'
                | '#'
                | '@'
                | '*'
        )
}

/// TextInput is a one-line editable buffer with a cursor and tab-completion
/// support — the bubbles textinput operations menu_keys.go uses: SetValue /
/// Value / CursorEnd, character insert, backspace/delete, left/right,
/// home/end (ctrl+a/ctrl+e), ctrl+k/ctrl+u line kills, ctrl+w word kill, and
/// ctrl+y yank of the last kill.
#[derive(Clone, Debug)]
pub struct TextInput {
    value: String,
    /// Byte offset into `value`, always on a char boundary.
    cursor: usize,
    placeholder: String,
    suggestions: Vec<String>,
    /// Completion cycle position; None until the first complete_* call.
    sug_idx: Option<usize>,
    /// Last text removed by ctrl+k/u/w, pasted back by ctrl+y.
    kill: String,
}

impl Default for TextInput {
    fn default() -> TextInput {
        TextInput {
            value: String::new(),
            cursor: 0,
            placeholder: String::new(),
            suggestions: Vec::new(),
            sug_idx: None,
            kill: String::new(),
        }
    }
}

impl TextInput {
    pub fn new() -> TextInput {
        TextInput::default()
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// Cursor position as a character (rune) index, for rendering.
    pub fn cursor(&self) -> usize {
        self.value[..self.cursor].chars().count()
    }

    pub fn set_placeholder(&mut self, p: impl Into<String>) {
        self.placeholder = p.into();
    }

    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    /// SetValue: replaces the buffer and moves the cursor to the end.
    pub fn set_value(&mut self, v: impl Into<String>) {
        self.value = v.into();
        self.cursor_end();
    }

    pub fn cursor_end(&mut self) {
        self.cursor = self.value.len();
    }

    /// Inserts text at the cursor (typed characters and pasted text).
    pub fn insert_str(&mut self, s: &str) {
        self.value.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn set_suggestions(&mut self, sugs: Vec<String>) {
        self.suggestions = sugs;
        self.sug_idx = None;
    }

    /// complete_next advances the tab cycle (None → first → … → last →
    /// first); each accepted suggestion replaces the value with the cursor
    /// at the end, exactly like the menu layer's SetValue+CursorEnd.
    pub fn complete_next(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        let n = self.suggestions.len();
        let idx = match self.sug_idx {
            None => 0,
            Some(i) => (i + 1) % n,
        };
        self.sug_idx = Some(idx);
        let sug = self.suggestions[idx].clone();
        self.set_value(sug);
    }

    /// complete_prev moves the cycle backward (None → last), wrapping.
    pub fn complete_prev(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        let n = self.suggestions.len();
        let idx = match self.sug_idx {
            None => n - 1,
            Some(i) => (i + n - 1) % n,
        };
        self.sug_idx = Some(idx);
        let sug = self.suggestions[idx].clone();
        self.set_value(sug);
    }

    /// The suggestion the cycle currently points at, if any.
    pub fn suggestion(&self) -> Option<&str> {
        self.sug_idx.map(|i| self.suggestions[i].as_str())
    }

    /// Byte offset of the char boundary before `cursor` (None at position 0).
    fn prev_boundary(&self) -> Option<usize> {
        self.value[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
    }

    /// Byte offset of the char boundary after `cursor` (None at the end).
    fn next_boundary(&self) -> Option<usize> {
        self.value[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
    }

    /// save_kill stores text removed by a kill key for ctrl+y; an empty kill
    /// keeps the previous buffer (readline keeps the last non-empty kill).
    fn save_kill(&mut self, killed: String) {
        if !killed.is_empty() {
            self.kill = killed;
        }
    }

    /// deleteWordBackward: bubbles ctrl+w — swallow the delimiter run, then
    /// the word run before the cursor.
    fn delete_word_backward(&mut self) {
        let mut cut = self.cursor;
        while let Some(i) = self.prev_boundary_at(cut) {
            if !is_word_delim(self.value[i..cut].chars().next().unwrap()) {
                break;
            }
            cut = i;
        }
        while let Some(i) = self.prev_boundary_at(cut) {
            if is_word_delim(self.value[i..cut].chars().next().unwrap()) {
                break;
            }
            cut = i;
        }
        // bubbles lands back on the delimiter run and steps right once, so
        // trailing delimiters survive a word kill
        if cut > 0 {
            if let Some(c) = self.value[cut..].chars().next() {
                if is_word_delim(c) {
                    cut += c.len_utf8();
                }
            }
        }
        self.save_kill(self.value[cut..self.cursor].to_string());
        self.value.replace_range(cut..self.cursor, "");
        self.cursor = cut;
    }

    fn prev_boundary_at(&self, pos: usize) -> Option<usize> {
        self.value[..pos].char_indices().next_back().map(|(i, _)| i)
    }

    /// word_forward moves to the end of the current word, or skips the
    /// delimiter run and the following word (alt+f).
    fn word_forward(&mut self) {
        let mut end = self.cursor;
        let mut in_word = false;
        for (i, c) in self.value[self.cursor..].char_indices() {
            let next = self.cursor + i + c.len_utf8();
            if is_word_delim(c) {
                if in_word {
                    break;
                }
            } else {
                in_word = true;
            }
            end = next;
        }
        self.cursor = end;
    }

    /// word_backward moves to the start of the current word, or skips the
    /// delimiter run and the preceding word (alt+b).
    fn word_backward(&mut self) {
        let mut start = self.cursor;
        let mut in_word = false;
        for (i, c) in self.value[..self.cursor].char_indices().rev() {
            if is_word_delim(c) {
                if in_word {
                    break;
                }
            } else {
                in_word = true;
            }
            start = i;
        }
        self.cursor = start;
    }

    pub fn handle_key(&mut self, key: Key) {
        match key {
            Key::Char(c) => {
                self.value.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            Key::Backspace => {
                if let Some(i) = self.prev_boundary() {
                    self.value.replace_range(i..self.cursor, "");
                    self.cursor = i;
                }
            }
            Key::Delete => {
                if let Some(i) = self.next_boundary() {
                    self.value.replace_range(self.cursor..i, "");
                }
            }
            Key::Left => {
                if let Some(i) = self.prev_boundary() {
                    self.cursor = i;
                }
            }
            Key::Right => {
                if let Some(i) = self.next_boundary() {
                    self.cursor = i;
                }
            }
            Key::Home | Key::CtrlA => self.cursor = 0,
            Key::End | Key::CtrlE => self.cursor_end(),
            Key::CtrlK => {
                self.save_kill(self.value[self.cursor..].to_string());
                self.value.truncate(self.cursor);
            }
            Key::CtrlU => {
                self.save_kill(self.value[..self.cursor].to_string());
                self.value.replace_range(0..self.cursor, "");
                self.cursor = 0;
            }
            Key::CtrlW => self.delete_word_backward(),
            Key::CtrlY => {
                if !self.kill.is_empty() {
                    let killed = self.kill.clone();
                    self.insert_str(&killed);
                }
            }
            Key::WordForward => self.word_forward(),
            Key::WordBackward => self.word_backward(),
            Key::Enter | Key::Up | Key::Down => {}
        }
    }
}

/// TextArea is a multi-line editable buffer with a row/column cursor — the
/// bubbles textarea operations menu_keys.go uses: SetValue / Value, character
/// and newline insert, backspace/delete, up/down/left/right (left/right wrap
/// across line boundaries), line home/end, ctrl+k (kill to end of line),
/// ctrl+u (kill from line start), ctrl+w (delete word backward).
/// Width/height are stored for the renderer only.
pub struct TextArea {
    lines: Vec<String>,
    row: usize,
    /// Byte offset into `lines[row]`, always on a char boundary.
    col: usize,
    width: u16,
    height: u16,
    /// Last text removed by ctrl+k/u/w on the current line, yanked by ctrl+y.
    kill: String,
}

impl Default for TextArea {
    fn default() -> TextArea {
        TextArea {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            width: 0,
            height: 0,
            kill: String::new(),
        }
    }
}

impl TextArea {
    pub fn new() -> TextArea {
        TextArea::default()
    }

    /// Value joins the lines with '\n' (a trailing newline in the input is
    /// a final empty line, matching bubbles).
    pub fn value(&self) -> String {
        self.lines.join("\n")
    }

    /// SetValue replaces the buffer and resets the cursor to (0, 0).
    pub fn set_value(&mut self, v: &str) {
        self.lines = v.split('\n').map(str::to_string).collect();
        self.row = 0;
        self.col = 0;
    }

    pub fn set_width(&mut self, w: u16) {
        self.width = w;
    }

    pub fn set_height(&mut self, h: u16) {
        self.height = h;
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.row_col_char())
    }

    /// window returns the first visible line and the visible line count.
    /// The window follows the cursor row, mirroring bubbles' textarea
    /// viewport: a tall buffer renders only its height, not every line.
    pub fn window(&self) -> (usize, usize) {
        let h = (self.height as usize).max(1);
        let top = self.row.saturating_add(1).saturating_sub(h);
        (top.min(self.row), h)
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, i: usize) -> Option<&str> {
        self.lines.get(i).map(String::as_str)
    }

    fn row_col_char(&self) -> usize {
        self.lines[self.row][..self.col].chars().count()
    }

    fn line_len(&self) -> usize {
        self.lines[self.row].len()
    }

    /// Inserts text at the cursor on the current line.
    pub fn insert_str(&mut self, s: &str) {
        self.lines[self.row].insert_str(self.col, s);
        self.col += s.len();
    }

    pub fn insert_char(&mut self, c: char) {
        self.insert_str(&c.to_string());
    }

    /// Inserts pasted text: newlines split the current line instead of
    /// becoming literal characters in it (bracketed paste of a document must
    /// stay multi-line). CRLF and lone CR normalize to LF.
    pub fn insert_paste(&mut self, s: &str) {
        let text = s.replace("\r\n", "\n").replace('\r', "\n");
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                self.insert_newline();
            }
            self.insert_str(line);
        }
    }

    /// InsertNewline splits the current line at the cursor.
    pub fn insert_newline(&mut self) {
        let rest = self.lines[self.row].split_off(self.col);
        self.row += 1;
        self.col = 0;
        self.lines.insert(self.row, rest);
    }

    fn prev_boundary(&self) -> Option<usize> {
        self.lines[self.row][..self.col]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.lines[self.row][self.col..]
            .chars()
            .next()
            .map(|c| self.col + c.len_utf8())
    }

    /// save_kill stores text removed by a kill key for ctrl+y; an empty kill
    /// keeps the previous buffer (readline keeps the last non-empty kill).
    fn save_kill(&mut self, killed: String) {
        if !killed.is_empty() {
            self.kill = killed;
        }
    }

    /// DeleteWordBackward on the current line (bubbles ctrl+w is line-scoped).
    fn delete_word_backward(&mut self) {
        let line = &self.lines[self.row];
        let mut cut = self.col;
        let delim =
            |pos: usize| -> bool { is_word_delim(line[..pos].chars().next_back().unwrap()) };
        while cut > 0 && delim(cut) {
            cut = line[..cut]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap();
        }
        while cut > 0 && !delim(cut) {
            cut = line[..cut]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap();
        }
        // bubbles lands back on the delimiter run and steps right once, so
        // trailing delimiters survive a word kill
        if cut > 0 {
            if let Some(c) = self.lines[self.row][cut..].chars().next() {
                if is_word_delim(c) {
                    cut += c.len_utf8();
                }
            }
        }
        self.save_kill(self.lines[self.row][cut..self.col].to_string());
        self.lines[self.row].replace_range(cut..self.col, "");
        self.col = cut;
    }

    /// word_forward moves to the end of the current/next word on this line
    /// (alt+f); the cursor stops at the line end.
    fn word_forward(&mut self) {
        let line = &self.lines[self.row];
        let mut end = self.col;
        let mut in_word = false;
        for (i, c) in line[self.col..].char_indices() {
            let next = self.col + i + c.len_utf8();
            if is_word_delim(c) {
                if in_word {
                    break;
                }
            } else {
                in_word = true;
            }
            end = next;
        }
        self.col = end;
    }

    /// word_backward moves to the start of the current/previous word on this
    /// line (alt+b); the cursor stops at the line start.
    fn word_backward(&mut self) {
        let line = &self.lines[self.row];
        let mut start = self.col;
        let mut in_word = false;
        for (i, c) in line[..self.col].char_indices().rev() {
            if is_word_delim(c) {
                if in_word {
                    break;
                }
            } else {
                in_word = true;
            }
            start = i;
        }
        self.col = start;
    }

    pub fn handle_key(&mut self, key: Key) {
        match key {
            Key::Char(c) => self.insert_char(c),
            Key::Enter => self.insert_newline(),
            Key::Backspace => {
                if let Some(i) = self.prev_boundary() {
                    self.lines[self.row].replace_range(i..self.col, "");
                    self.col = i;
                } else if self.row > 0 {
                    // join with the previous line
                    let line = self.lines.remove(self.row);
                    self.row -= 1;
                    self.col = self.line_len();
                    self.lines[self.row].push_str(&line);
                }
            }
            Key::Delete => {
                if let Some(i) = self.next_boundary() {
                    self.lines[self.row].replace_range(self.col..i, "");
                } else if self.row + 1 < self.lines.len() {
                    // join the next line up
                    let next = self.lines.remove(self.row + 1);
                    self.lines[self.row].push_str(&next);
                }
            }
            Key::Left => {
                if let Some(i) = self.prev_boundary() {
                    self.col = i;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.line_len();
                }
            }
            Key::Right => {
                if let Some(i) = self.next_boundary() {
                    self.col = i;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
            }
            Key::Up => {
                if self.row > 0 {
                    self.row -= 1;
                    self.col = self.col.min(self.line_len());
                }
            }
            Key::Down => {
                if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = self.col.min(self.line_len());
                }
            }
            Key::Home | Key::CtrlA => self.col = 0,
            Key::End | Key::CtrlE => self.col = self.line_len(),
            Key::CtrlK => {
                self.save_kill(self.lines[self.row][self.col..].to_string());
                self.lines[self.row].truncate(self.col);
            }
            Key::CtrlU => {
                self.save_kill(self.lines[self.row][..self.col].to_string());
                self.lines[self.row].replace_range(0..self.col, "");
                self.col = 0;
            }
            Key::CtrlW => self.delete_word_backward(),
            Key::CtrlY => {
                if !self.kill.is_empty() {
                    let killed = self.kill.clone();
                    self.insert_str(&killed);
                }
            }
            Key::WordForward => self.word_forward(),
            Key::WordBackward => self.word_backward(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::jsondoc::parse;
    use std::sync::Arc;

    fn seg(path: &[&str]) -> Vec<PathSeg> {
        path.iter().map(|p| PathSeg::key(*p)).collect()
    }

    fn fn_json_type(t: &str) -> FormNode {
        FormNode {
            json_type: t.to_string(),
            ..Default::default()
        }
    }

    // --- ValidateInput (Go TestValidateInputNumbers / TestValidateInputStrings)

    #[test]
    fn validate_input_numbers() {
        let cases: Vec<(FormNode, &str, &str)> = vec![
            (fn_json_type("integer"), "42", ""),
            (fn_json_type("integer"), "4.5", "must be an integer"),
            (fn_json_type("number"), "abc", "must be a number"),
            (
                FormNode {
                    min: Some(1024.0),
                    ..fn_json_type("integer")
                },
                "1024",
                "",
            ),
            (
                FormNode {
                    min: Some(1024.0),
                    ..fn_json_type("integer")
                },
                "80",
                "must be at least 1024",
            ),
            (
                FormNode {
                    min: Some(1.0),
                    exclusive_min: true,
                    ..fn_json_type("number")
                },
                "1",
                "must be greater than 1",
            ),
            (
                FormNode {
                    max: Some(10.0),
                    ..fn_json_type("number")
                },
                "11",
                "must be at most 10",
            ),
            (
                FormNode {
                    max: Some(10.0),
                    exclusive_max: true,
                    ..fn_json_type("number")
                },
                "10",
                "must be less than 10",
            ),
            (
                FormNode {
                    multiple_of: Some(5.0),
                    ..fn_json_type("integer")
                },
                "10",
                "",
            ),
            (
                FormNode {
                    multiple_of: Some(5.0),
                    ..fn_json_type("integer")
                },
                "7",
                "must be a multiple of 5",
            ),
            (
                FormNode {
                    required: true,
                    ..fn_json_type("string")
                },
                "",
                "required",
            ),
            (fn_json_type("string"), "", ""),
        ];
        for (fnode, input, want) in cases {
            let got = validate_input(&fnode, input).err().unwrap_or_default();
            assert!(
                got == want || got.contains(want),
                "{input}: got {got:?}, want {want:?}"
            );
        }
    }

    #[test]
    fn validate_input_strings() {
        let cases: Vec<(FormNode, &str, &str)> = vec![
            (
                FormNode {
                    min_len: Some(2),
                    ..fn_json_type("string")
                },
                "a",
                "minimum length is 2",
            ),
            (
                FormNode {
                    max_len: Some(3),
                    ..fn_json_type("string")
                },
                "abcd",
                "maximum length is 3",
            ),
            (
                FormNode {
                    pattern: "^[a-z]+$".to_string(),
                    ..fn_json_type("string")
                },
                "abc",
                "",
            ),
            (
                FormNode {
                    pattern: "^[a-z]+$".to_string(),
                    ..fn_json_type("string")
                },
                "ABC",
                "must match ^[a-z]+$",
            ),
            (
                FormNode {
                    pattern: "([unclosed".to_string(),
                    ..fn_json_type("string")
                },
                "any",
                "",
            ),
            (
                FormNode {
                    max_len: Some(3),
                    ..fn_json_type("string")
                },
                "名字字",
                "",
            ),
        ];
        for (fnode, input, want) in cases {
            let got = validate_input(&fnode, input).err().unwrap_or_default();
            assert!(
                got == want || got.contains(want),
                "{input}: got {got:?}, want {want:?}"
            );
        }
    }

    #[test]
    fn validate_input_num_str_go_format() {
        assert_eq!(num_str(1024.0), "1024");
        assert_eq!(num_str(1000000.0), "1e+06");
        assert_eq!(num_str(1.5), "1.5");
    }

    // --- ShouldOmit (Go TestScalarValidation)

    #[test]
    fn should_omit_cases() {
        let cases = [
            (true, false, false, true),
            (true, true, false, false),
            (true, false, true, false),
            (false, false, false, false),
        ];
        for (empty, required, has_default, want) in cases {
            assert_eq!(should_omit(empty, required, has_default), want);
        }
    }

    // --- omitOrSet (Go TestOmitOrSetSemantics)

    #[test]
    fn omit_or_set_semantics() {
        let mut doc = parse(r#"{"keep": 1, "drop": "x"}"#).unwrap();
        let empty = FormNode::default();
        let mut schema_doc = Doc::new();

        // empty optional without default → key removed
        let str_val = |doc: &mut Doc| doc.new_str("v");
        omit_or_set(
            &mut doc,
            &seg(&["drop"]),
            true,
            &empty,
            &schema_doc,
            str_val,
        )
        .unwrap();
        assert!(doc.get(&seg(&["drop"])).is_none(), "drop should be removed");

        // empty optional with default → default written
        let dflt = schema_doc.new_str("dflt");
        let fnode = FormNode {
            default_: Some(dflt),
            ..Default::default()
        };
        omit_or_set(
            &mut doc,
            &seg(&["keep"]),
            true,
            &fnode,
            &schema_doc,
            str_val,
        )
        .unwrap();
        let got = doc.get(&seg(&["keep"])).unwrap();
        assert_eq!(doc.value(got), &Value::Str("dflt".to_string()));

        // non-empty → value written
        omit_or_set(
            &mut doc,
            &seg(&["new"]),
            false,
            &empty,
            &schema_doc,
            str_val,
        )
        .unwrap();
        let got = doc.get(&seg(&["new"])).unwrap();
        assert_eq!(doc.value(got), &Value::Str("v".to_string()));
    }

    #[test]
    fn omit_or_set_null_placeholder_stays() {
        // a null placeholder (a key seeded by a) must survive an empty submit
        let mut doc = parse(r#"{"seed": null}"#).unwrap();
        let empty = FormNode::default();
        let schema_doc = Doc::new();
        omit_or_set(&mut doc, &seg(&["seed"]), true, &empty, &schema_doc, |d| {
            d.new_str("v")
        })
        .unwrap();
        let got = doc.get(&seg(&["seed"])).unwrap();
        assert_eq!(doc.value(got), &Value::Null);
    }

    // --- scalarText (Go TestScalarText)

    #[test]
    fn scalar_text_cases() {
        let mut doc = Doc::new();
        let s = doc.new_str("s");
        let n = doc.new_num("1.5");
        let b = doc.new_bool(true);
        assert_eq!(scalar_text(&doc, Some(s)), "s");
        assert_eq!(scalar_text(&doc, Some(n)), "1.5");
        assert_eq!(scalar_text(&doc, Some(b)), "true");
        assert_eq!(scalar_text(&doc, None), "");
        let nul = doc.new_null();
        assert_eq!(scalar_text(&doc, Some(nul)), "");
    }

    // --- applyScalar

    #[test]
    fn apply_scalar_writes_numbers_and_strings() {
        let mut doc = Doc::new();
        let schema_doc = Doc::new();
        let int_fn = fn_json_type("integer");

        apply_scalar(&mut doc, &int_fn, &schema_doc, &[], "42").unwrap();
        assert_eq!(doc.value(doc.root), &Value::Num("42".to_string()));

        let str_fn = fn_json_type("string");
        apply_scalar(&mut doc, &str_fn, &schema_doc, &[], "hi").unwrap();
        assert_eq!(doc.value(doc.root), &Value::Str("hi".to_string()));
    }

    #[test]
    fn apply_scalar_validation_error_does_not_touch_doc() {
        let mut doc = parse(r#"{"p": 1}"#).unwrap();
        let schema_doc = Doc::new();
        let int_fn = fn_json_type("integer");
        let err = apply_scalar(&mut doc, &int_fn, &schema_doc, &seg(&["p"]), "abc").unwrap_err();
        assert_eq!(err, "must be a number");
        assert_eq!(
            doc.value(doc.get(&seg(&["p"])).unwrap()),
            &Value::Num("1".to_string())
        );
    }

    // --- raw.go helpers

    #[test]
    fn raw_defaults_are_json() {
        let codec = RawCodec {
            label: "",
            ext: "",
            marshal_doc: Arc::new(|doc| Ok(doc.write())),
            unmarshal_doc: Arc::new(|data| jsondoc::parse(data).map_err(|e| e.msg)),
            adopt_doc: None,
            marshal_value: Arc::new(|r: NodeRef<'_>| Ok(r.doc.write_subtree(r.id))),
            unmarshal_value: Arc::new(|data| jsondoc::parse(data).map_err(|e| e.msg)),
        };
        assert_eq!(codec.raw_label(), "JSON");
        assert_eq!(codec.raw_ext(), ".json");

        let doc = jsondoc::parse(r#"{"k": "v"}"#).unwrap();
        assert_eq!(
            codec.encode_raw_doc(&doc).unwrap(),
            "{\n  \"k\": \"v\"\n}\n"
        );
        let k = doc.get(&seg(&["k"])).unwrap();
        assert_eq!(codec.encode_raw_value(&doc, k).unwrap(), "\"v\"\n");

        let back = codec.decode_raw_doc(r#"{"a": 1}"#).unwrap();
        assert_eq!(
            back.value(back.root),
            &Value::Object(vec![Member {
                key: "a".to_string(),
                val: back.get(&seg(&["a"])).unwrap(),
            }])
        );

        let val = codec.decode_raw_value("\"x\"\n").unwrap();
        assert_eq!(val.value(val.root), &Value::Str("x".to_string()));
        assert!(codec.decode_raw_value("{bad").is_err());
    }

    #[test]
    fn raw_yaml_codec_overrides() {
        let codec = RawCodec {
            label: "YAML",
            ext: ".yaml",
            // stand-in YAML codec: identity-ish text round trip through JSON
            marshal_doc: Arc::new(|doc| Ok(doc.write())),
            unmarshal_doc: Arc::new(|data| jsondoc::parse(data).map_err(|e| e.msg)),
            adopt_doc: Some(Arc::new(|data: &str| {
                jsondoc::parse(data)
                    .map_err(|e| e.msg)
                    .map(|d| (d, crate::doc::Meta::default()))
            })),
            marshal_value: Arc::new(|r: NodeRef<'_>| Ok(r.doc.write_subtree(r.id))),
            unmarshal_value: Arc::new(|data| jsondoc::parse(data).map_err(|e| e.msg)),
        };
        assert_eq!(codec.raw_label(), "YAML");
        assert_eq!(codec.raw_ext(), ".yaml");
        // AdoptDoc supersedes UnmarshalDoc
        let doc = codec.decode_raw_doc(r#"{"a": 1}"#).unwrap();
        assert_eq!(
            doc.value(doc.get(&seg(&["a"])).unwrap()),
            &Value::Num("1".to_string())
        );
    }

    #[test]
    fn raw_encode_value_subtree() {
        let codec = RawCodec {
            label: "",
            ext: "",
            marshal_doc: Arc::new(|doc| Ok(doc.write())),
            unmarshal_doc: Arc::new(|data| jsondoc::parse(data).map_err(|e| e.msg)),
            adopt_doc: None,
            marshal_value: Arc::new(|r: NodeRef<'_>| Ok(r.doc.write_subtree(r.id))),
            unmarshal_value: Arc::new(|data| jsondoc::parse(data).map_err(|e| e.msg)),
        };
        let doc = jsondoc::parse(r#"{"outer": {"in": [1, 2]}}"#).unwrap();
        let outer = doc.get(&seg(&["outer"])).unwrap();
        assert_eq!(
            codec.encode_raw_value(&doc, outer).unwrap(),
            "{\n  \"in\": [\n    1,\n    2\n  ]\n}\n"
        );
    }

    // --- TextInput

    #[test]
    fn text_input_typing_and_cursor() {
        let mut ti = TextInput::new();
        ti.insert_str("hello");
        assert_eq!(ti.value(), "hello");
        assert_eq!(ti.cursor(), 5);
        ti.handle_key(Key::Left);
        ti.handle_key(Key::Left);
        assert_eq!(ti.cursor(), 3);
        ti.handle_key(Key::Char('X'));
        assert_eq!(ti.value(), "helXlo");
        assert_eq!(ti.cursor(), 4);
        ti.handle_key(Key::Backspace);
        assert_eq!(ti.value(), "hello");
        assert_eq!(ti.cursor(), 3);
        ti.handle_key(Key::Delete);
        assert_eq!(ti.value(), "helo");
        ti.handle_key(Key::Home);
        assert_eq!(ti.cursor(), 0);
        ti.handle_key(Key::End);
        assert_eq!(ti.cursor(), 4);
    }

    #[test]
    fn text_input_unicode_cursor() {
        let mut ti = TextInput::new();
        ti.insert_str("名字字");
        assert_eq!(ti.cursor(), 3);
        ti.handle_key(Key::Left);
        assert_eq!(ti.cursor(), 2);
        ti.handle_key(Key::Backspace);
        assert_eq!(ti.value(), "名字");
        ti.handle_key(Key::Home);
        ti.handle_key(Key::Delete);
        assert_eq!(ti.value(), "字");
    }

    #[test]
    fn text_input_line_kills_and_word_kill() {
        let mut ti = TextInput::new();
        ti.set_value("one two three");
        ti.handle_key(Key::CtrlA);
        ti.handle_key(Key::CtrlK);
        assert_eq!(ti.value(), "");

        let mut ti = TextInput::new();
        ti.set_value("one two three");
        ti.handle_key(Key::CtrlE);
        ti.handle_key(Key::CtrlU);
        assert_eq!(ti.value(), "");

        let mut ti = TextInput::new();
        ti.set_value("foo bar   baz");
        ti.handle_key(Key::CtrlW); // bubbles deletes the word, leaving the whitespace run
        assert_eq!(ti.value(), "foo bar   ");
        ti.handle_key(Key::CtrlW); // deletes "bar   " — the word plus preceding spaces
        assert_eq!(ti.value(), "foo ");
        ti.handle_key(Key::CtrlW);
        assert_eq!(ti.value(), "");

        // punctuation is a delimiter for word kills too
        let mut ti = TextInput::new();
        ti.set_value("foo.bar baz");
        ti.handle_key(Key::CtrlW);
        assert_eq!(ti.value(), "foo.bar ");
        ti.handle_key(Key::CtrlW);
        assert_eq!(ti.value(), "foo.");
        ti.handle_key(Key::CtrlW);
        assert_eq!(ti.value(), "");

        // edge cases on an empty/zero-length buffer
        let mut ti = TextInput::new();
        ti.handle_key(Key::Backspace);
        ti.handle_key(Key::Delete);
        ti.handle_key(Key::CtrlW);
        assert_eq!(ti.value(), "");
    }

    #[test]
    fn text_input_kill_then_yank_roundtrips() {
        let mut ti = TextInput::new();
        ti.set_value("echo hello");
        ti.handle_key(Key::CtrlU);
        assert_eq!(ti.value(), "");
        ti.handle_key(Key::CtrlY);
        assert_eq!(ti.value(), "echo hello");

        // ctrl+w replaces the kill buffer with the word it removed
        ti.handle_key(Key::CtrlW);
        assert_eq!(ti.value(), "echo ");
        ti.handle_key(Key::CtrlY);
        assert_eq!(ti.value(), "echo hello");

        // ctrl+k kills to the end and ctrl+y restores it
        ti.handle_key(Key::CtrlA);
        ti.handle_key(Key::CtrlK);
        assert_eq!(ti.value(), "");
        ti.handle_key(Key::CtrlY);
        assert_eq!(ti.value(), "echo hello");

        // empty kills leave the previous buffer intact
        let mut ti = TextInput::new();
        ti.set_value("keep");
        ti.handle_key(Key::CtrlU);
        ti.set_value("");
        ti.handle_key(Key::CtrlU);
        ti.handle_key(Key::CtrlY);
        assert_eq!(ti.value(), "keep");
    }

    #[test]
    fn text_input_word_motion() {
        let mut ti = TextInput::new();
        ti.set_value("one two three");
        ti.handle_key(Key::WordBackward); // end of "three" -> its start
        assert_eq!(ti.cursor(), 8);
        ti.handle_key(Key::WordBackward);
        assert_eq!(ti.cursor(), 4);
        ti.handle_key(Key::WordForward); // start of "two" -> its end
        assert_eq!(ti.cursor(), 7);
        ti.handle_key(Key::WordForward);
        assert_eq!(ti.cursor(), 13);
        ti.handle_key(Key::WordForward); // already at the end
        assert_eq!(ti.cursor(), 13);

        // whitespace runs and unicode stay on char boundaries
        let mut ti = TextInput::new();
        ti.set_value("foo   bar 世界");
        ti.handle_key(Key::WordBackward);
        assert_eq!(ti.cursor(), 10);
        ti.handle_key(Key::WordBackward);
        assert_eq!(ti.cursor(), 6);
        ti.handle_key(Key::WordForward);
        assert_eq!(ti.cursor(), 9);
        ti.handle_key(Key::WordForward);
        assert_eq!(ti.cursor(), "foo   bar 世界".chars().count());

        // code punctuation splits words as well
        let mut ti = TextInput::new();
        ti.set_value("foo.bar_baz qux");
        ti.handle_key(Key::WordBackward);
        assert_eq!(ti.cursor(), 12);
        ti.handle_key(Key::WordBackward);
        assert_eq!(ti.cursor(), 8);
        ti.handle_key(Key::WordBackward);
        assert_eq!(ti.cursor(), 4);
        ti.handle_key(Key::WordForward);
        assert_eq!(ti.cursor(), 7);
        ti.handle_key(Key::WordForward);
        assert_eq!(ti.cursor(), 11);
    }

    #[test]
    fn text_input_set_value_cursor_end() {
        let mut ti = TextInput::new();
        ti.set_value("abc");
        assert_eq!(ti.cursor(), 3);
        ti.handle_key(Key::Char('d'));
        assert_eq!(ti.value(), "abcd");
    }

    #[test]
    fn text_input_completion_cycle() {
        let mut ti = TextInput::new();
        ti.set_suggestions(vec!["alpha".into(), "beta".into(), "gamma".into()]);
        ti.insert_str("be");
        ti.complete_next();
        assert_eq!(ti.value(), "alpha");
        assert_eq!(ti.cursor(), 5);
        assert_eq!(ti.suggestion(), Some("alpha"));
        ti.complete_next();
        assert_eq!(ti.value(), "beta");
        ti.complete_next();
        ti.complete_next(); // wraps to alpha
        assert_eq!(ti.value(), "alpha");
        ti.complete_prev();
        assert_eq!(ti.value(), "gamma");
        // a fresh cycle from None starts at the last suggestion
        ti.set_suggestions(vec!["x".into(), "y".into()]);
        ti.complete_prev();
        assert_eq!(ti.value(), "y");
        ti.set_suggestions(vec![]); // reset + empty: no-op
        ti.set_value("");
        ti.insert_str("keep");
        ti.complete_next();
        ti.complete_prev();
        assert_eq!(ti.value(), "keep");
    }

    #[test]
    fn text_input_placeholder() {
        let mut ti = TextInput::new();
        ti.set_placeholder("port");
        assert_eq!(ti.placeholder(), "port");
        assert_eq!(ti.value(), "");
    }

    // --- TextArea

    #[test]
    fn text_area_typing_and_newlines() {
        let mut ta = TextArea::new();
        ta.insert_str("ab");
        ta.handle_key(Key::Enter);
        ta.insert_str("cd");
        assert_eq!(ta.value(), "ab\ncd");
        assert_eq!(ta.cursor(), (1, 2));
    }

    #[test]
    fn text_area_word_motion_is_line_scoped() {
        let mut ta = TextArea::new();
        ta.set_value("one two\nthree four");
        ta.handle_key(Key::Down);
        ta.handle_key(Key::End);
        ta.handle_key(Key::WordBackward);
        assert_eq!(ta.cursor(), (1, 6));
        ta.handle_key(Key::WordBackward);
        assert_eq!(ta.cursor(), (1, 0));
        ta.handle_key(Key::WordBackward); // stops at the line start
        assert_eq!(ta.cursor(), (1, 0));
        ta.handle_key(Key::WordForward);
        assert_eq!(ta.cursor(), (1, 5));
        ta.handle_key(Key::WordForward);
        assert_eq!(ta.cursor(), (1, 10));
    }

    #[test]
    fn text_area_backspace_delete() {
        let mut ta = TextArea::new();
        ta.set_value("ab\ncd");
        ta.handle_key(Key::Down);
        ta.handle_key(Key::End); // end of line 1
        ta.handle_key(Key::Backspace); // deletes 'd'
        assert_eq!(ta.value(), "ab\nc");
        ta.handle_key(Key::Backspace); // deletes 'c'
        assert_eq!(ta.value(), "ab\n");
        ta.handle_key(Key::Backspace); // at col 0: joins into the previous line
        assert_eq!(ta.value(), "ab");
        assert_eq!(ta.cursor(), (0, 2));
        ta.handle_key(Key::Delete); // at end of the last line: no-op
        assert_eq!(ta.value(), "ab");
    }

    #[test]
    fn text_area_backspace_at_origin_noop() {
        let mut ta = TextArea::new();
        ta.set_value("x");
        ta.handle_key(Key::Home);
        ta.handle_key(Key::Backspace);
        assert_eq!(ta.value(), "x");
        assert_eq!(ta.cursor(), (0, 0));
    }

    #[test]
    fn text_area_motion() {
        let mut ta = TextArea::new();
        ta.set_value("one\ntwo\nthree");
        ta.handle_key(Key::Down);
        ta.handle_key(Key::Down);
        assert_eq!(ta.cursor(), (2, 0));
        ta.handle_key(Key::Up);
        assert_eq!(ta.cursor(), (1, 0));
        ta.handle_key(Key::End);
        assert_eq!(ta.cursor(), (1, 3));
        ta.handle_key(Key::Right); // wraps to the next line start
        assert_eq!(ta.cursor(), (2, 0));
        ta.handle_key(Key::Right);
        assert_eq!(ta.cursor(), (2, 1));
        ta.handle_key(Key::Left); // within the line
        assert_eq!(ta.cursor(), (2, 0));
        ta.handle_key(Key::Left); // wraps to the previous line end
        assert_eq!(ta.cursor(), (1, 3));
        ta.handle_key(Key::Left);
        assert_eq!(ta.cursor(), (1, 2));
        // up/down clamp the column to the line length
        ta.handle_key(Key::End);
        ta.handle_key(Key::Down); // line 2 is longer, column stays 3
        assert_eq!(ta.cursor(), (2, 3));
        ta.handle_key(Key::End);
        ta.handle_key(Key::Up); // line 1 is shorter: column clamps to 3
        assert_eq!(ta.cursor(), (1, 3));
        // motion at the buffer edges stays put
        ta.set_value("a\nb");
        ta.handle_key(Key::Up);
        assert_eq!(ta.cursor(), (0, 0));
        ta.handle_key(Key::End);
        ta.handle_key(Key::Down); // line 1 is shorter: column clamps to 1
        assert_eq!(ta.cursor(), (1, 1));
    }

    #[test]
    fn text_area_kill_then_yank_roundtrips() {
        let mut ta = TextArea::new();
        ta.set_value("alpha beta\ngamma");
        ta.handle_key(Key::End);
        ta.handle_key(Key::CtrlU);
        assert_eq!(ta.value(), "\ngamma");
        ta.handle_key(Key::CtrlY);
        assert_eq!(ta.value(), "alpha beta\ngamma");

        ta.handle_key(Key::CtrlW);
        assert_eq!(ta.value(), "alpha \ngamma");
        ta.handle_key(Key::CtrlY);
        assert_eq!(ta.value(), "alpha beta\ngamma");
    }

    #[test]
    fn text_area_home_end_ctrl_kills() {
        let mut ta = TextArea::new();
        ta.set_value("alpha beta\ngamma");
        ta.handle_key(Key::End); // end of line 0: ctrl+k there is a no-op
        ta.handle_key(Key::CtrlK);
        assert_eq!(ta.value(), "alpha beta\ngamma");
        ta.handle_key(Key::Left);
        ta.handle_key(Key::Left);
        ta.handle_key(Key::Left);
        ta.handle_key(Key::Left); // col 6, before "beta"
        ta.handle_key(Key::CtrlK); // kill to end of line from mid-line
        assert_eq!(ta.value(), "alpha \ngamma");
        ta.handle_key(Key::CtrlA); // line home
        ta.insert_str("H");
        assert_eq!(ta.value(), "Halpha \ngamma");
        ta.handle_key(Key::CtrlU); // kill from line start to cursor
        assert_eq!(ta.value(), "alpha \ngamma");
        // ctrl+w deletes the word before the cursor on the line (preceding
        // whitespace goes with it; the trailing run stays)
        let mut ta = TextArea::new();
        ta.set_value("foo bar baz");
        ta.handle_key(Key::End);
        ta.handle_key(Key::CtrlW);
        assert_eq!(ta.value(), "foo bar ");
        ta.handle_key(Key::CtrlW);
        assert_eq!(ta.value(), "foo ");
    }

    #[test]
    fn text_area_set_value_and_dimensions() {
        let mut ta = TextArea::new();
        ta.set_width(80);
        ta.set_height(24);
        assert_eq!((ta.width(), ta.height()), (80, 24));
        ta.set_value("a\n\nb\n"); // trailing newline keeps a final empty line
        assert_eq!(ta.value(), "a\n\nb\n");
        assert_eq!(ta.cursor(), (0, 0));
        ta.set_value(""); // empty value is one empty line
        assert_eq!(ta.value(), "");
    }

    #[test]
    fn text_area_unicode_columns() {
        let mut ta = TextArea::new();
        ta.set_value("名字");
        ta.handle_key(Key::End);
        assert_eq!(ta.cursor(), (0, 2));
        ta.handle_key(Key::Left);
        assert_eq!(ta.cursor(), (0, 1));
        ta.handle_key(Key::Backspace);
        assert_eq!(ta.value(), "字");
    }
}

#[cfg(test)]
mod ta_window_tests {
    use super::TextArea;

    #[test]
    fn window_follows_the_cursor_row() {
        let mut ta = TextArea::new();
        ta.set_value(
            &(0..100)
                .map(|i| format!("l{i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        ta.set_height(10);
        ta.set_width(40);
        assert_eq!(ta.window(), (0, 10), "cursor at row 0 shows the first page");
        for _ in 0..15 {
            ta.insert_newline();
        }
        assert_eq!(ta.window(), (6, 10), "page follows the cursor row");
        for _ in 0..90 {
            ta.insert_newline();
        }
        assert_eq!(
            ta.window(),
            (96, 10),
            "the page follows the cursor to the tail"
        );
    }
}
