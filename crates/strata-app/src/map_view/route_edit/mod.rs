//! Route editing on the map (design §3.2): waypoint-handle drags with
//! feature snapping, rubber-band inserts on a leg, and the planning-mode
//! right-click menu. Active **only** while a flight is open — in explorer
//! mode every press falls through to the untouched pan/click path.
//!
//! Hit-testing is app-side and screen-space (plan §4: the renderer draws,
//! it never picks): the [`hit::Projection`] frozen from the live camera
//! projects the pushed [`RenderRoute`]'s vertices — whose ids are the
//! contract with `state::flight::render_route` — and the [`machine`]
//! decides click vs drag with the same 4 px slop as the pan-click
//! detection. During an active drag the document is untouched; the moved
//! or spliced [`ghost`] route feeds the renderer, and only the release
//! commits through the `AppState` mutation API (which re-renders the route
//! via `FlightChanged` with byte-identical geometry, so the handoff never
//! flickers). Dragged points snap to airports/navaids/reporting points
//! within [`SNAP_RADIUS_PX`] (see [`snap`]); while the ghost locks onto a
//! candidate the push seam also sets [`RenderRoute::snap_ring`] so the
//! renderer pulses a ring on the target. A snapped release commits a
//! re-resolvable `NamedPoint`, a free release a plain coordinate.

mod ghost;
mod hit;
mod machine;
mod snap;

use gpui::{App, AppContext as _, Context, Entity, MouseDownEvent, Task, Window};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use strata_data::domain::{LatLon as GeoLatLon, Meters};
use strata_plan::flight::{FreePoint, RoutePoint};
use strata_render::RenderRoute;
use strata_render::glam::DVec2;

use crate::state::flight::render_route::ALTERNATE_ID_BASE;

use super::{CLICK_SLOP_PX, MapView, feed_bbox};
use hit::{Projection, RouteHit};
use machine::{DragOutcome, Gesture, RouteDrag};
use snap::SnapCandidate;

/// Handle pickup radius in logical px (the handles draw at 6.5–9 px, so a
/// near miss still grabs).
const HANDLE_HIT_RADIUS_PX: f64 = 10.0;
/// Rubber-band pickup distance from a leg line (design §3.2: ~8 px, and
/// never inside a handle's radius).
const LEG_HIT_RADIUS_PX: f64 = 8.0;
/// Drag snapping radius — screen-fixed, hence zoom-scaled on the ground.
const SNAP_RADIUS_PX: f64 = 12.0;

/// Everything the route-editing interactions keep between events.
#[derive(Default)]
pub(super) struct RouteEditState {
    /// The captured gesture between mouse-down and mouse-up, if any.
    drag: Option<RouteDrag>,
    /// Ghost position (`[lon, lat]`) once the drag is active — already
    /// snapped when [`Self::snap`] is set.
    ghost_pos: Option<[f64; 2]>,
    /// The candidate the ghost currently locks onto.
    snap: Option<SnapCandidate>,
    /// Snap targets near the viewport, loaded once per drag.
    candidates: Vec<SnapCandidate>,
    candidates_task: Option<Task<()>>,
    /// The cursor is over a handle (hover affordance).
    hover_handle: bool,
    /// The cursor sits on the route polyline and owns the shared profile
    /// scrub. Only the on→off transition clears the scrub, so a scrub set
    /// elsewhere (badge navigation, the drawer) survives unrelated mouse
    /// travel across the map.
    scrub_hover: bool,
    /// Position of the last planning-mode right click (the context-menu
    /// anchor and the position its actions operate on).
    context_click: Option<ContextClick>,
}

impl RouteEditState {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Where a context menu was opened: local px for leg math, geo for the
/// created point (`None` when the renderer was unavailable to pick).
#[derive(Clone, Copy)]
struct ContextClick {
    local_px: DVec2,
    geo: Option<GeoLatLon>,
}

impl MapView {
    // --- gesture lifecycle (called from the mouse handlers) -----------------

