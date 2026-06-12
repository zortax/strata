//! The per-profile editor: one persistent `Entity<InputState>` per field,
//! each bound through a commit closure into the manager's draft (typed
//! edits commit on every parseable change — the save funnel is
//! [`AppState::upsert_aircraft_profile`]'s diff-cheap atomic write), plus
//! the section renderers styled like the settings modal's groups.
//!
//! Structural edits (cruise/station rows added or removed, profile switch)
//! rebuild the whole [`ProfileEditor`]; value edits never do, so focus and
//! caret survive typing.
//!
//! [`AppState::upsert_aircraft_profile`]: crate::state::AppState::upsert_aircraft_profile

use std::rc::Rc;

use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _, SharedString,
    Styled as _, Subscription, Window, div,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::group_box::GroupBox;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use strata_data::domain::Meters;
use strata_plan::AircraftProfile;
use strata_plan::aircraft::{FuelType, StationKind};
use strata_plan::units::{
    FeetPerMinute, Kilograms, KilogramsPerLiter, Knots, Liters, LitersPerHour,
};

use crate::assets::IconName;

use super::AircraftManagerView;
use super::envelope::{EnvelopeEditor, EnvelopeEvent};
use super::fields::{NumOpts, format_num, parse_num};

// --- field ranges (clamps; design §3.5 "numeric clamps") ---------------------

const SPEED_KT: NumOpts = NumOpts::positive(999.0, 1);
const RATE_FT_MIN: NumOpts = NumOpts::positive(9999.0, 0);
const FLOW_L_H: NumOpts = NumOpts::positive(999.0, 1);
const VOLUME_L: NumOpts = NumOpts::positive(99_999.0, 1);
const DENSITY_KG_L: NumOpts = NumOpts::new(0.0, 10.0, 3);
const MASS_KG: NumOpts = NumOpts::positive(999_999.0, 1);
/// Arms may be negative — forward of the datum.
const ARM_M: NumOpts = NumOpts::new(-99.0, 99.0, 3);
const DISTANCE_M: NumOpts = NumOpts::positive(99_999.0, 0);
const SAFETY_FACTOR: NumOpts = NumOpts::new(0.0, 10.0, 2);
/// Correction factors are fractional increases; negative decreases
/// (headwind's template is −0.10).
const CORRECTION_FACTOR: NumOpts = NumOpts::new(-1.0, 5.0, 2);

// --- the editor ---------------------------------------------------------------

pub(super) struct CruiseRow {
    pub name: Entity<InputState>,
    pub tas: Entity<InputState>,
    pub flow: Entity<InputState>,
}

pub(super) struct StationRow {
    pub name: Entity<InputState>,
    pub arm: Entity<InputState>,
    pub max_load: Entity<InputState>,
}

/// All input entities of the open profile. Recreated on profile switch and
/// on structural row changes; field edits only mutate the draft.
pub(super) struct ProfileEditor {
    pub name: Entity<InputState>,
    pub registration: Entity<InputState>,
    pub type_designator: Entity<InputState>,
    pub callsign: Entity<InputState>,

    pub cruise_rows: Vec<CruiseRow>,
    pub climb_ias: Entity<InputState>,
    pub climb_rate: Entity<InputState>,
    pub climb_flow: Entity<InputState>,
    pub descent_ias: Entity<InputState>,
    pub descent_rate: Entity<InputState>,
    pub descent_flow: Entity<InputState>,
    pub taxi_flow: Entity<InputState>,

    pub fuel_usable: Entity<InputState>,
    pub fuel_tabs: Entity<InputState>,
    pub fuel_density: Entity<InputState>,

    pub empty_mass: Entity<InputState>,
    pub empty_arm: Entity<InputState>,
    pub max_takeoff: Entity<InputState>,
    pub max_landing: Entity<InputState>,
    pub max_zero_fuel: Entity<InputState>,
    pub max_ramp: Entity<InputState>,
    pub station_rows: Vec<StationRow>,
    pub envelope: Entity<EnvelopeEditor>,

