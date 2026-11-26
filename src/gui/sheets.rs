//! Modal surfaces of the GUI: the open screen and one sheet per overlay
//! kind the editor drives. Sheets dispatch back through the bridge (Apply =
//! Enter, Cancel = Esc), so apply semantics stay the TUI's.
//!
//! Sheet constructors take the bridge by value so their `impl IntoElement`
//! stays `'static`; `sheet_body` wraps each in a `view().child(..)` so the
//! `dynamic` arms share one element type.

use std::rc::Rc;

use nsh_base::SelectItem;
use nsh_base::button;
use nsh_base::dialog;
use nsh_base::input;
use nsh_base::select;
use nsh_base::theme;
use nsh_core::prelude::ComponentBuilder;
use nsh_core::prelude::FontWeight;
use nsh_core::prelude::Interactive;
use nsh_core::prelude::IntoElement;
use nsh_core::prelude::ParentElement;
use nsh_core::prelude::SharedString;
use nsh_core::prelude::Styled;
use nsh_core::prelude::dynamic;
use nsh_core::prelude::each;
#[cfg(any(target_os = "android", target_os = "ios"))]
use nsh_core::prelude::pick_files;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use nsh_core::prelude::pick_paths;
use nsh_core::prelude::px;
use nsh_core::prelude::rgba;
use nsh_core::prelude::scroll;
use nsh_core::prelude::spawn;
use nsh_core::prelude::text;
use nsh_core::prelude::view;
use nsh_core::prelude::when;

use crate::gui::bridge::{Bridge, SheetView};

/// Opens the system's file picker and loads the picked config: the
/// document picker (SAF) on Android/iOS — the picked bytes are stored in
/// the app's data directory — and the open dialog on the desktops.
fn browse_for_config(ed: Rc<Bridge>) {
    spawn(async move {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let mut request = nsh_core::picker::FilePickRequest::new(false, 0);
            request.filters = vec![nsh_core::file_dialog::FileFilter::new(
                "Config",
                ["json", "jsonc", "yaml", "yml", "toml"],
            )];
            match pick_files(request).await {
                Ok(files) if !files.is_empty() => {
                    let f = &files[0];
                    if f.status == nsh_core::picker::PickedStatus::Read {
                        ed.open_picked(&f.name, &f.data);
                    } else {
                        ed.open_err()
                            .set(format!("{} is too large to open", f.name));
                    }
                }
                Ok(_) => {} // cancelled
                Err(e) => ed.open_err().set(e),
            }
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            match pick_paths(nsh_core::file_dialog::OpenDialog::files()).await {
                Ok(Some(paths)) => {
                    if let Some(path) = paths.first() {
                        let path = path.to_string_lossy().to_string();
                        ed.open_file(&path);
                    }
                }
                Ok(None) => {} // cancelled
                Err(e) => ed.open_err().set(e),
            }
        }
    });
}

/// The open screen: shown when no file is loaded (Android launch, or the
/// binary started without arguments).
pub fn open_screen(ed: Rc<Bridge>) -> impl IntoElement {
    let ed_open = ed.clone();
    let ed_set = ed.clone();
    let ed_press = ed.clone();
    let ed_browse = ed.clone();
    let ed_err = ed.clone();
    view()
        .flex_1()
        .justify_center()
        .px(px(24.0))
        .gap_4()
        .bg(move || theme().background)
        .child(
            text("jse")
                .text_size(px(32.0))
                .font_weight(FontWeight::BOLD)
                .text_color(move || theme().foreground),
        )
        .child(
            text("Schema-driven config editor")
                .text_size(px(14.0))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            input()
                .value(move || ed_open.open_path().get())
                .on_change(move |s: &str| ed_set.open_path().set(s.to_string()))
                .placeholder("config.(json|jsonc|yaml|toml)")
                .auto_focus(true)
                .h(px(42.0))
                .px(px(10.0))
                .rounded(move || theme().radius)
                .border_1()
                .border_color(move || theme().input)
                .bg(move || theme().card)
                .text_size(px(14.0)),
        )
        .child(
            button()
                .flex_row()
                .items_center()
                .justify_center()
                .px(px(16.0))
                .h(px(42.0))
                .rounded(move || theme().radius)
                .bg(move || theme().primary)
                .active(|s| s.opacity(0.8))
                .on_press(move || {
                    let p = ed_open_path(&ed_press);
                    ed_press.open_file(&p);
                })
                .child(
                    text("Open")
                        .text_size(px(15.0))
                        .text_color(move || theme().primary_foreground),
                ),
        )
        .child(
            button()
                .flex_row()
                .items_center()
                .justify_center()
                .px(px(16.0))
                .h(px(42.0))
                .rounded(move || theme().radius)
                .bg(move || theme().muted)
                .active(|s| s.opacity(0.8))
                .on_press(move || browse_for_config(ed_browse.clone()))
                .child(
                    text("Browse…")
                        .text_size(px(15.0))
                        .text_color(move || theme().foreground),
                ),
        )
        .child(
            text(move || ed_err.open_err().get())
                .text_size(px(13.0))
                .text_color(move || theme().destructive),
        )
}