    /// Captures a left press on a handle or leg (planning mode only).
    /// Returns whether the press is ours — the caller then never starts a
    /// map pan; presses elsewhere keep today's behavior exactly.
    pub(super) fn begin_route_gesture(&mut self, local: DVec2, cx: &mut Context<Self>) -> bool {
        if !self.app_state.read(cx).planning_mode() {
            return false;
        }
        let Some(route) = &self.route else {
            return false;
        };
        let Some(proj) = self.route_projection() else {
            return false;
        };
        let Some(hit) = hit::hit_test(route, &proj, local) else {
            return false;
        };
        let gesture = match hit {
            RouteHit::Handle(id) => Gesture::Handle { id },
            RouteHit::Leg(index) => Gesture::Leg { index },
        };
        self.route_edit.drag = Some(RouteDrag::new(gesture, local));
        self.route_edit.ghost_pos = None;
        self.route_edit.snap = None;
        self.load_snap_candidates(cx);
        true
    }

    /// Whether a route gesture (pending or active) owns the mouse.
    pub(super) fn route_gesture_active(&self) -> bool {
        self.route_edit.drag.is_some()
    }

    /// Feeds a cursor move into the captured gesture: past the click slop
    /// the drag activates and the ghost route (with snapping) tracks the
    /// cursor.
    pub(super) fn update_route_gesture(&mut self, local: DVec2, cx: &mut Context<Self>) {
        let Some(drag) = self.route_edit.drag.as_mut() else {
            return;
        };
        if !drag.moved(local, CLICK_SLOP_PX) {
            return; // still a potential click — no ghost yet
        }
        let Some((proj, geo)) = self.projection_and_pick(local) else {
            return;
        };
        let snapped =
            snap::nearest_snap(&self.route_edit.candidates, &proj, local, SNAP_RADIUS_PX).cloned();
        let pos = match &snapped {
            Some(candidate) => [candidate.position.lon(), candidate.position.lat()],
            None => [geo.lon_deg(), geo.lat_deg()],
        };
        self.route_edit.snap = snapped;
        if self.route_edit.ghost_pos != Some(pos) {
            self.route_edit.ghost_pos = Some(pos);
            self.push_route_to_renderer(cx);
        }
    }

    /// Releases the captured gesture. Returns `true` when it never left
    /// the click slop — the caller then runs the plain selection click,
    /// exactly as today. Active drags commit through the document API; the
    /// `FlightChanged` round-trip re-pushes geometry identical to the
    /// final ghost, so the renderer stays put.
    pub(super) fn finish_route_gesture(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.route_edit.drag.take() else {
            return false;
        };
        let snapped = self.route_edit.snap.take();
        let ghost_pos = self.route_edit.ghost_pos.take();
        self.route_edit.candidates = Vec::new();
        self.route_edit.candidates_task = None;

        let point = ghost_pos.and_then(|pos| commit_point(snapped, pos));
        match (drag.outcome(), point) {
            (DragOutcome::Click, _) => {
                self.push_route_to_renderer(cx);
                return true;
            }
            (DragOutcome::MoveHandle { id }, Some(point)) => self.commit_handle_move(id, point, cx),
            (DragOutcome::InsertIntoLeg { index }, Some(point)) => {
                let at = hit::insert_index(index);
                self.app_state.update(cx, |state, cx| {
                    state.insert_waypoint(at, point, cx);
                });
            }
            // An active drag without a resolvable position (renderer gone
            // mid-drag): drop the ghost, commit nothing.
            (_, None) => {}
        }
        self.push_route_to_renderer(cx);
        false
    }

    /// Hover affordance: grab cursor over handles while nothing drags.
    pub(super) fn update_route_hover(&mut self, local: DVec2, cx: &mut Context<Self>) {
        let hover = self.app_state.read(cx).planning_mode()
            && self
                .route
                .as_ref()
                .zip(self.route_projection())
                .is_some_and(|(route, proj)| {
                    hit::nearest_handle(route, &proj, local, HANDLE_HIT_RADIUS_PX).is_some()
                });
        if self.route_edit.hover_handle != hover {
            self.route_edit.hover_handle = hover;
            cx.notify();
        }
        self.update_route_scrub(local, cx);
    }