    pub takeoff_roll: Entity<InputState>,
    pub takeoff_50: Entity<InputState>,
    pub landing_roll: Entity<InputState>,
    pub landing_50: Entity<InputState>,
    pub takeoff_safety: Entity<InputState>,
    pub landing_safety: Entity<InputState>,
    pub factor_da: Entity<InputState>,
    pub factor_headwind: Entity<InputState>,
    pub factor_tailwind: Entity<InputState>,
    pub factor_grass: Entity<InputState>,
    pub factor_wet: Entity<InputState>,
    pub factor_slope: Entity<InputState>,

    _subscriptions: Vec<Subscription>,
}

impl ProfileEditor {
    pub fn new(
        profile: &AircraftProfile,
        window: &mut Window,
        cx: &mut Context<AircraftManagerView>,
    ) -> Self {
        let mut subs = Vec::new();
        let p = profile;

        // -- identity --------------------------------------------------------
        let name = text_field(
            window,
            cx,
            &mut subs,
            p.name.as_deref().unwrap_or_default(),
            "Display name",
            |p, v| p.name = (!v.is_empty()).then(|| v.to_owned()),
        );
        let registration = text_field(window, cx, &mut subs, &p.registration, "D-EABC", |p, v| {
            p.registration = v.to_owned();
        });
        // Stored as typed; the FPL generator uppercases at the edge.
        let type_designator =
            text_field(window, cx, &mut subs, &p.type_designator, "C172", |p, v| {
                p.type_designator = v.to_owned();
            });
        let callsign = text_field(window, cx, &mut subs, &p.callsign, "Registration", |p, v| {
            p.callsign = v.to_owned();
        });

        // -- performance ------------------------------------------------------
        let cruise_rows = p
            .performance
            .cruise_settings
            .iter()
            .enumerate()
            .map(|(ix, setting)| CruiseRow {
                name: text_field(window, cx, &mut subs, &setting.name, "65 %", move |p, v| {
                    if let Some(s) = p.performance.cruise_settings.get_mut(ix) {
                        s.name = v.to_owned();
                    }
                }),
                tas: num_field(
                    window,
                    cx,
                    &mut subs,
                    p,
                    SPEED_KT,
                    move |p| p.performance.cruise_settings.get(ix).map(|s| s.tas.0),
                    move |p, v| {
                        if let Some(s) = p.performance.cruise_settings.get_mut(ix) {
                            s.tas = Knots(v.unwrap_or(0.0));
                        }
                    },
                ),
                flow: num_field(
                    window,
                    cx,
                    &mut subs,
                    p,
                    FLOW_L_H,
                    move |p| p.performance.cruise_settings.get(ix).map(|s| s.fuel_flow.0),
                    move |p, v| {
                        if let Some(s) = p.performance.cruise_settings.get_mut(ix) {
                            s.fuel_flow = LitersPerHour(v.unwrap_or(0.0));
                        }
                    },
                ),
            })
            .collect();

        let climb_ias = num_field(
            window, cx, &mut subs,
            p, SPEED_KT,
            |p| Some(p.performance.climb.ias.0),
            |p, v| p.performance.climb.ias = Knots(v.unwrap_or(0.0)),
        );
        let climb_rate = num_field(
            window, cx, &mut subs,
            p, RATE_FT_MIN,
            |p| Some(p.performance.climb.rate.0),
            |p, v| p.performance.climb.rate = FeetPerMinute(v.unwrap_or(0.0)),
        );
        let climb_flow = num_field(
            window, cx, &mut subs,
            p, FLOW_L_H,
            |p| Some(p.performance.climb.fuel_flow.0),
            |p, v| p.performance.climb.fuel_flow = LitersPerHour(v.unwrap_or(0.0)),
        );
        let descent_ias = num_field(
            window, cx, &mut subs,
            p, SPEED_KT,
            |p| Some(p.performance.descent.ias.0),
            |p, v| p.performance.descent.ias = Knots(v.unwrap_or(0.0)),
        );
        let descent_rate = num_field(
            window, cx, &mut subs,
            p, RATE_FT_MIN,
            |p| Some(p.performance.descent.rate.0),
            |p, v| p.performance.descent.rate = FeetPerMinute(v.unwrap_or(0.0)),
        );
        let descent_flow = num_field(
            window, cx, &mut subs,
            p, FLOW_L_H,
            |p| Some(p.performance.descent.fuel_flow.0),
            |p, v| p.performance.descent.fuel_flow = LitersPerHour(v.unwrap_or(0.0)),
        );
        let taxi_flow = num_field(
            window, cx, &mut subs,
            p, FLOW_L_H,
            |p| Some(p.performance.taxi_fuel_flow.0),
            |p, v| p.performance.taxi_fuel_flow = LitersPerHour(v.unwrap_or(0.0)),
        );

        // -- fuel --------------------------------------------------------------
        let fuel_usable = num_field(
            window, cx, &mut subs,
            p, VOLUME_L,
            |p| Some(p.fuel.usable.0),
            |p, v| p.fuel.usable = Liters(v.unwrap_or(0.0)),
        );
        let fuel_tabs = num_field(
            window, cx, &mut subs,
            p, VOLUME_L.optional(),
            |p| p.fuel.tabs.map(|t| t.0),
            |p, v| p.fuel.tabs = v.map(Liters),
        );
        let fuel_density = num_field(
            window, cx, &mut subs,
            p, DENSITY_KG_L,
            |p| Some(p.fuel.density.0),
            |p, v| p.fuel.density = KilogramsPerLiter(v.unwrap_or(0.0)),
        );

        // -- weight & balance ----------------------------------------------------
        let wb = &p.weight_balance;
        let empty_mass = num_field(
            window, cx, &mut subs,
            p, MASS_KG,
            |p| Some(p.weight_balance.empty_mass.0),
            |p, v| p.weight_balance.empty_mass = Kilograms(v.unwrap_or(0.0)),
        );
        let empty_arm = num_field(
            window, cx, &mut subs,
            p, ARM_M,
            |p| Some(p.weight_balance.empty_arm.0),
            |p, v| p.weight_balance.empty_arm = Meters(v.unwrap_or(0.0)),
        );
        let max_takeoff = num_field(
            window, cx, &mut subs,
            p, MASS_KG,
            |p| Some(p.weight_balance.max_takeoff.0),
            |p, v| p.weight_balance.max_takeoff = Kilograms(v.unwrap_or(0.0)),
        );
        let max_landing = num_field(
            window, cx, &mut subs,
            p, MASS_KG.optional(),
            |p| p.weight_balance.max_landing.map(|m| m.0),
            |p, v| p.weight_balance.max_landing = v.map(Kilograms),
        );
        let max_zero_fuel = num_field(
            window, cx, &mut subs,
            p, MASS_KG.optional(),
            |p| p.weight_balance.max_zero_fuel.map(|m| m.0),
            |p, v| p.weight_balance.max_zero_fuel = v.map(Kilograms),
        );
        let max_ramp = num_field(
            window, cx, &mut subs,
            p, MASS_KG.optional(),
            |p| p.weight_balance.max_ramp.map(|m| m.0),
            |p, v| p.weight_balance.max_ramp = v.map(Kilograms),
        );
        let station_rows = wb
            .stations
            .iter()
            .enumerate()
            .map(|(ix, station)| StationRow {
                name: text_field(window, cx, &mut subs, &station.name, "Station", move |p, v| {
                    if let Some(s) = p.weight_balance.stations.get_mut(ix) {
                        s.name = v.to_owned();
                    }
                }),
                arm: num_field(
                    window,
                    cx,
                    &mut subs,
                    p,
                    ARM_M,
                    move |p| p.weight_balance.stations.get(ix).map(|s| s.arm.0),
                    move |p, v| {
                        if let Some(s) = p.weight_balance.stations.get_mut(ix) {
                            s.arm = Meters(v.unwrap_or(0.0));
                        }
                    },
                ),
                max_load: num_field(
                    window,
                    cx,
                    &mut subs,
                    p,
                    MASS_KG.optional(),
                    move |p| p.weight_balance.stations.get(ix).and_then(|s| s.max_load).map(|m| m.0),
                    move |p, v| {
                        if let Some(s) = p.weight_balance.stations.get_mut(ix) {
                            s.max_load = v.map(Kilograms);
                        }
                    },
                ),
            })
            .collect();

        let envelope = cx.new(|_| EnvelopeEditor::new(wb.envelope.clone()));
        subs.push(cx.subscribe(
            &envelope,
            |this: &mut AircraftManagerView, editor, event: &EnvelopeEvent, cx| {
                let points = editor.read(cx).points().to_vec();
                match event {
                    // Live drag: draft only — persisting waits for release.
                    EnvelopeEvent::Changed => this.update_draft_in_memory(cx, move |p| {
                        p.weight_balance.envelope = points;
                    }),
                    EnvelopeEvent::Committed => this.commit_draft_edit(cx, move |p| {
                        p.weight_balance.envelope = points;
                    }),
                }
            },
        ));

        // -- distances ------------------------------------------------------------
        let takeoff_roll = num_field(
            window, cx, &mut subs,
            p, DISTANCE_M,
            |p| Some(p.distances.takeoff_roll.0),
            |p, v| p.distances.takeoff_roll = Meters(v.unwrap_or(0.0)),
        );
        let takeoff_50 = num_field(
            window, cx, &mut subs,
            p, DISTANCE_M.optional(),
            |p| p.distances.takeoff_over_50ft.map(|m| m.0),
            |p, v| p.distances.takeoff_over_50ft = v.map(Meters),
        );
        let landing_roll = num_field(
            window, cx, &mut subs,
            p, DISTANCE_M,
            |p| Some(p.distances.landing_roll.0),
            |p, v| p.distances.landing_roll = Meters(v.unwrap_or(0.0)),
        );
        let landing_50 = num_field(
            window, cx, &mut subs,
            p, DISTANCE_M.optional(),
            |p| p.distances.landing_over_50ft.map(|m| m.0),
            |p, v| p.distances.landing_over_50ft = v.map(Meters),
        );
        let takeoff_safety = num_field(
            window, cx, &mut subs,
            p, SAFETY_FACTOR,
            |p| Some(p.distances.takeoff_safety_factor),
            |p, v| p.distances.takeoff_safety_factor = v.unwrap_or(1.0),
        );
        let landing_safety = num_field(
            window, cx, &mut subs,
            p, SAFETY_FACTOR,
            |p| Some(p.distances.landing_safety_factor),
            |p, v| p.distances.landing_safety_factor = v.unwrap_or(1.0),
        );
        let factor_da = num_field(
            window, cx, &mut subs,
            p, CORRECTION_FACTOR,
            |p| Some(p.distances.factors.per_1000_ft_density_altitude),
            |p, v| p.distances.factors.per_1000_ft_density_altitude = v.unwrap_or(0.0),
        );
        let factor_headwind = num_field(
            window, cx, &mut subs,
            p, CORRECTION_FACTOR,
            |p| Some(p.distances.factors.per_10_kt_headwind),
            |p, v| p.distances.factors.per_10_kt_headwind = v.unwrap_or(0.0),
        );
        let factor_tailwind = num_field(
            window, cx, &mut subs,
            p, CORRECTION_FACTOR,
            |p| Some(p.distances.factors.per_10_kt_tailwind),
            |p, v| p.distances.factors.per_10_kt_tailwind = v.unwrap_or(0.0),
        );
        let factor_grass = num_field(
            window, cx, &mut subs,
            p, CORRECTION_FACTOR,
            |p| Some(p.distances.factors.grass),
            |p, v| p.distances.factors.grass = v.unwrap_or(0.0),
        );
        let factor_wet = num_field(
            window, cx, &mut subs,
            p, CORRECTION_FACTOR,
            |p| Some(p.distances.factors.wet),
            |p, v| p.distances.factors.wet = v.unwrap_or(0.0),
        );
        let factor_slope = num_field(
            window, cx, &mut subs,
            p, CORRECTION_FACTOR,
            |p| Some(p.distances.factors.per_percent_slope),
            |p, v| p.distances.factors.per_percent_slope = v.unwrap_or(0.0),
        );

        Self {
            name,
            registration,
            type_designator,
            callsign,
            cruise_rows,
            climb_ias,
            climb_rate,
            climb_flow,
            descent_ias,
            descent_rate,
            descent_flow,
            taxi_flow,
            fuel_usable,
            fuel_tabs,
            fuel_density,
            empty_mass,
            empty_arm,
            max_takeoff,
            max_landing,
            max_zero_fuel,
            max_ramp,
            station_rows,
            envelope,
            takeoff_roll,
            takeoff_50,
            landing_roll,
            landing_50,
            takeoff_safety,
            landing_safety,
            factor_da,
            factor_headwind,
            factor_tailwind,
            factor_grass,
            factor_wet,
            factor_slope,
            _subscriptions: subs,
        }
    }
}

