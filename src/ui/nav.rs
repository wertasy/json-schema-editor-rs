//! Navigation history for the menu TUI: every relocation — drill-ins into
//! submenus, level pops, and jumps (gg/G, ga/gr, search/issue/error jumps) —
//! records the menu stack it leaves behind. ctrl+o walks back through that
//! trail, ctrl+i forward (bubbletea reports ctrl+i as the tab key: same
//! byte). Fresh forward navigation drops the not-yet-visited forward branch,
//! like a browser.
//!
//! Rust port of `internal/ui/menu_nav.go`. Go hangs the trails on the
//! `*listModel`; here they live in [`Nav`] and the functions take/return
//! marks explicitly, because the stack and the current file index belong to
//! the menu layer (`FileState`, src/ui/files.rs).
//!
//! Contract of `App::nav_restore` for the caller (menu layer): when it moves
//! the session to the returned (stack, file index) it must also
//!   1. switch the active file if the index differs from the current tab
//!      (plain swap — `nav_go` already moved the trails before restoring),
//!   2. clear the parked one-level-return and overlay-return state
//!      (`jump_origin`, `jump_return`, `search.parked`, `issues_back`): the
//!      restored location has no relation to the current landing,
//!   3. run `history::repair_stack` on the restored stack (levels the
//!      document no longer resolves — undo/redo shrank it meanwhile — are
//!      pruned like after an undo, and cursors clamp to surviving rows).

use crate::doc::jsondoc::path_string;
use crate::ui::MenuState;

/// maxNavHistory caps each direction of the navigation trail; the oldest
/// entry is dropped first.
pub const MAX_NAV_HISTORY: usize = 100;

/// navMark is one frozen menu stack: value copies, so cursors and offsets
/// are snapshots, not live state. `file` is the tab the location belongs to,
/// so the trail can step across files.
#[derive(Clone, Debug)]
pub struct NavMark {
    pub stack: Vec<MenuState>,
    pub file: usize,
}

/// The two navigation trails. Go: `navBack` / `navFwd` on the listModel.
#[derive(Default)]
pub struct Nav {
    pub back: Vec<NavMark>,
    pub fwd: Vec<NavMark>,
}

/// Result of [`Nav::go`]: either a status notice (byte-identical to Go) or
/// the mark to hand to `App::nav_restore`.
#[derive(Clone, Debug)]
pub enum NavGo {
    Stay(&'static str),
    Jump(NavMark),
}

/// navSnapshot freezes the current menu stack.
pub fn nav_snapshot(stack: &[MenuState], file: usize) -> NavMark {
    NavMark {
        stack: stack.to_vec(),
        file,
    }
}

/// sameMark reports whether two marks describe the same location: node
/// identity per level plus the frozen cursor. Rebuilt dynamic menus may
/// compare unequal for the same path, which only leaves a stale entry.
pub fn same_mark(a: &NavMark, b: &NavMark) -> bool {
    if a.file != b.file {
        return false;
    }
    if a.stack.len() != b.stack.len() {
        return false;
    }
    for (x, y) in a.stack.iter().zip(b.stack.iter()) {
        if x.node != y.node
            || x.kind != y.kind
            || x.cursor != y.cursor
            || x.variant_of != y.variant_of
            || x.one_of != y.one_of
            || path_string(&x.path) != path_string(&y.path)
        {
            return false;
        }
    }
    true
}

/// navTrailPush appends a mark, capping the trail.
pub fn nav_trail_push(trail: &mut Vec<NavMark>, mk: NavMark) {
    trail.push(mk);
    if trail.len() > MAX_NAV_HISTORY {
        trail.remove(0);
    }
}

impl Nav {
    /// navForward records the location left behind by a forward relocation
    /// (drill-in, jump): it becomes a ctrl+o back step, and the ctrl+i
    /// forward branch dies like a browser's. `leaving` is the frozen current
    /// stack (see [`nav_snapshot`]).
    pub fn forward(&mut self, leaving: NavMark) {
        nav_trail_push(&mut self.back, leaving);
        self.fwd.clear();
    }

    /// navBackStep records the location a back motion (esc level pop,
    /// one-level return to a jump or search/issue origin) leaves behind as a
    /// ctrl+i forward step. When the landing is exactly the trail's back top
    /// — the common drill/esc cycle — that entry is consumed, so the cycle
    /// does not pile up dead locations. Call after the stack changed, with
    /// `landing` the frozen new stack.
    pub fn back_step(&mut self, leaving: NavMark, landing: &NavMark) {
        if let Some(top) = self.back.last() {
            if same_mark(top, landing) {
                self.back.pop();
            }
        }
        nav_trail_push(&mut self.fwd, leaving);
    }