    /// The reverse of the profile drawer's hover-scrub (design §3.2 "a dot
    /// on the track mirroring the profile drawer's cursor — and vice
    /// versa"): the cursor resting on the route polyline moves the shared
    /// scrub, which the drawer's crosshair follows. `set_profile_scrub`
    /// dedupes, so dwelling on one spot stays event-free.
    fn update_route_scrub(&mut self, local: DVec2, cx: &mut Context<Self>) {
        match self.route_scrub_at(local, cx) {
            Some(along) => {
                self.route_edit.scrub_hover = true;
                self.app_state.update(cx, |state, cx| {
                    state.set_profile_scrub(Some(Meters(along)), cx);
                });
            }
            None if self.route_edit.scrub_hover => {
                self.route_edit.scrub_hover = false;
                self.app_state
                    .update(cx, |state, cx| state.set_profile_scrub(None, cx));
            }
            None => {}
        }
    }

    /// Along-track meters of the cursor's projection onto the nearest leg,
    /// when the cursor is within the leg hit radius. Measured with the
    /// computed legs' geodesic distances — the same axis the profile and
    /// the renderer's marker use. `None` without a computed flight (no
    /// profile to mirror) or with a stale leg mismatch (transient; the
    /// landing recompute resolves it).
    fn route_scrub_at(&self, local: DVec2, cx: &Context<Self>) -> Option<f64> {
        let state = self.app_state.read(cx);
        let computed = state.flight.as_ref()?.computed.as_ref()?;
        let route = self.route.as_ref()?;
        let proj = self.route_projection()?;
        let (leg, distance, fraction) = hit::nearest_leg_projection(route, &proj, local)?;
        if distance > LEG_HIT_RADIUS_PX {
            return None;
        }
        let leg_len = computed.legs.get(leg)?.distance.0;
        let before: f64 = computed.legs.get(..leg)?.iter().map(|l| l.distance.0).sum();
        Some(before + fraction * leg_len)
    }

    /// Whether a drag is past the slop (grabbing cursor).
    pub(super) fn route_drag_engaged(&self) -> bool {
        self.route_edit.drag.as_ref().is_some_and(RouteDrag::active)
    }

    /// Whether the cursor rests on a handle (grab cursor).
    pub(super) fn route_handle_hovered(&self) -> bool {
        self.route_edit.hover_handle
    }

    // --- renderer push -------------------------------------------------------

    /// Pushes the effective route — the doc route with any active ghost
    /// applied — into the renderer's route layer. Identical routes keep
    /// the renderer idle.
    ///
    /// The scrub marker, the hover highlight, the corridor outline and the
    /// snap ring are overlaid here, at the push seam: all four are
    /// view/interaction state (`profile_scrub` — the drawer↔map sync
    /// point —, `route_highlight` — the flight panel's row hover —,
    /// `corridor_visible`, and the drag's current [`SnapCandidate`]), never
    /// part of the stored route — so every push path (doc route, drag
    /// ghost, scrub/highlight/corridor event) carries the current values. The outline
    /// width comes from the *computed* corridor, so the map shows exactly
    /// what the profile considered (design §3.2); the ring sits on the snap
    /// target the ghost currently locks onto, so it clears with the snap
    /// (and with the drag, which resets [`RouteEditState`]).
    pub(super) fn push_route_to_renderer(&mut self, cx: &mut Context<Self>) {
        let mut effective = self.effective_route();
        if let Some(route) = &mut effective {
            let state = self.app_state.read(cx);
            route.scrub_along_m = state.profile_scrub.map(|m| m.0);
            route.highlight = state.route_highlight;
            route.snap_ring = self
                .route_edit
                .snap
                .as_ref()
                .map(|candidate| [candidate.position.lon(), candidate.position.lat()]);
            route.corridor_halfwidth_m = state
                .corridor_visible
                .then(|| {
                    state
                        .flight
                        .as_ref()?
                        .computed
                        .as_ref()
                        .map(|computed| computed.corridor.params.half_width.0)
                })
                .flatten();
        }
        if let Some(cell) = &self.cell {
            cell.lock().renderer.set_route(effective);
            cx.notify();
        }
    }