// --- input bindings -----------------------------------------------------------

/// A text input committing on every change.
fn text_field(
    window: &mut Window,
    cx: &mut Context<AircraftManagerView>,
    subs: &mut Vec<Subscription>,
    initial: &str,
    placeholder: &str,
    apply: impl Fn(&mut AircraftProfile, &str) + 'static,
) -> Entity<InputState> {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder.to_owned())
            .default_value(initial.to_owned())
    });
    subs.push(cx.subscribe(
        &input,
        move |this: &mut AircraftManagerView, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().trim().to_owned();
                this.commit_draft_edit(cx, |profile| apply(profile, &value));
            }
        },
    ));
    input
}

/// A numeric input: parseable changes commit clamped values immediately;
/// blur/Enter rewrites the canonical formatted value from the draft.
/// `optional` fields treat an empty input as `None` ("not published").
fn num_field(
    window: &mut Window,
    cx: &mut Context<AircraftManagerView>,
    subs: &mut Vec<Subscription>,
    profile: &AircraftProfile,
    opts: NumOpts,
    get: impl Fn(&AircraftProfile) -> Option<f64> + 'static,
    set: impl Fn(&mut AircraftProfile, Option<f64>) + 'static,
) -> Entity<InputState> {
    let initial = get(profile)
        .map(|v| format_num(v, opts.decimals))
        .unwrap_or_default();
    let placeholder = if opts.optional { "—" } else { "0" };
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(initial)
    });
    let set = Rc::new(set);
    subs.push(cx.subscribe_in(
        &input,
        window,
        move |this: &mut AircraftManagerView, input, event: &InputEvent, window, cx| {
            match event {
                InputEvent::Change => {
                    let raw = input.read(cx).value().trim().to_owned();
                    if raw.is_empty() {
                        if opts.optional {
                            let set = Rc::clone(&set);
                            this.commit_draft_edit(cx, move |p| set(p, None));
                        }
                        // Required fields keep their previous value while
                        // the input is mid-edit; blur restores the text.
                    } else if let Some(parsed) = parse_num(&raw) {
                        let clamped = opts.clamp(parsed);
                        let set = Rc::clone(&set);
                        this.commit_draft_edit(cx, move |p| set(p, Some(clamped)));
                    }
                }
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    // Canonicalize the display from the committed draft
                    // (clamps become visible, junk text resets).
                    let Some(draft) = this.draft() else { return };
                    let text = get(draft)
                        .map(|v| format_num(v, opts.decimals))
                        .unwrap_or_default();
                    if input.read(cx).value().as_ref() != text {
                        input.update(cx, |state, cx| state.set_value(text, window, cx));
                    }
                }
                _ => {}
            }
        },
    ));
    input
}

