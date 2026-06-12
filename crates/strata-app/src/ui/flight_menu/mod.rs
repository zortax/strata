//! The title-bar Flight menu and flight strip (design §2) — the entry into
//! planning mode.
//!
//! The menu is a plain text button rendered with the established CSD-safe
//! slot technique (see `ui::theme::render_theme_toggle`): absolutely
//! positioned over the title bar, `occlude()`, mouse-down swallowed so the
//! Wayland drag handling never sees the spot. The strip is *non-interactive*
//! text and therefore lives inside the [`TitleBar`]'s children where window
//! dragging keeps working over it (the flight panel it will focus arrives
//! with the next phase).
//!
//! Every flight-replacing entry (New, Open…, Recent) and Close Flight run
//! through the dirty guard: unsaved changes prompt Save / Discard / Cancel
//! before the document is replaced or closed.

mod library;
mod model;

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Entity, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _, TITLE_BAR_HEIGHT, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
};

use crate::app::RootView;
use crate::assets::IconName;
use library::open_library_dialog;
use model::MenuModel;

/// Left edge of the Flight slot: clear of the title-bar icon + app title
/// (`pl(12)` + 16 px icon + `gap_2` + "Strata" in `text_sm`), with a small
/// margin before the menu.
const FLIGHT_SLOT_LEFT_PX: f32 = 96.;

/// Right margin of the flight strip: clears the overlaid settings gear +
/// sun/moon toggle (2 × 24 px small buttons + gaps) left of the window
/// controls — the strip is a `TitleBar` child, so the 3 control buttons
/// themselves are already outside its row.
const STRIP_RIGHT_CLEARANCE_PX: f32 = 72.;

/// The continuation a guarded menu entry runs once unsaved changes are
/// dealt with (shared by the Save / Discard dialog buttons).
type GuardedAction = Rc<dyn Fn(&mut RootView, &mut Window, &mut Context<RootView>)>;

/// A dirty-dialog button continuation. The dialog buttons run at the
/// window level, so these get the bare `App` context.
type DialogAction = Rc<dyn Fn(&mut Window, &mut gpui::App)>;

// --- title bar pieces -------------------------------------------------------

/// "Flight" text menu button in its CSD-safe title-bar slot.
pub fn render_flight_menu(root: &RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let state = root.app_state.read(cx);
    let menu_model = model::menu_model(state.flight.as_ref().map(|f| f.dirty));
    let recent = state.config.recent_flights.clone();
    let root_entity = cx.entity();
    div()
        .id("flight-menu-slot")
        .occlude()
        .absolute()
        .top_0()
        .left(px(FLIGHT_SLOT_LEFT_PX))
        .h(TITLE_BAR_HEIGHT)
        .flex()
        .items_center()
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .child(
            Button::new("flight-menu")
                .ghost()
                .small()
                .label("Flight")
                .child(
                    Icon::new(IconName::ChevronDown)
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                )
                .dropdown_menu(move |menu, window, cx| {
                    build_menu(menu, &root_entity, menu_model, &recent, window, cx)
                }),
        )
}