    fn effective_route(&self) -> Option<RenderRoute> {
        let route = self.route.as_ref()?;
        let (Some(drag), Some(pos)) = (&self.route_edit.drag, self.route_edit.ghost_pos) else {
            return Some(route.clone());
        };
        Some(match drag.gesture() {
            Gesture::Handle { id } => ghost::move_vertex(route, id, pos),
            Gesture::Leg { index } => ghost::insert_into_leg(route, index, pos),
        })
    }

    /// Route cleared (flight closed): drop every interaction leftover so
    /// the explorer keeps no route-editing residue.
    pub(super) fn reset_route_edit(&mut self) {
        self.route_edit.reset();
    }

    // --- helpers ---------------------------------------------------------------

    /// Projection from the live camera, plus the geo pick of `local` —
    /// both under one lock so they describe the same camera pose.
    fn projection_and_pick(&self, local: DVec2) -> Option<(Projection, strata_render::LatLon)> {
        let cell = self.cell.as_ref()?;
        let viewport = DVec2::new(
            f64::from(self.bounds.size.width),
            f64::from(self.bounds.size.height),
        );
        if !viewport.cmpge(DVec2::ONE).all() {
            return None;
        }
        let cell = cell.lock();
        Some((
            Projection::new(&cell.renderer.camera(), viewport),
            cell.renderer.pick(local),
        ))
    }

    fn route_projection(&self) -> Option<Projection> {
        self.projection_and_pick(DVec2::ZERO).map(|(proj, _)| proj)
    }

    /// One bulk snap-candidate load per drag: the camera cannot move while
    /// a route gesture owns the mouse, so the (margin-expanded) viewport
    /// envelope stays valid for the whole drag.
    fn load_snap_candidates(&mut self, cx: &mut Context<Self>) {
        self.route_edit.candidates = Vec::new();
        let Some(store) = self.app_state.read(cx).store.clone() else {
            return;
        };
        let Some(snapshot) = self.last_pushed_camera else {
            return;
        };
        let Some(bbox) = feed_bbox(&snapshot) else {
            return;
        };
        let zoom = snapshot.zoom;
        self.route_edit.candidates_task = Some(cx.spawn(async move |this, cx| {
            let candidates = cx
                .background_spawn(async move { snap::query_candidates(&store, bbox, zoom) })
                .await;
            this.update(cx, |this, _| {
                // Only the drag that requested them still wants them.
                if this.route_edit.drag.is_some() {
                    this.route_edit.candidates = candidates;
                }
            })
            .ok();
        }));
    }

    fn commit_handle_move(&mut self, id: u64, point: RoutePoint, cx: &mut Context<Self>) {
        if id >= ALTERNATE_ID_BASE {
            let index = (id - ALTERNATE_ID_BASE) as usize;
            self.app_state.update(cx, |state, cx| {
                state.edit_flight_doc(cx, |doc| match doc.alternates.get_mut(index) {
                    Some(slot) if *slot != point => {
                        *slot = point;
                        true
                    }
                    _ => false,
                });
            });
        } else {
            self.app_state.update(cx, |state, cx| {
                state.replace_waypoint_point(id as usize, point, cx);
            });
        }
    }

    // --- right-click context menu ---------------------------------------------

    /// Records where a planning-mode right click landed; the context menu
    /// (which opens deferred) reads it back for its actions.
    pub(super) fn on_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local = self.local_px(event.position);
        let geo = self
            .cell
            .as_ref()
            .map(|cell| cell.lock().renderer.pick(local));
        let geo = geo.and_then(|g| GeoLatLon::new(g.lat_deg(), g.lon_deg()).ok());
        self.route_edit.context_click = Some(ContextClick {
            local_px: local,
            geo,
        });
        cx.notify();
    }

    fn context_point(&self) -> Option<RoutePoint> {
        let position = self.route_edit.context_click?.geo?;
        Some(RoutePoint::Free(FreePoint {
            name: None,
            position,
        }))
    }

    fn context_append_waypoint(&mut self, cx: &mut Context<Self>) {
        let Some(point) = self.context_point() else {
            return;
        };
        self.app_state.update(cx, |state, cx| {
            state.append_waypoint(point, cx);
        });
    }

