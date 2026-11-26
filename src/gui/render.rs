//! GUI views, shared by the editor shell: theme styles, the header bar and
//! one row of the field list. Screens assemble these in `mod.rs`; sheets
//! live in `sheets.rs`.
//!
//! List cells are recycled: every per-row binding reads `at.index()` inside
//! the closure, never at element build time.

use std::rc::Rc;

use nsh_base::button;
use nsh_base::theme;
use nsh_core::prelude::ComponentBuilder;
use nsh_core::prelude::FontWeight;
use nsh_core::prelude::Interactive;
use nsh_core::prelude::IntoElement;
use nsh_core::prelude::ParentElement;
use nsh_core::prelude::Position;
use nsh_core::prelude::SharedString;
use nsh_core::prelude::Styled;
use nsh_core::prelude::px;
use nsh_core::prelude::text;
use nsh_core::prelude::view;
use nsh_core::prelude::when;

use crate::gui::bridge::{Bridge, RowOps};

/// A toolbar button with the app's look.
pub fn tool_button(
    label: &'static str,
    tip: &'static str,
    accent: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = SharedString::from(label);
    let mut b = button()
        .flex_row()
        .items_center()
        .justify_center()
        .accessibility_label(tip)
        .px(px(10.0))
        .h(px(32.0))
        .rounded(move || theme().radius)
        .active(|s| s.opacity(0.7))
        .on_press(on_press);
    if accent {
        b = b.bg(move || theme().primary);
        b = b.child(
            text(label)
                .text_size(px(13.0))
                .text_color(move || theme().primary_foreground),
        );
    } else {
        b = b.bg(move || theme().muted);
        b = b.child(
            text(label)
                .text_size(px(13.0))
                .text_color(move || theme().foreground),
        );
    }
    b
}

/// The header bar: back, file badge, breadcrumb, actions, save.
pub fn header_bar(ed: &Rc<Bridge>) -> impl IntoElement {
    let ed_back = ed.clone();
    let ed_file = ed.clone();
    let ed_crumb = ed.clone();
    let ed_filter_sel = ed.clone();
    let ed_filter_on = ed.clone();
    let ed_add = ed.clone();
    let ed_issues_on = ed.clone();
    let ed_issues_n = ed.clone();
    let ed_files_cond = ed.clone();
    let ed_files = ed.clone();
    let ed_save = ed.clone();
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px(px(12.0))
        .py(px(8.0))
        .border_b(px(1.0))
        .border_color(move || theme().border)
        .child(
            button()
                .flex_row()
                .items_center()
                .justify_center()
                .accessibility_label("Back")
                .px(px(8.0))
                .h(px(32.0))
                .rounded(move || theme().radius)
                .bg(move || theme().muted)
                .on_press(move || ed_back.back())
                .child(text("←").text_size(px(15.0))),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_1()
                .child(
                    text(move || {
                        ed_file.header().with(|h| {
                            let mut s = h.file.clone();
                            if h.dirty {
                                s.push_str(" ●");
                            }
                            s
                        })
                    })
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .number_of_lines(1)
                    .truncate(),
                )
                .child(
                    text(move || ed_crumb.header().with(|h| h.crumb.join(" / ")))
                        .text_size(px(13.0))
                        .text_color(move || theme().muted_foreground)
                        .number_of_lines(1)
                        .truncate(),
                ),
        )
        .child(
            button()
                .flex_row()
                .items_center()
                .justify_center()
                .accessibility_label("Toggle empty fields")
                .px(px(8.0))
                .h(px(32.0))
                .rounded(move || theme().radius)
                .bg(move || theme().muted)
                .selected(move || ed_filter_sel.header().with(|h| h.hide_empty))
                .on_press(move || ed_filter_on.hide_empty())
                .child(text("∅").text_size(px(14.0))),
        )
        .child(
            button()
                .flex_row()
                .items_center()
                .justify_center()
                .accessibility_label("Add key or item")
                .px(px(8.0))
                .h(px(32.0))
                .rounded(move || theme().radius)
                .bg(move || theme().muted)
                .on_press(move || ed_add.add_key())
                .child(text("＋").text_size(px(14.0))),
        )
        .child(
            button()
                .flex_row()
                .items_center()
                .justify_center()
                .accessibility_label("Issues")
                .px(px(8.0))
                .h(px(32.0))
                .rounded(move || theme().radius)
                .bg(move || theme().muted)
                .on_press(move || ed_issues_on.open_issues())
                .child(
                    text(move || {
                        ed_issues_n
                            .header()
                            .with(|h| format!("⚠ {}", h.errs + h.warns))
                    })
                    .text_size(px(13.0)),
                ),
        )
        .child(when(
            move || ed_files_cond.header().with(|h| h.files.len() > 1),
            move || {
                let ed_press = ed_files.clone();
                let ed_label = ed_files.clone();
                button()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .accessibility_label("Files")
                    .px(px(8.0))
                    .h(px(32.0))
                    .rounded(move || theme().radius)
                    .bg(move || theme().muted)
                    .on_press(move || ed_press.open_files())
                    .child(
                        text(move || {
                            ed_label
                                .header()
                                .with(|h| format!("{}/{}", h.cur + 1, h.files.len()))
                        })
                        .text_size(px(13.0)),
                    )
            },
        ))
        .child(tool_button("Save", "Save the file", true, move || {
            ed_save.save()
        }))
}