    /// navGo moves one step along the navigation trail: back=true is ctrl+o
    /// (older), back=false is ctrl+i (newer). List mode only — overlays and
    /// inputs route keys elsewhere. On `NavGo::Jump` the caller passes the
    /// mark to `App::nav_restore`; the trails were already updated here.
    pub fn go(&mut self, back: bool, current: &NavMark) -> NavGo {
        let leaving = current.clone();
        if back {
            let Some(mk) = self.back.pop() else {
                return NavGo::Stay("no earlier location");
            };
            nav_trail_push(&mut self.fwd, leaving);
            return NavGo::Jump(mk);
        }
        let Some(mk) = self.fwd.pop() else {
            return NavGo::Stay("no later location");
        };
        nav_trail_push(&mut self.back, leaving);
        NavGo::Jump(mk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::MenuKind;

    fn st(node: u32, path: &[&str], cursor: usize) -> MenuState {
        MenuState {
            kind: MenuKind::Object,
            node,
            path: path
                .iter()
                .map(|k| crate::doc::PathSeg::Key((*k).into()))
                .collect(),
            cursor,
            offset: 0,
            variant_of: None,
            one_of: None,
            sel: Default::default(),
            visual: false,
            vanchor: 0,
            filter: Default::default(),
        }
    }

    fn mark(file: usize, levels: &[(u32, &[&str], usize)]) -> NavMark {
        NavMark {
            stack: levels.iter().map(|(n, p, c)| st(*n, p, *c)).collect(),
            file,
        }
    }

    fn cur(file: usize) -> NavMark {
        mark(file, &[(1, &[], 0), (2, &["a"], 3)])
    }

    #[test]
    fn forward_clears_fwd_and_pushes_back() {
        let mut nav = Nav::default();
        nav.fwd.push(mark(0, &[(9, &["old"], 0)]));
        nav.forward(cur(0));
        assert_eq!(nav.back.len(), 1);
        assert!(nav.fwd.is_empty());
        assert_eq!(nav.back[0].file, 0);
        assert_eq!(nav.back[0].stack.len(), 2);
    }

    #[test]
    fn same_mark_identity_rules() {
        let a = cur(0);
        // identical content: same
        assert!(same_mark(&a, &cur(0)));
        // different file: not the same location
        assert!(!same_mark(&a, &cur(1)));
        // different node identity
        let mut b = cur(0);
        b.stack[1].node = 7;
        assert!(!same_mark(&a, &b));
        // different cursor
        let mut b = cur(0);
        b.stack[1].cursor = 4;
        assert!(!same_mark(&a, &b));
        // different kind
        let mut b = cur(0);
        b.stack[1].kind = MenuKind::Items;
        assert!(!same_mark(&a, &b));
        // different variant_of / one_of
        for field in [0usize, 1] {
            let mut b = cur(0);
            if field == 0 {
                b.stack[1].variant_of = Some(5);
            } else {
                b.stack[1].one_of = Some(5);
            }
            assert!(!same_mark(&a, &b));
        }
        // different stack depth
        let mut b = cur(0);
        b.stack.pop();
        assert!(!same_mark(&a, &b));
        // same location, different path spellings (rebuilt menus): compared
        // as path strings, so equivalent paths match
        let mut b = mark(0, &[(1, &[], 0), (2, &["a"], 3)]);
        b.stack[1].node = 9; // node identity differs → still unequal
        assert!(!same_mark(&a, &b));
    }

    #[test]
    fn same_mark_compares_paths_as_strings() {
        // two marks whose path segments are built differently but spell the
        // same path string are the same location (Go compares
        // jsondoc.PathString, not slice identity)
        let mut a = cur(0);
        let b = cur(0);
        a.stack[1].path = vec![crate::doc::PathSeg::Key("a".to_string())];
        assert!(same_mark(&a, &b));
    }

    #[test]
    fn trail_push_capped() {
        let mut trail = Vec::new();
        for i in 0..105 {
            nav_trail_push(&mut trail, mark(0, &[(i as u32, &[], 0)]));
        }
        assert_eq!(trail.len(), MAX_NAV_HISTORY);
        assert_eq!(trail[0].stack[0].node, 5, "oldest entries dropped first");
        assert_eq!(trail[99].stack[0].node, 104);
    }

    #[test]
    fn go_notices_when_empty() {
        let mut nav = Nav::default();
        let c = cur(0);
        assert!(matches!(
            nav.go(true, &c),
            NavGo::Stay("no earlier location")
        ));
        assert!(matches!(
            nav.go(false, &c),
            NavGo::Stay("no later location")
        ));
    }
    #[test]
    fn forward_then_back_step_cycle() {
        // drill-in: forward() parks the list, then esc: back_step() consumes
        // the matching back top and parks the submenu as a forward step.
        let mut nav = Nav::default();
        let list = mark(0, &[(1, &[], 0)]);
        let sub = mark(0, &[(1, &[], 0), (2, &["a"], 1)]);
        nav.forward(list.clone());
        nav.back_step(sub.clone(), &list);
        // the drill/esc cycle consumed the back top: trail is clean
        assert!(nav.back.is_empty());
        assert_eq!(nav.fwd.len(), 1);
        assert!(same_mark(&nav.fwd[0], &sub));
        // landing elsewhere does NOT consume the back top
        let mut nav2 = Nav::default();
        let jump_origin = mark(0, &[(1, &[], 0)]);
        let elsewhere = mark(0, &[(1, &[], 5)]);
        nav2.forward(jump_origin);
        nav2.back_step(sub.clone(), &elsewhere);
        assert_eq!(nav2.back.len(), 1, "unrelated landing keeps the back entry");
        assert_eq!(nav2.fwd.len(), 1);
    }

    #[test]
    fn go_round_trip_moves_marks_between_trails() {
        let mut nav = Nav::default();
        let list = cur(0);
        nav.forward(mark(0, &[(1, &[], 0)])); // drilled in from `list`

        // ctrl+o: back to the list, leaving becomes a forward step
        let jump = nav.go(true, &list);
        let NavGo::Jump(mk) = jump else {
            panic!("expected jump")
        };
        assert_eq!(mk.file, 0);
        assert_eq!(
            mk.stack.len(),
            1,
            "the back mark is the pre-drill list stack"
        );
        assert!(nav.back.is_empty());
        assert!(same_mark(&nav.fwd[0], &list));

        // ctrl+i: forward again
        let jump = nav.go(false, &list);
        let NavGo::Jump(mk) = jump else {
            panic!("expected jump")
        };
        assert_eq!(mk.stack.len(), 2);
        assert_eq!(crate::doc::jsondoc::path_string(&mk.stack[1].path), "$.a");
        assert!(same_mark(&nav.back[0], &list));
        assert!(nav.fwd.is_empty());
    }
}