/// Compact flight summary beside the menu while a flight is open —
/// `"EDFE → EDQN · D-EABC · 09:30Z"` plus the unsaved-changes dot.
/// Deliberately non-interactive for now: clicking it will focus the flight
/// panel once that lands with the next phase.
pub fn render_flight_strip(
    root: &RootView,
    cx: &mut Context<RootView>,
) -> Option<impl IntoElement + use<>> {
    let state = root.app_state.read(cx);
    let flight = state.flight.as_ref()?;
    let registration = state.flight_aircraft().map(|p| p.registration.as_str());
    let text = model::strip_text(&flight.doc, registration);
    let dirty = flight.dirty;
    Some(
        h_flex()
            .gap_2()
            .items_center()
            .mr(px(STRIP_RIGHT_CLEARANCE_PX))
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .when(dirty, |el| {
                el.child(
                    div()
                        .size(px(6.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(cx.theme().warning),
                )
            })
            .child(text),
    )
}

// --- the popup menu ---------------------------------------------------------

fn build_menu(
    menu: PopupMenu,
    root: &Entity<RootView>,
    menu_model: MenuModel,
    recent: &[PathBuf],
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let mut menu = menu
        .item(PopupMenuItem::new("New Flight").on_click({
            let root = root.clone();
            move |_, window, cx| {
                root.update(cx, |this, cx| new_flight(this, window, cx));
            }
        }))
        .item(PopupMenuItem::new("Open…").on_click({
            let root = root.clone();
            move |_, window, cx| {
                root.update(cx, |this, cx| open_library_dialog(this, window, cx));
            }
        }))
        .submenu("Recent", window, cx, {
            let root = root.clone();
            let recent = recent.to_vec();
            move |menu, _, _| build_recent_submenu(menu, &root, &recent)
        })
        .separator()
        .item(PopupMenuItem::new("Manage Aircraft…").on_click({
            let root = root.clone();
            move |_, window, cx| {
                root.update(cx, |this, cx| {
                    crate::ui::aircraft_manager::open_aircraft_manager(this, window, cx);
                });
            }
        }));

    if menu_model.document_section {
        menu = menu
            .separator()
            .item(
                PopupMenuItem::new("Save")
                    .disabled(!menu_model.save_enabled)
                    .on_click({
                        let root = root.clone();
                        move |_, window, cx| {
                            root.update(cx, |this, cx| save_flight(this, window, cx));
                        }
                    }),
            )
            .item(PopupMenuItem::new("Save As…").on_click({
                let root = root.clone();
                move |_, window, cx| {
                    root.update(cx, |this, cx| save_flight_as(this, window, cx));
                }
            }))
            .item(PopupMenuItem::new("Duplicate").on_click({
                let root = root.clone();
                move |_, _, cx| {
                    root.update(cx, duplicate_flight);
                }
            }))
            .submenu("Export", window, cx, {
                let root = root.clone();
                move |menu, _, _| build_export_submenu(menu, &root)
            })
            .separator()
            .item(PopupMenuItem::new("Close Flight").on_click({
                let root = root.clone();
                move |_, window, cx| {
                    root.update(cx, |this, cx| close_flight(this, window, cx));
                }
            }));
    }
    menu
}

/// Export ▸ Briefing PDF… / ICAO FPL… (design §3.4 / §4): the PDF entry
/// starts the convert → background-render → save flow; the FPL entry opens
/// the preview/validation dialog. Both surfaces are also on the Briefing
/// tab — the menu is the keyboard-near path.
fn build_export_submenu(menu: PopupMenu, root: &Entity<RootView>) -> PopupMenu {
    menu.item(PopupMenuItem::new("Briefing PDF…").on_click({
        let root = root.clone();
        move |_, _, cx| {
            let app_state = root.read(cx).app_state.clone();
            crate::ui::context_tabs::briefing::start_pdf_export(&app_state, cx);
        }
    }))
    .item(PopupMenuItem::new("ICAO FPL…").on_click({
        let root = root.clone();
        move |_, window, cx| {
            let app_state = root.read(cx).app_state.clone();
            crate::ui::context_tabs::briefing::open_fpl_dialog(app_state, window, cx);
        }
    }))
}

fn build_recent_submenu(menu: PopupMenu, root: &Entity<RootView>, recent: &[PathBuf]) -> PopupMenu {
    if recent.is_empty() {
        return menu.item(PopupMenuItem::new("No recent flights").disabled(true));
    }
    recent.iter().fold(menu, |menu, path| {
        let root = root.clone();
        let path = path.clone();
        menu.item(
            PopupMenuItem::new(model::recent_label(&path)).on_click(move |_, window, cx| {
                let path = path.clone();
                root.update(cx, |this, cx| open_flight_guarded(this, path, window, cx));
            }),
        )
    })
}

// --- menu actions (shared with the library dialog) ---------------------------

/// New Flight: empty document, departure rounded up to the next 10 minutes,
/// the first library aircraft preselected. Guarded against unsaved changes.
pub(crate) fn new_flight(this: &mut RootView, window: &mut Window, cx: &mut Context<RootView>) {
    confirm_discard_if_dirty(this, window, cx, |this, _, cx| {
        this.app_state.update(cx, |state, cx| {
            let aircraft = state.aircraft_library.first().map(|p| p.id.clone());
            let now = chrono::Utc::now();
            state.new_flight("", cx);
            state.edit_flight_doc(cx, |doc| {
                model::apply_new_flight_defaults(doc, now, aircraft)
            });
            // The seeded defaults are part of "fresh document", not user
            // edits: an untouched new flight must not wear the unsaved
            // dot or trip the discard guard on the next menu action.
            if let Some(flight) = &mut state.flight {
                flight.dirty = false;
                cx.emit(crate::state::AppStateEvent::FlightChanged);
                cx.notify();
            }
        });
    });
}

/// Open a library/recent flight, prompting for unsaved changes first.
pub(crate) fn open_flight_guarded(
    this: &mut RootView,
    path: PathBuf,
    window: &mut Window,
    cx: &mut Context<RootView>,
) {
    confirm_discard_if_dirty(this, window, cx, move |this, _, cx| {
        this.app_state
            .update(cx, |state, cx| state.open_flight(path.clone(), cx));
    });
}

/// Save: to the document's path, or fall through to Save As… while it has
/// none (a brand-new flight).
fn save_flight(this: &mut RootView, window: &mut Window, cx: &mut Context<RootView>) {
    let saved = this.app_state.update(cx, |state, cx| state.save_flight(cx));
    if !saved {
        save_flight_as(this, window, cx);
    }
}

/// Save As…: the platform save dialog (XDG portal), seeded with the
/// library directory and the slugged default file name; the picked path
/// gets the flight extension forced on. Forcing the extension can land
/// the path on an existing flight file the portal never warned about —
/// such collisions get a `" (2)"`-style suffix instead of a silent
/// overwrite (saving onto the document's own file stays a plain save).
fn save_flight_as(this: &mut RootView, _window: &mut Window, cx: &mut Context<RootView>) {
    let Some(default_path) = this.app_state.read(cx).default_flight_path() else {
        return; // no flight open
    };
    let directory = default_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let suggested = default_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let receiver = cx.prompt_for_new_path(&directory, suggested.as_deref());
    cx.spawn(async move |this, cx| {
        // Outer Err = channel dropped, inner Err = portal failure, None =
        // cancelled — none of them save anything.
        let Ok(Ok(Some(path))) = receiver.await else {
            return;
        };
        let path = model::with_flight_extension(path);
        this.update(cx, |this, cx| {
            this.app_state.update(cx, |state, cx| {
                let current = state.flight.as_ref().and_then(|f| f.path.clone());
                let path = crate::flight_io::dedupe_flight_path(path, current.as_deref());
                state.save_flight_as(path, cx);
            });
        })
        .ok();
    })
    .detach();
}

/// Duplicate: reopen the current document (including unsaved edits) as a
/// new, pathless, dirty flight named "… (copy)". The original file keeps
/// its on-disk state, so no dirty guard is needed.
fn duplicate_flight(this: &mut RootView, cx: &mut Context<RootView>) {
    this.app_state.update(cx, |state, cx| {
        let Some(flight) = &state.flight else {
            return;
        };
        let mut doc = flight.doc.clone();
        doc.name = model::duplicate_name(&doc.name);
        state.new_flight("", cx);
        state.edit_flight_doc(cx, |target| {
            *target = doc;
            true
        });
    });
}

/// Close Flight — back to the clean explorer; unsaved changes prompt first.
fn close_flight(this: &mut RootView, window: &mut Window, cx: &mut Context<RootView>) {
    confirm_discard_if_dirty(this, window, cx, |this, _, cx| {
        this.app_state
            .update(cx, |state, cx| state.close_flight(cx));
    });
}

// --- the dirty guard ----------------------------------------------------------

/// Runs `then` immediately while the open flight is clean (or absent);
/// otherwise raises the Save / Discard / Cancel dialog and runs `then`
/// after Save or Discard. The guard's Save writes to the document's path —
/// or silently to the library default path while it has none (a Save As…
/// portal round-trip would outlive the close gesture).
fn confirm_discard_if_dirty(
    this: &mut RootView,
    window: &mut Window,
    cx: &mut Context<RootView>,
    then: impl Fn(&mut RootView, &mut Window, &mut Context<RootView>) + 'static,
) {
    if !window_close_needs_guard(this, cx) {
        then(this, window, cx);
        return;
    }
    let root = cx.entity();
    let then: GuardedAction = Rc::new(then);
    let save: DialogAction = {
        let root = root.clone();
        let then = then.clone();
        Rc::new(move |window, cx| {
            root.update(cx, |this, cx| {
                save_to_known_path(this, cx);
                then(this, window, cx);
            });
        })
    };
    let discard: DialogAction = Rc::new(move |window, cx| {
        root.update(cx, |this, cx| then(this, window, cx));
    });
    open_dirty_dialog(window, cx, save, discard);
}

/// Whether closing the window (or replacing the document) must prompt:
/// a flight is open and has unsaved changes.
pub(crate) fn window_close_needs_guard(this: &RootView, cx: &Context<RootView>) -> bool {
    this.app_state
        .read(cx)
        .flight
        .as_ref()
        .is_some_and(|f| f.dirty)
}

/// The close-window guard — the CSD X button and the compositor close
/// both funnel here. Clean (or no) flight: the window closes immediately.
/// Dirty: the shared Save / Discard / Cancel dialog, where Discard closes
/// the window and Save **awaits the write** before closing it — process
/// exit does not wait for detached background tasks, so a detached save
/// racing the window teardown would be silently lost.
pub(crate) fn request_close_window(
    this: &mut RootView,
    window: &mut Window,
    cx: &mut Context<RootView>,
) {
    if !window_close_needs_guard(this, cx) {
        window.remove_window();
        return;
    }
    let app_state = this.app_state.clone();
    let save: DialogAction = Rc::new(move |window, cx| {
        let task = app_state.update(cx, |state, cx| state.save_flight_to_known_path(cx));
        let handle = window.window_handle();
        cx.spawn(async move |cx| {
            if let Some(task) = task {
                task.await;
            }
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        })
        .detach();
    });
    let discard: DialogAction = Rc::new(|window, _| window.remove_window());
    open_dirty_dialog(window, cx, save, discard);
}

/// The shared Save / Discard / Cancel dialog body (the menu guard and the
/// close-window guard differ only in their continuations; Cancel always
/// just dismisses the dialog and keeps everything as it was).
fn open_dirty_dialog(
    window: &mut Window,
    cx: &mut Context<RootView>,
    save: DialogAction,
    discard: DialogAction,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let save = save.clone();
        let discard = discard.clone();
        alert
            .title("Unsaved changes")
            .description("The open flight has unsaved changes.")
            .footer(
                DialogFooter::new()
                    // A plain button, not `DialogClose` — that wrapper is
                    // `size_full()` and would stretch Cancel across the
                    // footer's free space.
                    .child(
                        Button::new("dirty-cancel")
                            .outline()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("dirty-discard")
                            .danger()
                            .label("Discard")
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                discard(window, cx);
                            }),
                    )
                    .child(Button::new("dirty-save").primary().label("Save").on_click(
                        move |_, window, cx| {
                            window.close_dialog(cx);
                            save(window, cx);
                        },
                    )),
            )
    });
}

/// The guard's Save: document path, or the deduplicated library default
/// for never-saved flights (the close-window guard awaits the same write
/// via [`crate::state::AppState::save_flight_to_known_path`] instead of
/// detaching it).
fn save_to_known_path(this: &mut RootView, cx: &mut Context<RootView>) {
    this.app_state.update(cx, |state, cx| {
        if let Some(task) = state.save_flight_to_known_path(cx) {
            task.detach();
        }
    });
}