// --- section renderers -----------------------------------------------------------

/// Builds all editor sections for the manager's render pass.
pub(super) fn render_sections(
    editor: &ProfileEditor,
    draft: &AircraftProfile,
    cx: &mut Context<AircraftManagerView>,
) -> Vec<AnyElement> {
    vec![
        section_identity(editor, draft, cx).into_any_element(),
        section_performance(editor, cx).into_any_element(),
        section_fuel(editor, draft, cx).into_any_element(),
        section_weight_balance(editor, draft, cx).into_any_element(),
        section_distances(editor, cx).into_any_element(),
    ]
}

/// One labelled field row — title + optional description left, the control
/// right (the settings-item layout).
fn field_row(
    label: impl Into<SharedString>,
    description: Option<&str>,
    control: impl IntoElement,
    cx: &Context<AircraftManagerView>,
) -> impl IntoElement {
    h_flex()
        .gap_4()
        .items_center()
        .justify_between()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().child(label.into()))
                .children(description.map(|text| {
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(text.to_owned())
                })),
        )
        .child(control)
}

fn section_title(
    title: &str,
    description: Option<&str>,
    cx: &Context<AircraftManagerView>,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(title.to_owned())
        .children(description.map(|text| {
            div()
                .text_sm()
                .font_normal()
                .text_color(cx.theme().muted_foreground)
                .child(text.to_owned())
        }))
}