fn ed_open_path(ed: &Rc<Bridge>) -> String {
    ed.open_path().get()
}

/// The open sheet, if any. Presence is the dialog; the body follows the
/// sheet signal so chained overlays (add key → field editor) swap in place.
pub fn sheet_dialog(ed: Rc<Bridge>) -> impl IntoElement {
    let ed_cancel = ed.clone();
    dialog()
        .close_on_backdrop_press(true)
        .close_on_escape(true)
        .on_cancel(move || ed_cancel.sheet_cancel())
        .child(
            view()
                .flex_1()
                .items_center()
                .justify_center()
                .p(px(24.0))
                .bg(rgba(0x00000066))
                .child(sheet_body(ed)),
        )
}

/// One row of a list sheet: its position (the each key) and its text.
#[derive(Clone, Debug, PartialEq)]
struct LineRow {
    i: usize,
    line: String,
}

/// One dynamic arm, type-erased into a concrete `View`.
fn sheet_arm(el: impl IntoElement) -> nsh_core::prelude::View {
    view().child(el)
}

/// Dispatches on the open sheet kind; rebuilt whenever the sheet changes.
fn sheet_body(ed: Rc<Bridge>) -> impl IntoElement {
    dynamic(move || {
        let Some(s) = ed.sheet().get() else {
            return view();
        };
        let body = match s {
            SheetView::Edit { .. } => sheet_arm(edit_sheet(ed.clone(), s)),
            SheetView::Text { .. } => sheet_arm(text_sheet(ed.clone(), s)),
            SheetView::Add { .. } => sheet_arm(add_sheet(ed.clone(), s)),
            SheetView::Confirm { .. } => sheet_arm(confirm_sheet(ed.clone(), s)),
            SheetView::Issues { .. } | SheetView::Files { .. } => {
                sheet_arm(list_sheet(ed.clone(), s))
            }
            SheetView::Errs { .. } => sheet_arm(errs_sheet(ed.clone(), s)),
        };
        body
    })
}

/// The field editor: enum → select, else a text input (numbers included;
/// the TUI validates on apply, the error shows under the input).
fn edit_sheet(ed: Rc<Bridge>, s: SheetView) -> impl IntoElement {
    let SheetView::Edit {
        title,
        path,
        text: value,
        err,
        options,
        selected,
        ..
    } = s
    else {
        unreachable!("edit_sheet with another sheet");
    };
    let ed_set = ed.clone();
    let ed_submit = ed.clone();
    let ed_pick = ed.clone();
    let ed_apply = ed.clone();
    let ed_cancel = ed.clone();
    let empty = options.is_empty();
    let items: Vec<SelectItem> = options
        .iter()
        .map(|o| SelectItem::new(SharedString::from(o.clone()), SharedString::from(o.clone())))
        .collect();
    let opts_value = options.clone();
    let opts_pick = options.clone();
    view()
        .w(px(460.0))
        .p(px(16.0))
        .gap_3()
        .rounded(move || theme().radius)
        .bg(move || theme().card)
        .child(
            text(SharedString::from(title))
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            text(SharedString::from(path))
                .text_size(px(12.0))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            input()
                .value(move || value.to_string())
                .on_change(move |s: &str| ed_set.sheet_set_text(s))
                .on_submit(move |s: &str| {
                    ed_submit.sheet_set_text(s);
                    ed_submit.sheet_apply();
                })
                .auto_focus(true)
                .h(px(40.0))
                .px(px(10.0))
                .rounded(move || theme().radius)
                .border_1()
                .border_color(move || theme().input)
                .bg(move || theme().background)
                .text_size(px(14.0)),
        )
        .child(when(
            move || !empty,
            move || {
                let ed = ed_pick.clone();
                let selected = selected;
                let opts_value = opts_value.clone();
                let opts_pick = opts_pick.clone();
                select()
                    .items(items.clone())
                    .value(move || selected.map(|i| SharedString::from(opts_value[i].clone())))
                    .on_change(move |v: &Option<SharedString>| {
                        let Some(v) = v else { return };
                        let k = opts_pick.iter().position(|o| o == &**v).unwrap_or(0);
                        ed.sheet_pick_option(k);
                    })
                    .h(px(40.0))
            },
        ))
        .child(
            text(SharedString::from(err))
                .text_size(px(12.0))
                .text_color(move || theme().destructive),
        )
        .child(sheet_buttons(ed_apply, ed_cancel))
}

