//! GUI front end (Native Shell): a list/form editor over the same core the
//! terminal UI drives. The model is a headless terminal [`App`] driven
//! through its own key dispatcher (`bridge::Bridge`), so editing semantics
//! are the TUI's by construction; this module owns the window shell, the
//! screen switch and the desktop mounts.
//!
//! Gated behind the `gui` feature; the binary picks this or the terminal
//! UI. The GUI never loads or compiles schemas itself: sessions arrive
//! preloaded and the binary provides the `OpenFn` loader for files opened
//! inside the app — the same rule `src/ui` enforces architecturally.
//!
//! Platforms: `run()` mounts a desktop window (Linux/macOS/Windows); on
//! Android the Native Shell runner mounts [`App`] directly (`nsh.json`),
//! which starts at the open screen unless files were passed on the command
//! line (desktops only).

mod bridge;
mod render;
mod sheets;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;

use nsh_base::theme;
use nsh_core::element::desktop::window as app_window;
use nsh_core::prelude::IntoElement;
use nsh_core::prelude::ListLayout;
use nsh_core::prelude::ParentElement;
use nsh_core::prelude::Styled;
use nsh_core::prelude::px;
use nsh_core::prelude::view;
use nsh_core::prelude::virtual_list;
use nsh_core::prelude::when;

pub use bridge::{Bridge, OpenFn, SharedOutcome};

use crate::ui::{FileSession, Outcome};

/// What `run()` hands the root component: all of it `Send`, because the
/// component runs on the engine thread, not the caller's.
struct Boot {
    files: Vec<FileSession>,
    open: Option<OpenFn>,
    outcome: Arc<SharedOutcome>,
}

// Picked up once by the root component; empty (open screen) when a runner
// mounts `App` directly (Android, `nsh run`).
static BOOT: Mutex<Option<Boot>> = Mutex::new(None);

/// Runs the GUI over preloaded sessions and returns the outcome for the
/// binary's exit mapping. Blocks until the window closes (desktops). The
/// desktop mounts require the desktop platform backends (`gui` feature,
/// not just `gui-lib`).
pub fn run(files: Vec<FileSession>, open: Option<OpenFn>) -> Outcome {
    let outcome = Arc::new(SharedOutcome::default());
    if let Ok(mut slot) = BOOT.lock() {
        *slot = Some(Boot {
            files,
            open,
            outcome: Arc::clone(&outcome),
        });
    }

    // Desktop mounts; Android reaches App through the Native Shell runner
    // (nsh.json "root"), where run() is never called.
    #[cfg(all(target_os = "linux", feature = "gui"))]
    nsh_platform_linux::run_app(
        nsh_platform_linux::AppSource::Rust(nsh_platform_linux::Root::of::<App>()),
        nsh_platform_linux::AppOptions {
            title: "jse".to_owned(),
            app_id: "dev.jse.editor".to_owned(),
            size: (1100, 760),
            ..Default::default()
        },
    );
    #[cfg(all(target_os = "macos", feature = "gui"))]
    nsh_platform_macos::run_app(
        nsh_platform_macos::AppSource::Rust(nsh_platform_macos::Root::of::<App>()),
        nsh_platform_macos::AppOptions {
            title: "jse".to_owned(),
            size: (1100, 760),
            ..Default::default()
        },
    );
    #[cfg(all(target_os = "windows", feature = "gui"))]
    nsh_platform_windows::run_app(
        nsh_platform_windows::AppSource::Rust(nsh_platform_windows::Root::of::<App>()),
        nsh_platform_windows::AppOptions {
            title: "jse".to_owned(),
            app_id: "dev.jse.editor".to_owned(),
            size: (1100, 760),
            ..Default::default()
        },
    );

    outcome.into_outcome()
}

/// The application's root component: the first window's content on the
/// desktops, the app's screen off them (Android).
#[nsh_core::prelude::component]
pub fn App() -> impl IntoElement {
    let ed = match BOOT.lock().ok().and_then(|mut b| b.take()) {
        Some(boot) => Rc::new(Bridge::new(boot.files, boot.outcome, boot.open)),
        None => Rc::new(Bridge::new(
            Vec::new(),
            Arc::new(SharedOutcome::default()),
            None,
        )),
    };
    // Signals exist only on the engine thread inside a component.
    ed.attach(bridge::Views::new());
    ed.set_viewport(120, 40);
    ed.publish();
    let content = move || shell(ed.clone());
    app_window().id("main").content(content)
}

/// The shell: open screen until a file is loaded, editor over it.
fn shell(ed: Rc<Bridge>) -> impl IntoElement {
    let ed_open = ed.clone();
    let ed_edit = ed.clone();
    when(
        move || !ed.loaded().get(),
        move || sheets::open_screen(ed_open.clone()),
    )
    .otherwise(move || editor_shell(ed_edit.clone()))
}

/// The editor screen: header bar, the field list, the status line, and the
/// open sheet over everything.
fn editor_shell(ed: Rc<Bridge>) -> impl IntoElement {
    let ed_header = ed.clone();
    let ed_count = ed.clone();
    let ed_list = ed.clone();
    let ed_status = ed.clone();
    let ed_sheet_cond = ed.clone();
    let ed_sheet_body = ed.clone();
    view()
        .flex_1()
        .flex_col()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(render::header_bar(&ed_header))
        .child(
            virtual_list(
                move || ed_count.rows().with(|r| r.len()),
                move |at| render::list_row(ed_list.clone(), at),
            )
            .estimated_item_size(px(44.0))
            .flex_1(),
        )
        .child(render::status_line(&ed_status))
        .child(when(
            move || ed_sheet_cond.sheet().get().is_some(),
            move || sheets::sheet_dialog(ed_sheet_body.clone()),
        ))
}
