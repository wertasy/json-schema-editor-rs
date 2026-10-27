use super::*;

// --- navigation stack -------------------------------------------------------

/// rebuildStack reconstructs the navigation stack from the root down to the
/// menu that contains the row for path (leaf segments are not pushed: the
/// row lives in its parent menu).
pub fn rebuild_stack(schema: &mut Schema, doc: &Doc, path: &[PathSeg]) -> Vec<MenuState> {
    fn state(kind: MenuKind, node: FormNodeId, path: Vec<PathSeg>) -> MenuState {
        MenuState {
            kind,
            node,
            path,
            cursor: 0,
            offset: 0,
            variant_of: None,
            one_of: None,
            sel: HashSet::new(),
            visual: false,
            vanchor: 0,
            filter: Default::default(),
        }
    }
    let root = schema.root;
    let mut synth: HashMap<SynthKey, FormNodeId> = HashMap::new();
    let mut stack = vec![state(menu_kind_for(schema, root), root, Vec::new())];
    let mut cur = root;
    let mut cur_path: Vec<PathSeg> = Vec::new();

    /// raw_runtime reports whether Enter on this raw row opens the runtime
    /// editor (any-type menus) rather than the raw textarea.
    fn raw_runtime(schema: &Schema, f: FormNodeId) -> bool {
        let n = &schema.nodes[f as usize];
        n.kind == Kind::Raw && !n.null_only && !n.raw_editor
    }

    /// runtime_descend pushes the schema-less Object/Array editor for the
    /// document value at path — the menu Enter would open for an unknown
    /// key, a raw leaf, or an array item without a schema type.
    fn runtime_descend(
        schema: &mut Schema,
        synth: &mut HashMap<SynthKey, FormNodeId>,
        doc: &Doc,
        stack: &mut Vec<MenuState>,
        path: &[PathSeg],
    ) -> Option<FormNodeId> {
        let want = match doc.get(path).map(|v| doc.value(v)) {
            Some(Value::Object(_)) => Kind::Object,
            Some(Value::Array(_)) => Kind::Array,
            _ => return None,
        };
        let any = any_type_menu(schema, synth);
        let node = schema.nodes[any as usize]
            .variants
            .iter()
            .copied()
            .find(|&v| {
                let n = &schema.nodes[v as usize];
                n.key.is_empty() && n.kind == want
            })?;
        let kind = menu_kind_for(schema, node);
        stack.push(state(kind, node, path.to_vec()));
        Some(node)
    }

    for seg in path {
        match seg {
            PathSeg::Key(s) => {
                if schema.nodes[cur as usize].kind == Kind::OneOf {
                    let v = doc.get(&cur_path);
                    let idx = current_variant(schema, cur, doc, v);
                    if idx < 0 {
                        return stack;
                    }
                    let variant = schema.nodes[cur as usize].variants[idx as usize];
                    if menu_leaf(schema, variant) {
                        // A raw variant still opens the runtime editor for a
                        // container value; scalars edit in place.
                        if !raw_runtime(schema, variant) {
                            return stack;
                        }
                        match runtime_descend(schema, &mut synth, doc, &mut stack, &cur_path) {
                            Some(node) => cur = node,
                            None => return stack,
                        }
                    } else {
                        let kind = menu_kind_for(schema, variant);
                        let mut st = state(kind, variant, cur_path.clone());
                        st.one_of = Some(cur);
                        stack.push(st);
                        cur = variant;
                    }
                }
                let node_kind = schema.nodes[cur as usize].kind;
                let cpath = append_key(&cur_path, s);
                let child = if node_kind == Kind::Object {
                    schema.nodes[cur as usize]
                        .children
                        .iter()
                        .copied()
                        .find(|c| schema.nodes[*c as usize].key == *s)
                } else if node_kind == Kind::Map {
                    // any doc key resolves against the map's value schema
                    let items = schema.nodes[cur as usize].items;
                    match (items, doc.get(&cpath)) {
                        (Some(items), Some(_)) => Some(items),
                        _ => None,
                    }
                } else {
                    None
                };
                let Some(child) = child else {
                    // A document key with no schema node (schema-less file or
                    // a key the schema does not define): Enter on its row
                    // opens the runtime editor, so the jump descends the same
                    // way through objects and arrays.
                    if node_kind == Kind::Object {
                        if let Some(node) =
                            runtime_descend(schema, &mut synth, doc, &mut stack, &cpath)
                        {
                            cur = node;
                            cur_path = cpath;
                            continue;
                        }
                        if doc.get(&cpath).is_some() {
                            return stack; // scalar row lives in this menu
                        }
                    }
                    continue; // extra key without schema: skip the segment
                };
                if schema.nodes[child as usize].kind == Kind::OneOf {
                    // an existing value edits its variant directly; the
                    // picker stays one t away
                    if let Some(v) = doc.get(&cpath) {
                        if !is_null(doc, v) {
                            let vi = current_variant(schema, child, doc, Some(v));
                            if vi >= 0 {
                                let variant = schema.nodes[child as usize].variants[vi as usize];
                                if !menu_leaf(schema, variant) {
                                    let kind = menu_kind_for(schema, variant);
                                    let mut st = state(kind, variant, cpath.clone());
                                    st.one_of = Some(child);
                                    stack.push(st);
                                    cur = variant;
                                    cur_path = cpath;
                                    continue;
                                }
                                // A raw leaf variant opens the runtime editor
                                // for a container value; other leaf variants
                                // keep their row in the current menu.
                                if raw_runtime(schema, variant) {
                                    if let Some(node) =
                                        runtime_descend(schema, &mut synth, doc, &mut stack, &cpath)
                                    {
                                        cur = node;
                                        cur_path = cpath;
                                        continue;
                                    }
                                }
                                return stack;
                            }
                        }
                    }
                }
                if menu_leaf(schema, child) {
                    if raw_runtime(schema, child) {
                        if let Some(node) =
                            runtime_descend(schema, &mut synth, doc, &mut stack, &cpath)
                        {
                            cur = node;
                            cur_path = cpath;
                            continue;
                        }
                    }
                    return stack; // leaf row lives in the current top menu
                }
                let kind = menu_kind_for(schema, child);
                stack.push(state(kind, child, cpath.clone()));
                cur = child;
                cur_path = cpath;
            }
            PathSeg::Idx(s) => {
                let ipath = append_idx(&cur_path, *s);
                let items = match schema.nodes[cur as usize].kind {
                    Kind::Array => schema.nodes[cur as usize].items,
                    _ => None,
                };
                let Some(items) = items else {
                    // No element schema: a container item's row opens the
                    // runtime editor, so descend the same way.
                    if let Some(node) = runtime_descend(schema, &mut synth, doc, &mut stack, &ipath)
                    {
                        cur = node;
                        cur_path = ipath;
                        continue;
                    }
                    return stack;
                };
                if menu_leaf(schema, items) {
                    if raw_runtime(schema, items) {
                        if let Some(node) =
                            runtime_descend(schema, &mut synth, doc, &mut stack, &ipath)
                        {
                            cur = node;
                            cur_path = ipath;
                            continue;
                        }
                    }
                    return stack; // scalar item rows live in the array menu
                }
                if schema.nodes[items as usize].kind == Kind::OneOf {
                    if let Some(variants) = dynamic_variants(schema, items) {
                        let ival = doc.get(&ipath);
                        let inode = if match_variant_list(schema, &variants, doc, ival).len() == 1 {
                            let cands = match_variant_list(schema, &variants, doc, ival);
                            let mut synth = HashMap::new();
                            pinned_one_of_menu_id(schema, &mut synth, items, cands[0])
                        } else {
                            let mut synth = HashMap::new();
                            merged_one_of_menu_id(schema, &mut synth, items, &variants)
                        };
                        let mut st = state(MenuKind::Object, inode, ipath.clone());
                        st.variant_of = Some(items);
                        stack.push(st);
                        cur = inode;
                        cur_path = ipath;
                        continue;
                    }
                    // classic oneOf item: follow the matched leaf variant
                    // (oneOf variants may themselves be variant groups). A
                    // leaf variant's row lives in the array Items menu; an
                    // object variant gets its own editor menu.
                    let ival = doc.get(&ipath);
                    if matches!(ival, Some(v) if !is_null(doc, v)) {
                        if let Some(variant) = matched_leaf_variant(schema, items, doc, ival) {
                            if menu_leaf(schema, variant) {
                                if raw_runtime(schema, variant) {
                                    if let Some(node) =
                                        runtime_descend(schema, &mut synth, doc, &mut stack, &ipath)
                                    {
                                        cur = node;
                                        cur_path = ipath;
                                        continue;
                                    }
                                }
                                return stack;
                            }
                            let kind = menu_kind_for(schema, variant);
                            let mut st = state(kind, variant, ipath.clone());
                            st.one_of = Some(items);
                            stack.push(st);
                            cur = variant;
                            cur_path = ipath;
                            continue;
                        }
                    }
                    // unset item: its row is in the array Items menu
                    return stack;
                }
                let kind = menu_kind_for(schema, items);
                stack.push(state(kind, items, ipath.clone()));
                cur = items;
                cur_path = ipath;
            }
        }
    }
    for st in stack.iter_mut() {
        seed_variant_cursor(schema, st, doc);
    }
    stack
}