/// The raw/multiline editor.
fn text_sheet(ed: Rc<Bridge>, s: SheetView) -> impl IntoElement {
    let SheetView::Text {
        title,
        path,
        text: value,
        err,
    } = s
    else {
        unreachable!("text_sheet with another sheet");
    };
    let ed_set = ed.clone();
    let ed_apply = ed.clone();
    let ed_cancel = ed.clone();
    view()
        .w(px(560.0))
        .p(px(16.0))
        .gap_3()
        .rounded(move || theme().radius)
        .bg(move || theme().card)
        .child(
            text(SharedString::from(title))
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            text(SharedString::from(path))
                .text_size(px(12.0))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            input()
                .value(move || value.to_string())
                .on_change(move |s: &str| ed_set.sheet_set_text(s))
                .multiline(true)
                .auto_focus(true)
                .h(px(200.0))
                .px(px(10.0))
                .py(px(8.0))
                .rounded(move || theme().radius)
                .border_1()
                .border_color(move || theme().input)
                .bg(move || theme().background)
                .text_size(px(13.0)),
        )
        .child(
            text(SharedString::from(err))
                .text_size(px(12.0))
                .text_color(move || theme().destructive),
        )
        .child(sheet_buttons(ed_apply, ed_cancel))
}

/// The key-name input.
fn add_sheet(ed: Rc<Bridge>, s: SheetView) -> impl IntoElement {
    let SheetView::Add {
        title,
        text: value,
        err,
    } = s
    else {
        unreachable!("add_sheet with another sheet");
    };
    let ed_set = ed.clone();
    let ed_submit = ed.clone();
    let ed_apply = ed.clone();
    let ed_cancel = ed.clone();
    view()
        .w(px(420.0))
        .p(px(16.0))
        .gap_3()
        .rounded(move || theme().radius)
        .bg(move || theme().card)
        .child(
            text(SharedString::from(title))
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            input()
                .value(move || value.to_string())
                .on_change(move |s: &str| ed_set.sheet_set_text(s))
                .on_submit(move |s: &str| {
                    ed_submit.sheet_set_text(s);
                    ed_submit.sheet_apply();
                })
                .auto_focus(true)
                .h(px(40.0))
                .px(px(10.0))
                .rounded(move || theme().radius)
                .border_1()
                .border_color(move || theme().input)
                .bg(move || theme().background)
                .text_size(px(14.0)),
        )
        .child(
            text(SharedString::from(err))
                .text_size(px(12.0))
                .text_color(move || theme().destructive),
        )
        .child(sheet_buttons(ed_apply, ed_cancel))
}

/// Save / exit confirmation: y / n / stay.
fn confirm_sheet(ed: Rc<Bridge>, s: SheetView) -> impl IntoElement {
    let SheetView::Confirm { title, hint } = s else {
        unreachable!("confirm_sheet with another sheet");
    };
    let ed_stay = ed.clone();
    let ed_no = ed.clone();
    let ed_yes = ed.clone();
    view()
        .w(px(420.0))
        .p(px(16.0))
        .gap_3()
        .rounded(move || theme().radius)
        .bg(move || theme().card)
        .child(
            text(SharedString::from(title))
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            text(SharedString::from(hint))
                .text_size(px(13.0))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(
                    button()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .px(px(12.0))
                        .h(px(34.0))
                        .rounded(move || theme().radius)
                        .bg(move || theme().muted)
                        .on_press(move || ed_stay.sheet_cancel())
                        .child(text("Stay").text_size(px(13.0))),
                )
                .child(
                    button()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .px(px(12.0))
                        .h(px(34.0))
                        .rounded(move || theme().radius)
                        .bg(move || theme().destructive)
                        .on_press(move || ed_no.confirm(false))
                        .child(
                            text("Discard")
                                .text_size(px(13.0))
                                .text_color(move || theme().background),
                        ),
                )
                .child(
                    button()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .px(px(12.0))
                        .h(px(34.0))
                        .rounded(move || theme().radius)
                        .bg(move || theme().primary)
                        .on_press(move || ed_yes.confirm(true))
                        .child(
                            text("Save & exit")
                                .text_size(px(13.0))
                                .text_color(move || theme().primary_foreground),
                        ),
                ),
        )
}