fn input_w32(state: &Entity<InputState>) -> impl IntoElement {
    div().w_32().flex_shrink_0().child(Input::new(state).small())
}

fn section_identity(
    editor: &ProfileEditor,
    draft: &AircraftProfile,
    cx: &mut Context<AircraftManagerView>,
) -> GroupBox {
    let file_caption = format!(
        "id {} · aircraft/{}.{}",
        draft.id,
        draft.id,
        crate::flight_io::AIRCRAFT_EXTENSION
    );
    GroupBox::new()
        .id("aircraft-identity")
        .title(section_title("Identity", Some(&file_caption), cx))
        .gap_3()
        .child(field_row(
            "Name",
            Some("Display name in the library and the aircraft selector."),
            div().w_64().flex_shrink_0().child(Input::new(&editor.name).small()),
            cx,
        ))
        .child(field_row(
            "Registration",
            Some("Tail number, e.g. D-EABC."),
            input_w32(&editor.registration),
            cx,
        ))
        .child(field_row(
            "Type designator",
            Some("ICAO aircraft type (FPL item 9), e.g. C172."),
            input_w32(&editor.type_designator),
            cx,
        ))
        .child(field_row(
            "Callsign default",
            Some("FPL item 7; empty = registration without hyphens."),
            input_w32(&editor.callsign),
            cx,
        ))
}