    fn context_insert_into_nearest_leg(&mut self, cx: &mut Context<Self>) {
        let Some(point) = self.context_point() else {
            return;
        };
        let Some(click) = self.route_edit.context_click else {
            return;
        };
        let Some(route) = &self.route else {
            return;
        };
        let Some(proj) = self.route_projection() else {
            return;
        };
        let Some((leg, _)) = hit::nearest_leg(route, &proj, click.local_px) else {
            return;
        };
        let at = hit::insert_index(leg);
        self.app_state.update(cx, |state, cx| {
            state.insert_waypoint(at, point, cx);
        });
    }

    fn context_set_alternate(&mut self, cx: &mut Context<Self>) {
        let Some(point) = self.context_point() else {
            return;
        };
        self.app_state.update(cx, |state, cx| {
            state.set_alternate(Some(point), cx);
        });
    }
}

/// The map's planning-mode context menu (design §3.2): append / insert
/// into nearest leg / set as alternate, all operating on the recorded
/// right-click position. Explorer mode never builds this — the element
/// isn't even attached there.
pub(super) fn build_route_context_menu(
    menu: PopupMenu,
    map: &Entity<MapView>,
    cx: &App,
) -> PopupMenu {
    let view = map.read(cx);
    let has_point = view
        .route_edit
        .context_click
        .is_some_and(|click| click.geo.is_some());
    let has_leg = view
        .route
        .as_ref()
        .is_some_and(|route| hit::main_track(route).count() >= 2);

    menu.item(
        PopupMenuItem::new("Append waypoint")
            .disabled(!has_point)
            .on_click({
                let map = map.clone();
                move |_, _, cx| {
                    map.update(cx, |this, cx| this.context_append_waypoint(cx));
                }
            }),
    )
    .item(
        PopupMenuItem::new("Insert into nearest leg")
            .disabled(!(has_point && has_leg))
            .on_click({
                let map = map.clone();
                move |_, _, cx| {
                    map.update(cx, |this, cx| this.context_insert_into_nearest_leg(cx));
                }
            }),
    )
    .item(
        PopupMenuItem::new("Set as alternate")
            .disabled(!has_point)
            .on_click({
                let map = map.clone();
                move |_, _, cx| {
                    map.update(cx, |this, cx| this.context_set_alternate(cx));
                }
            }),
    )
}

/// The committed route point: the snap target as a named, re-resolvable
/// point, or a free coordinate ("drag onto empty = free LatLon point").
fn commit_point(snapped: Option<SnapCandidate>, pos: [f64; 2]) -> Option<RoutePoint> {
    if let Some(candidate) = snapped {
        return Some(candidate.route_point());
    }
    GeoLatLon::new(pos[1], pos[0]).ok().map(|position| {
        RoutePoint::Free(FreePoint {
            name: None,
            position,
        })
    })
}

#[cfg(test)]
mod tests {
    use strata_plan::flight::NamedPointKind;

    use super::*;

    /// Snapped releases commit the named point; free releases the
    /// coordinate; an invalid coordinate (corrupt pick) commits nothing.
    #[test]
    fn commit_point_prefers_the_snap() {
        let candidate = SnapCandidate {
            kind: NamedPointKind::Airport,
            id: "EDQN".to_owned(),
            name: "Neustadt/Aisch".to_owned(),
            position: GeoLatLon::new(49.58, 10.58).unwrap(),
        };
        match commit_point(Some(candidate), [99.0, 99.0]) {
            Some(RoutePoint::Named(named)) => assert_eq!(named.id, "EDQN"),
            other => panic!("expected the named snap point, got {other:?}"),
        }

        match commit_point(None, [10.5, 49.5]) {
            Some(RoutePoint::Free(free)) => {
                assert_eq!(free.position, GeoLatLon::new(49.5, 10.5).unwrap());
                assert!(free.name.is_none());
            }
            other => panic!("expected a free point, got {other:?}"),
        }

        assert!(commit_point(None, [10.5, 200.0]).is_none());
    }
}