/// The issue list / file switcher: rows that jump or switch on press.
fn list_sheet(ed: Rc<Bridge>, s: SheetView) -> impl IntoElement {
    let ed_press = ed.clone();
    let (items, cursor) = match &s {
        SheetView::Issues { items, cursor } => (items.clone(), *cursor),
        SheetView::Files { paths, cursor } => (paths.clone(), *cursor),
        _ => (Vec::new(), 0),
    };
    let title = SharedString::from(s.title().to_string());
    view()
        .w(px(560.0))
        .max_h(px(420.0))
        .p(px(8.0))
        .rounded(move || theme().radius)
        .bg(move || theme().card)
        .child(
            text(title)
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .px(px(8.0))
                .py(px(6.0)),
        )
        .child(scroll().max_h(px(340.0)).child(view().gap_1().child(each(
            // keyed by position: duplicate texts must not collide
            move || {
                items
                    .iter()
                    .enumerate()
                    .map(|(i, line)| LineRow {
                        i,
                        line: line.clone(),
                    })
                    .collect::<Vec<_>>()
            },
            |row: &LineRow| row.i,
            move |row: nsh_core::prelude::ReadSignal<LineRow>,
                  i: nsh_core::prelude::ReadSignal<usize>| {
                let ed_row = ed_press.clone();
                view()
                    .flex_row()
                    .items_center()
                    .px(px(8.0))
                    .py(px(8.0))
                    .rounded(move || theme().radius)
                    .style_when(move || i.get() == cursor, |st| st.bg(move || theme().muted))
                    .on_press(move || ed_row.sheet_pick_row(i.get()))
                    .child(
                        text(move || row.with(|r| r.line.clone()))
                            .text_size(px(13.0))
                            .number_of_lines(2),
                    )
            },
        ))))
}

/// Save-time validation errors: press a line to jump there; a `--force`
/// session can write past them.
fn errs_sheet(ed: Rc<Bridge>, s: SheetView) -> impl IntoElement {
    let SheetView::Errs {
        lines,
        cursor,
        can_force,
    } = s
    else {
        unreachable!("errs_sheet with another sheet");
    };
    let ed_row = ed.clone();
    let ed_force = ed.clone();
    let ed_cancel = ed.clone();
    view()
        .w(px(620.0))
        .max_h(px(460.0))
        .p(px(12.0))
        .gap_2()
        .rounded(move || theme().radius)
        .bg(move || theme().card)
        .child(
            text("Validation errors")
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().destructive)
                .px(px(4.0))
                .py(px(4.0)),
        )
        .child(scroll().max_h(px(340.0)).child(view().gap_1().child(each(
            move || lines.clone(),
            |line: &String| line.clone(),
            move |line: nsh_core::prelude::ReadSignal<String>,
                  i: nsh_core::prelude::ReadSignal<usize>| {
                let ed = ed_row.clone();
                view()
                    .px(px(6.0))
                    .py(px(6.0))
                    .rounded(move || theme().radius)
                    .style_when(move || i.get() == cursor, |st| st.bg(move || theme().muted))
                    .on_press(move || ed.sheet_pick_row(i.get()))
                    .child(
                        text(move || line.get())
                            .text_size(px(12.5))
                            .number_of_lines(3),
                    )
            },
        ))))
        .child(
            view()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(
                    button()
                        .px(px(12.0))
                        .h(px(34.0))
                        .rounded(move || theme().radius)
                        .bg(move || theme().muted)
                        .on_press(move || ed_cancel.sheet_cancel())
                        .child(text("Cancel").text_size(px(13.0))),
                )
                .child(when(
                    move || can_force,
                    move || {
                        let ed = ed_force.clone();
                        button()
                            .px(px(12.0))
                            .h(px(34.0))
                            .rounded(move || theme().radius)
                            .bg(move || theme().destructive)
                            .on_press(move || ed.write_force())
                            .child(
                                text("Save anyway (--force)")
                                    .text_size(px(13.0))
                                    .text_color(move || theme().background),
                            )
                    },
                )),
        )
}

/// Apply / Cancel row.
fn sheet_buttons(ed_apply: Rc<Bridge>, ed_cancel: Rc<Bridge>) -> impl IntoElement {
    view()
        .flex_row()
        .justify_end()
        .gap_2()
        .child(
            button()
                .flex_row()
                .items_center()
                .justify_center()
                .px(px(12.0))
                .h(px(34.0))
                .rounded(move || theme().radius)
                .bg(move || theme().muted)
                .on_press(move || ed_cancel.sheet_cancel())
                .child(text("Cancel").text_size(px(13.0))),
        )
        .child(
            button()
                .flex_row()
                .items_center()
                .justify_center()
                .px(px(12.0))
                .h(px(34.0))
                .rounded(move || theme().radius)
                .bg(move || theme().primary)
                .on_press(move || ed_apply.sheet_apply())
                .child(
                    text("Apply")
                        .text_size(px(13.0))
                        .text_color(move || theme().primary_foreground),
                ),
        )
}