fn section_performance(
    editor: &ProfileEditor,
    cx: &mut Context<AircraftManagerView>,
) -> GroupBox {
    let theme_muted = cx.theme().muted_foreground;
    let header = |text: &str| {
        div()
            .text_xs()
            .text_color(theme_muted)
            .child(text.to_owned())
    };

    let mut cruise = v_flex()
        .gap_1p5()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().min_w_0().child(header("Power setting")))
                .child(div().w_24().flex_shrink_0().child(header("TAS (kt)")))
                .child(div().w_24().flex_shrink_0().child(header("Flow (L/h)")))
                .child(div().w_6().flex_shrink_0()),
        );
    for (ix, row) in editor.cruise_rows.iter().enumerate() {
        cruise = cruise.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().min_w_0().child(Input::new(&row.name).small()))
                .child(div().w_24().flex_shrink_0().child(Input::new(&row.tas).small()))
                .child(div().w_24().flex_shrink_0().child(Input::new(&row.flow).small()))
                .child(
                    Button::new(("cruise-remove", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .tooltip("Remove this power setting")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.remove_cruise_setting(ix, window, cx);
                        })),
                ),
        );
    }
    cruise = cruise.child(
        h_flex().child(
            Button::new("cruise-add")
                .outline()
                .xsmall()
                .icon(IconName::Plus)
                .label("Add power setting")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.add_cruise_setting(window, cx);
                })),
        ),
    );

    let triple = |ias: &Entity<InputState>, rate: &Entity<InputState>, flow: &Entity<InputState>| {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().w_20().flex_shrink_0().child(Input::new(ias).small()))
            .child(div().w_20().flex_shrink_0().child(Input::new(rate).small()))
            .child(div().w_20().flex_shrink_0().child(Input::new(flow).small()))
    };

    GroupBox::new()
        .id("aircraft-performance")
        .title(section_title(
            "Performance",
            Some("POH planning values — cruise table plus single-segment climb and descent."),
            cx,
        ))
        .gap_3()
        .child(cruise)
        .child(field_row(
            "Climb",
            Some("IAS (kt) · rate (ft/min) · fuel flow (L/h)."),
            triple(&editor.climb_ias, &editor.climb_rate, &editor.climb_flow),
            cx,
        ))
        .child(field_row(
            "Descent",
            Some("IAS (kt) · rate (ft/min) · fuel flow (L/h)."),
            triple(&editor.descent_ias, &editor.descent_rate, &editor.descent_flow),
            cx,
        ))
        .child(field_row(
            "Taxi fuel flow (L/h)",
            Some("Ground flow; taxi fuel = policy taxi time × this."),
            input_w32(&editor.taxi_flow),
            cx,
        ))
}

fn fuel_type_label(fuel_type: FuelType) -> &'static str {
    match fuel_type {
        FuelType::Avgas100Ll => "Avgas 100LL",
        FuelType::Mogas => "Mogas",
        FuelType::JetA1 => "Jet A-1",
        FuelType::Diesel => "Diesel",
        FuelType::Other => "Other",
    }
}