/// One row of the field list. Cells are recycled: only bindings that read
/// `at` re-run, so all per-row state comes from the bridge's rows signal.
pub fn list_row(ed: Rc<Bridge>, at: Position) -> impl IntoElement {
    let ed_label = ed.clone();
    let ed_color = ed.clone();
    let ed_summary = ed.clone();
    let ed_req = ed.clone();
    let ed_press = ed.clone();
    let ed_ops = ed.clone();
    view()
        .flex_row()
        .items_center()
        .min_h(px(44.0))
        .px(px(12.0))
        .border_b(px(1.0))
        .border_color(move || theme().border)
        .on_press(move || {
            let i = at.index();
            ed_press.activate(i);
        })
        .child(
            text(move || {
                let i = at.index();
                ed_label.rows().with(|rs| label_at(rs, i))
            })
            .text_size(px(14.0))
            .text_color(move || {
                let i = at.index();
                ed_color
                    .rows()
                    .with(|rs| match rs.get(i).map(|r| (r.err, r.warn)) {
                        Some((true, _)) => theme().destructive,
                        Some((_, true)) => theme().accent,
                        _ => theme().foreground,
                    })
            })
            .number_of_lines(1)
            .truncate(),
        )
        .child(
            text(move || {
                let i = at.index();
                ed_req.rows().with(|rs| {
                    if rs.get(i).is_some_and(|r| r.required) {
                        "*"
                    } else {
                        ""
                    }
                })
            })
            .text_size(px(14.0))
            .text_color(move || theme().destructive)
            .w(px(10.0)),
        )
        .child(
            view().flex_1().flex_row().justify_end().child(
                text(move || {
                    let i = at.index();
                    ed_summary
                        .rows()
                        .with(|rs| rs.get(i).map(|r| r.summary.clone()).unwrap_or_default())
                })
                .text_size(px(13.0))
                .text_color(move || theme().muted_foreground)
                .number_of_lines(1)
                .truncate(),
            ),
        )
        .child(row_ops(ed_ops, at))
}

/// The label of row `i`: kind markers mirrored from the TUI's row_line.
fn label_at(rows: &[crate::gui::bridge::RowView], i: usize) -> String {
    let Some(r) = rows.get(i) else {
        return String::new();
    };
    let mut s = match r.kind {
        crate::ui::RowKind::Toggle => {
            format!("{} {}", if r.on { "[x]" } else { "[ ]" }, r.label)
        }
        _ => r.label.clone(),
    };
    if r.not_in_schema {
        s.push_str(" · not in schema");
    }
    if r.err {
        s.insert_str(0, "✖ ");
    } else if r.warn {
        s.insert_str(0, "⚠ ");
    }
    if r.drill {
        s.push_str("  →");
    }
    s
}

/// Trailing row actions: insert/delete for array items, delete for members.
fn row_ops(ed: Rc<Bridge>, at: Position) -> impl IntoElement {
    let ed_has = ed.clone();
    let ed_ins = ed.clone();
    let ed_del = ed.clone();
    when(
        move || {
            let i = at.index();
            ed_has
                .rows()
                .with(|rs| rs.get(i).is_some_and(|r| r.ops != RowOps::None))
        },
        move || {
            let ed_ins_cond = ed_ins.clone();
            let ed_ins_then = ed_ins.clone();
            let ed_del = ed_del.clone();
            view()
                .flex_row()
                .gap_1()
                .child(when(
                    move || {
                        let i = at.index();
                        ed_ins_cond
                            .rows()
                            .with(|rs| rs.get(i).is_some_and(|r| r.ops == RowOps::ArrayOps))
                    },
                    move || {
                        let ed = ed_ins_then.clone();
                        op_button("⇥", "Insert item after", move || {
                            let i = at.index();
                            ed.insert_item(i, true);
                        })
                    },
                ))
                .child(op_button("✕", "Delete", move || {
                    let i = at.index();
                    ed_del.delete_row(i);
                }))
        },
    )
}

fn op_button(
    label: &'static str,
    tip: &'static str,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = SharedString::from(label);
    button()
        .flex_row()
        .items_center()
        .justify_center()
        .accessibility_label(tip)
        .size(px(30.0))
        .rounded(move || theme().radius)
        .bg(move || theme().muted)
        .active(|s| s.opacity(0.6))
        .on_press(on_press)
        .child(text(label).text_size(px(13.0)))
}

/// The status line under the list: transient notice, schema status or load
/// errors.
pub fn status_line(ed: &Rc<Bridge>) -> impl IntoElement {
    let ed_msg = ed.clone();
    let ed_counts = ed.clone();
    view()
        .flex_row()
        .items_center()
        .min_h(px(28.0))
        .px(px(12.0))
        .border_t(px(1.0))
        .border_color(move || theme().border)
        .child(
            text(move || {
                let notice = ed_msg.notice().get();
                if !notice.is_empty() {
                    return notice;
                }
                ed_msg.header().with(|h| {
                    if h.load_errors > 0 {
                        return format!("{} problems on load", h.load_errors);
                    }
                    h.status.clone()
                })
            })
            .text_size(px(12.0))
            .text_color(move || theme().muted_foreground)
            .number_of_lines(1)
            .truncate(),
        )
        .child(view().flex_1())
        .child(
            text(move || {
                ed_counts.header().with(|h| {
                    let mut parts: Vec<String> = Vec::new();
                    if h.dirty {
                        parts.push("unsaved".to_string());
                    }
                    if h.errs > 0 {
                        parts.push(format!("{} errors", h.errs));
                    }
                    if h.warns > 0 {
                        parts.push(format!("{} warnings", h.warns));
                    }
                    parts.join(" · ")
                })
            })
            .text_size(px(12.0))
            .text_color(move || theme().muted_foreground),
        )
}