const FUEL_TYPES: [FuelType; 5] = [
    FuelType::Avgas100Ll,
    FuelType::Mogas,
    FuelType::JetA1,
    FuelType::Diesel,
    FuelType::Other,
];

fn section_fuel(
    editor: &ProfileEditor,
    draft: &AircraftProfile,
    cx: &mut Context<AircraftManagerView>,
) -> GroupBox {
    let view = cx.entity();
    let fuel_type_menu = Button::new("fuel-type")
        .outline()
        .small()
        .label(fuel_type_label(draft.fuel.fuel_type))
        .dropdown_menu(move |mut menu, _window, _cx| {
            for fuel_type in FUEL_TYPES {
                let view = view.clone();
                menu = menu.item(PopupMenuItem::new(fuel_type_label(fuel_type)).on_click(
                    move |_, _, cx| {
                        view.update(cx, |this, cx| {
                            this.commit_draft_edit(cx, |p| p.fuel.fuel_type = fuel_type);
                        });
                    },
                ));
            }
            menu
        });

    GroupBox::new()
        .id("aircraft-fuel")
        .title(section_title("Fuel system", None, cx))
        .gap_3()
        .child(field_row(
            "Usable fuel (L)",
            Some("Total usable capacity."),
            input_w32(&editor.fuel_usable),
            cx,
        ))
        .child(field_row(
            "Tabs (L)",
            Some("\"Filled to tabs\" partial level — empty if the tanks have none."),
            input_w32(&editor.fuel_tabs),
            cx,
        ))
        .child(field_row("Fuel type", None, fuel_type_menu, cx))
        .child(field_row(
            "Density (kg/L)",
            Some("Mass density for W&B — avgas ≈ 0.72, Jet A-1 ≈ 0.80."),
            input_w32(&editor.fuel_density),
            cx,
        ))
}

fn station_kind_label(kind: StationKind) -> &'static str {
    match kind {
        StationKind::Seat => "Seat",
        StationKind::Baggage => "Baggage",
        StationKind::Fuel => "Fuel",
        StationKind::Other => "Other",
    }
}

const STATION_KINDS: [StationKind; 4] = [
    StationKind::Seat,
    StationKind::Baggage,
    StationKind::Fuel,
    StationKind::Other,
];

fn section_weight_balance(
    editor: &ProfileEditor,
    draft: &AircraftProfile,
    cx: &mut Context<AircraftManagerView>,
) -> GroupBox {
    let theme_muted = cx.theme().muted_foreground;
    let header = |text: &str| {
        div()
            .text_xs()
            .text_color(theme_muted)
            .child(text.to_owned())
    };
    let moment = draft.weight_balance.empty_mass.0 * draft.weight_balance.empty_arm.0;

    let mut stations = v_flex()
        .gap_1p5()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().min_w_0().child(header("Station")))
                .child(div().w_20().flex_shrink_0().child(header("Arm (m)")))
                .child(div().w_20().flex_shrink_0().child(header("Max (kg)")))
                .child(div().w_24().flex_shrink_0().child(header("Kind")))
                .child(div().w_6().flex_shrink_0()),
        );
    for (ix, row) in editor.station_rows.iter().enumerate() {
        let kind = draft
            .weight_balance
            .stations
            .get(ix)
            .map(|s| s.kind)
            .unwrap_or(StationKind::Other);
        let view = cx.entity();
        let kind_menu = Button::new(("station-kind", ix))
            .outline()
            .small()
            .label(station_kind_label(kind))
            .dropdown_menu(move |mut menu, _window, _cx| {
                for candidate in STATION_KINDS {
                    let view = view.clone();
                    menu = menu.item(PopupMenuItem::new(station_kind_label(candidate)).on_click(
                        move |_, _, cx| {
                            view.update(cx, |this, cx| {
                                this.commit_draft_edit(cx, move |p| {
                                    if let Some(s) = p.weight_balance.stations.get_mut(ix) {
                                        s.kind = candidate;
                                    }
                                });
                            });
                        },
                    ));
                }
                menu
            });
        stations = stations.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().min_w_0().child(Input::new(&row.name).small()))
                .child(div().w_20().flex_shrink_0().child(Input::new(&row.arm).small()))
                .child(div().w_20().flex_shrink_0().child(Input::new(&row.max_load).small()))
                .child(div().w_24().flex_shrink_0().child(kind_menu))
                .child(
                    Button::new(("station-remove", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .tooltip("Remove this station")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.remove_station(ix, window, cx);
                        })),
                ),
        );
    }
    stations = stations.child(
        h_flex().child(
            Button::new("station-add")
                .outline()
                .xsmall()
                .icon(IconName::Plus)
                .label("Add station")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.add_station(window, cx);
                })),
        ),
    );

    let pair = |a: &Entity<InputState>, b: &Entity<InputState>| {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().w_24().flex_shrink_0().child(Input::new(a).small()))
            .child(div().w_24().flex_shrink_0().child(Input::new(b).small()))
    };

    GroupBox::new()
        .id("aircraft-wb")
        .title(section_title(
            "Weight & balance",
            Some("Arms in meters aft of datum (negative = forward); masses in kilograms."),
            cx,
        ))
        .gap_3()
        .child(field_row(
            "Empty mass · arm",
            Some(&format!("Basic empty mass and its arm (moment {} kg·m).", format_num(moment, 1))),
            pair(&editor.empty_mass, &editor.empty_arm),
            cx,
        ))
        .child(field_row(
            "MTOW · MLW (kg)",
            Some("Max takeoff / max landing — MLW empty if not published."),
            pair(&editor.max_takeoff, &editor.max_landing),
            cx,
        ))
        .child(field_row(
            "MZFW · max ramp (kg)",
            Some("Both empty if not published."),
            pair(&editor.max_zero_fuel, &editor.max_ramp),
            cx,
        ))
        .child(stations)
        .child(
            v_flex()
                .gap_1()
                .child(div().text_sm().child("CG envelope"))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme_muted)
                        .child("Certified envelope in (arm, mass) space — published order."),
                )
                .child(editor.envelope.clone()),
        )
}

fn section_distances(
    editor: &ProfileEditor,
    cx: &mut Context<AircraftManagerView>,
) -> GroupBox {
    let pair = |a: &Entity<InputState>, b: &Entity<InputState>| {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().w_24().flex_shrink_0().child(Input::new(a).small()))
            .child(div().w_24().flex_shrink_0().child(Input::new(b).small()))
    };
    GroupBox::new()
        .id("aircraft-distances")
        .title(section_title(
            "Takeoff & landing distances",
            Some(
                "Base rolls at the POH reference condition (ISA sea level, MTOW, paved level \
                 dry). The correction factors below are the classic safety-leaflet rules of \
                 thumb (CAA Safety Sense / EASA safety promotion) — a template, not POH data: \
                 verify against your aircraft's figures.",
            ),
            cx,
        ))
        .gap_3()
        .child(field_row(
            "Takeoff roll · over 50 ft (m)",
            Some("Ground roll and the 50 ft obstacle distance (empty if not published)."),
            pair(&editor.takeoff_roll, &editor.takeoff_50),
            cx,
        ))
        .child(field_row(
            "Landing roll · over 50 ft (m)",
            None,
            pair(&editor.landing_roll, &editor.landing_50),
            cx,
        ))
        .child(field_row(
            "Safety factors (takeoff · landing)",
            Some("Overall multipliers on the corrected distances — templates 1.33 / 1.43."),
            pair(&editor.takeoff_safety, &editor.landing_safety),
            cx,
        ))
        .child(field_row(
            "Density altitude (per 1000 ft)",
            Some("Fractional increase per 1000 ft above ISA sea level — template +0.10."),
            input_w32(&editor.factor_da),
            cx,
        ))
        .child(field_row(
            "Wind (per 10 kt head · tail)",
            Some("Templates −0.10 headwind, +0.40 tailwind (conservative)."),
            pair(&editor.factor_headwind, &editor.factor_tailwind),
            cx,
        ))
        .child(field_row(
            "Surface (grass · wet)",
            Some("Dry grass +0.20; wet surface +0.15 on top."),
            pair(&editor.factor_grass, &editor.factor_wet),
            cx,
        ))
        .child(field_row(
            "Slope (per 1 % adverse)",
            Some("Upslope takeoff / downslope landing — template +0.10."),
            input_w32(&editor.factor_slope),
            cx,
        ))
}
