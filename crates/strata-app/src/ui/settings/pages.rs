//! The settings pages (Appearance / Map / Data / Countries / Data sources),
//! built fresh every frame — all values read straight from
//! `AppState`/`Config`, all setters write through the appliers in the
//! parent module. The Countries page lives in `super::countries`.

use gpui::{
    App, Axis, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Size, ThemeRegistry,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, NumberInput},
    label::Label,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage},
    slider::Slider,
    spinner::Spinner,
    v_flex,
};

use crate::assets::IconName;
use crate::config::{DEFAULT_UI_THEME_DARK, DEFAULT_UI_THEME_LIGHT};
use crate::state::{AppState, IngestDataset};

use super::format::{format_bias, map_theme_options, map_theme_value, mask_api_key, maxzoom_options};
use super::{SettingsView, apply_map_theme, apply_mode, apply_ui_theme, countries, persist_config};

/// Build the pages from the live view (`this` — leased, so only its
/// fields are cloned out) plus its entity handle (`view` — captured by
/// event-time closures only; reading it during render would double-lease).
pub(super) fn build(
    this: &SettingsView,
    view: &Entity<SettingsView>,
    _window: &mut Window,
    cx: &mut gpui::Context<SettingsView>,
) -> Vec<SettingPage> {
    vec![
        appearance(this, cx),
        map(this),
        data(this, view),
        countries::page(this, cx),
        sources(),
    ]
}

// --- Appearance --------------------------------------------------------------

fn appearance(this: &SettingsView, cx: &App) -> SettingPage {
    let app_state = this.app_state.clone();
    let map_view = this.map_view.clone();

    // Theme catalog from the registry (everything embedded at startup),
    // split by mode so each slot only offers themes designed for it.
    let registry = ThemeRegistry::global(cx);
    let theme_names = |dark: bool| -> Vec<(SharedString, SharedString)> {
        registry
            .sorted_themes()
            .iter()
            .filter(|theme| theme.mode.is_dark() == dark)
            .map(|theme| (theme.name.clone(), theme.name.clone()))
            .collect()
    };
    let dark_themes = theme_names(true);
    let light_themes = theme_names(false);

    SettingPage::new("Appearance")
        .icon(Icon::new(IconName::Palette))
        .default_open(true)
        .group(
            SettingGroup::new()
                .title("Theme")
                .item(
                    SettingItem::new(
                        "Dark mode",
                        SettingField::switch(
                            {
                                let app_state = app_state.clone();
                                move |cx: &App| app_state.read(cx).dark_mode
                            },
                            {
                                let app_state = app_state.clone();
                                let map_view = map_view.clone();
                                move |dark: bool, cx: &mut App| {
                                    apply_mode(&app_state, &map_view, dark, cx);
                                }
                            },
                        )
                        .default_value(true),
                    )
                    .description(
                        "Dark or light UI — the same switch as the sun/moon in the title bar. \
                         Each mode applies its theme below.",
                    ),
                )
                .item(
                    SettingItem::new(
                        "Dark mode theme",
                        SettingField::scrollable_dropdown(
                            dark_themes,
                            {
                                let app_state = app_state.clone();
                                move |cx: &App| {
                                    SharedString::from(
                                        app_state.read(cx).config.ui_theme_dark.clone(),
                                    )
                                }
                            },
                            {
                                let app_state = app_state.clone();
                                let map_view = map_view.clone();
                                move |name: SharedString, cx: &mut App| {
                                    apply_ui_theme(&app_state, &map_view, true, &name, cx);
                                }
                            },
                        )
                        .default_value(DEFAULT_UI_THEME_DARK),
                    )
                    .description("UI theme used while dark mode is on."),
                )
                .item(
                    SettingItem::new(
                        "Light mode theme",
                        SettingField::scrollable_dropdown(
                            light_themes,
                            {
                                let app_state = app_state.clone();
                                move |cx: &App| {
                                    SharedString::from(
                                        app_state.read(cx).config.ui_theme_light.clone(),
                                    )
                                }
                            },
                            {
                                let app_state = app_state.clone();
                                let map_view = map_view.clone();
                                move |name: SharedString, cx: &mut App| {
                                    apply_ui_theme(&app_state, &map_view, false, &name, cx);
                                }
                            },
                        )
                        .default_value(DEFAULT_UI_THEME_LIGHT),
                    )
                    .description("UI theme used while light mode is on."),
                )
                .item(
                    SettingItem::new(
                        "Map theme",
                        SettingField::scrollable_dropdown(
                            map_theme_options(),
                            {
                                let app_state = app_state.clone();
                                move |cx: &App| {
                                    map_theme_value(&app_state.read(cx).config.map_theme)
                                }
                            },
                            {
                                let app_state = app_state.clone();
                                let map_view = map_view.clone();
                                move |value: SharedString, cx: &mut App| {
                                    apply_map_theme(&app_state, &map_view, &value, cx);
                                }
                            },
                        )
                        .default_value("auto"),
                    )
                    .description(
                        "Palette for the map itself. Auto follows the active UI theme \
                         (same-named map theme, mode default as fallback).",
                    ),
                ),
        )
}

// --- Map ---------------------------------------------------------------------

fn map(this: &SettingsView) -> SettingPage {
    let app_state = this.app_state.clone();
    let slider = this.bias_slider.clone();
    SettingPage::new("Map")
        .icon(Icon::new(IconName::Map))
        .group(
            SettingGroup::new().title("Basemap").item(
                SettingItem::new(
                    "Detail bias",
                    SettingField::render({
                        move |_options, _window, cx: &mut App| {
                            let bias = app_state.read(cx).config.basemap_detail_bias;
                            h_flex()
                                .gap_3()
                                .items_center()
                                .child(
                                    Label::new(format_bias(bias))
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground),
                                )
                                .child(div().w(px(200.)).child(Slider::new(&slider)))
                        }
                    }),
                )
                .description(
                    "Shifts how early basemap detail appears, in zoom levels \
                     (−1.5 calm … +0.5 busy). Applies immediately.",
                ),
            ),
        )
}

// --- Data ----------------------------------------------------------------------

fn data(this: &SettingsView, view: &Entity<SettingsView>) -> SettingPage {
    let app_state = this.app_state.clone();
    SettingPage::new("Data")
        .icon(Icon::new(IconName::Database))
        .group(openaip_group(this, view))
        .group(autorouter_group(this, view))
        .group(data_dir_group(&app_state))
        .group(downloads_group(&app_state))
        .group(weather_group(this))
}

/// The autorouter.aero account credentials (NOTAM briefings): email +
/// masked password inputs (commit on Enter/blur), a "Test connection"
/// button with its inline outcome, and the honest plain-text caveat.
fn autorouter_group(this: &SettingsView, view: &Entity<SettingsView>) -> SettingGroup {
    let email_input = this.autorouter_email_input.clone();
    let password_input = this.autorouter_password_input.clone();
    let test = this.autorouter_test.clone();
    let view = view.clone();
    SettingGroup::new()
        .title("Autorouter")
        .item(
            SettingItem::new(
                "Account email",
                SettingField::render(move |options, _window, _cx: &mut App| {
                    Input::new(&email_input).with_size(options.size)
                }),
            )
            .layout(Axis::Vertical)
            .description(
                "autorouter.aero account used to fetch live NOTAM briefings \
                 (free account; the API is licensed for end-user use only).",
            ),
        )
        .item(
            SettingItem::new(
                "Password",
                SettingField::render(move |options, _window, _cx: &mut App| {
                    Input::new(&password_input).mask_toggle().with_size(options.size)
                }),
            )
            .layout(Axis::Vertical)
            .description(
                "Caution: stored in plain text in config.toml — don't reuse a \
                 password you use elsewhere.",
            ),
        )
        .item(
            SettingItem::new(
                "Test connection",
                SettingField::render(move |options, _window, cx: &mut App| {
                    render_autorouter_test(&test, &view, options.size, cx)
                }),
            )
            .layout(Axis::Vertical)
            .description(
                "Authenticates against api.autorouter.aero and performs one \
                 cheap authenticated call.",
            ),
        )
}

/// The test button + its inline outcome. `view` is only touched from the
/// event-time `update` — never read during render.
fn render_autorouter_test(
    test: &super::AutorouterTest,
    view: &Entity<SettingsView>,
    size: Size,
    cx: &mut App,
) -> gpui::AnyElement {
    use super::AutorouterTest;

    let running = *test == AutorouterTest::Running;
    let outcome: Option<gpui::AnyElement> = match test {
        AutorouterTest::Idle | AutorouterTest::Running => None,
        AutorouterTest::Success => Some(
            h_flex()
                .gap_1()
                .items_center()
                .text_color(cx.theme().success)
                .child(Icon::new(IconName::Check).small())
                .child(Label::new("Connection OK — credentials accepted.").text_sm())
                .into_any_element(),
        ),
        AutorouterTest::Failed(message) => Some(
            div()
                .text_sm()
                .text_color(cx.theme().danger)
                .whitespace_normal()
                .child(message.clone())
                .into_any_element(),
        ),
    };
    h_flex()
        .w_full()
        .gap_3()
        .items_center()
        .child(
            Button::new("autorouter-test")
                .outline()
                .with_size(size)
                .label("Test connection")
                .loading(running)
                .disabled(running)
                .on_click({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |this, cx| this.run_autorouter_test(cx));
                    }
                }),
        )
        .children(outcome.map(|el| div().flex_1().min_w_0().child(el)))
        .into_any_element()
}

fn openaip_group(this: &SettingsView, view: &Entity<SettingsView>) -> SettingGroup {
    let app_state = this.app_state.clone();
    let api_key_input = this.api_key_input.clone();
    let editing = this.api_key_editing;
    let view = view.clone();
    SettingGroup::new().title("openAIP").item(
        SettingItem::new(
            "API key",
            SettingField::render({
                move |options, _window, cx: &mut App| {
                    render_api_key_field(
                        editing,
                        &api_key_input,
                        &app_state,
                        &view,
                        options.size,
                        cx,
                    )
                }
            }),
        )
        // While editing, the input + Save need the full row width.
        .layout(if editing {
            Axis::Vertical
        } else {
            Axis::Horizontal
        })
        .description(
            "Required to download aeronautical data. Stored in config.toml; the \
             OPENAIP_API_KEY environment variable (.env) is the fallback while unset. \
             Takes effect on the next download.",
        ),
    )
}

/// The API-key field: masked summary + Change/Clear at rest, an input +
/// Save while editing (Enter and blur also commit). `view` is only touched
/// from event-time `update`s — never read during render.
fn render_api_key_field(
    editing: bool,
    input: &Entity<gpui_component::input::InputState>,
    app_state: &Entity<AppState>,
    view: &Entity<SettingsView>,
    size: Size,
    cx: &mut App,
) -> gpui::AnyElement {
    use gpui::IntoElement as _;

    if editing {
        return h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(input).with_size(size)),
            )
            .child(
                Button::new("api-key-save")
                    .primary()
                    .label("Save")
                    .with_size(size)
                    .on_click({
                        let view = view.clone();
                        move |_, window, cx| {
                            view.update(cx, |this, cx| this.commit_api_key(window, cx));
                        }
                    }),
            )
            .into_any_element();
    }

    let key = app_state.read(cx).config.openaip_api_key.clone();
    let masked = key
        .as_deref()
        .map(mask_api_key)
        .filter(|masked| !masked.is_empty());
    let is_set = masked.is_some();
    h_flex()
        .gap_2()
        .items_center()
        .child(
            Label::new(masked.unwrap_or_else(|| "Not set".to_owned()))
                .text_sm()
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            // Clearing = Change… → empty the field → Save (a blank commit
            // removes the stored key); no extra clear button needed.
            Button::new("api-key-edit")
                .outline()
                .with_size(size)
                .label(if is_set { "Change…" } else { "Set key…" })
                .on_click({
                    let view = view.clone();
                    move |_, window, cx| {
                        view.update(cx, |this, cx| this.begin_api_key_edit(window, cx));
                    }
                }),
        )
        .into_any_element()
}

fn data_dir_group(app_state: &Entity<AppState>) -> SettingGroup {
    SettingGroup::new().title("Data directory").item(
        SettingItem::new(
            "Location",
            SettingField::render({
                let app_state = app_state.clone();
                move |_options, _window, cx: &mut App| {
                    let dir = app_state.read(cx).data_dir.display().to_string();
                    div()
                        .w_full()
                        .px_2()
                        .py_1()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().muted.opacity(0.4))
                        .text_sm()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(dir)
                }
            }),
        )
        .layout(Axis::Vertical)
        .description(
            "Where downloaded data lives. Set data_dir in config.toml to move it \
             (applies on restart); the STRATA_DATA_DIR environment variable \
             overrides both for a single launch.",
        ),
    )
}

fn downloads_group(app_state: &Entity<AppState>) -> SettingGroup {
    SettingGroup::new()
        .title("Downloads")
        .item(
            SettingItem::new(
                "Automatic download",
                SettingField::switch(
                    {
                        let app_state = app_state.clone();
                        move |cx: &App| app_state.read(cx).config.ingest.auto
                    },
                    {
                        let app_state = app_state.clone();
                        move |auto: bool, cx: &mut App| {
                            app_state.update(cx, |state, cx| {
                                state.config.ingest.auto = auto;
                                persist_config(state, cx);
                            });
                        }
                    },
                )
                .default_value(true),
            )
            .description("Check for missing or stale data at startup and download what's needed."),
        )
        .item(
            SettingItem::new(
                "Basemap max zoom",
                SettingField::dropdown(
                    maxzoom_options(),
                    {
                        let app_state = app_state.clone();
                        move |cx: &App| {
                            SharedString::from(
                                app_state.read(cx).config.ingest.basemap_maxzoom.to_string(),
                            )
                        }
                    },
                    {
                        let app_state = app_state.clone();
                        move |value: SharedString, cx: &mut App| {
                            let Ok(zoom) = value.parse::<u8>() else {
                                return;
                            };
                            app_state.update(cx, |state, cx| {
                                state.config.ingest.basemap_maxzoom = zoom;
                                persist_config(state, cx);
                            });
                        }
                    },
                )
                .default_value("13"),
            )
            .description(
                "Highest basemap tile zoom to download — higher means more detail \
                 and a larger download. Applies to the next basemap download.",
            ),
        )
        .item(
            SettingItem::new(
                "Download now",
                SettingField::render({
                    let app_state = app_state.clone();
                    move |options, _window, cx: &mut App| {
                        render_download_buttons(&app_state, options.size, cx)
                    }
                }),
            )
            .layout(Axis::Vertical)
            .description(
                "Fetch datasets on demand (one run at a time, aero → basemap → terrain); \
                 progress appears in the panel at the bottom-left of the map.",
            ),
        )
}

/// Manual ingest triggers + a small running indicator. The buttons disable
/// while a run is active; the progress panel owns the detailed progress.
fn render_download_buttons(
    app_state: &Entity<AppState>,
    size: Size,
    cx: &mut App,
) -> gpui::AnyElement {
    let running = app_state.read(cx).ingest_running();
    let last_run = app_state.read(cx).last_ingest_result().map(|result| {
        format!(
            "Last run ({}): {}",
            result
                .finished_at
                .with_timezone(&chrono::Local)
                .format("%H:%M"),
            result.message(),
        )
    });

    let dataset_button = |id: &'static str, label: &'static str, dataset: IngestDataset| {
        let app_state = app_state.clone();
        Button::new(id)
            .outline()
            .with_size(size)
            .icon(IconName::Download)
            .label(label)
            .disabled(running)
            .on_click(move |_, _, cx| {
                app_state.update(cx, |state, cx| {
                    state.run_ingest_dataset(dataset, cx);
                });
            })
    };

    v_flex()
        .w_full()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(dataset_button("ingest-aero", "Aero data", IngestDataset::Aero))
                .child(dataset_button("ingest-basemap", "Basemap", IngestDataset::Basemap))
                .child(dataset_button("ingest-terrain", "Terrain", IngestDataset::Terrain))
                .child({
                    let app_state = app_state.clone();
                    Button::new("ingest-full")
                        .outline()
                        .with_size(size)
                        .icon(IconName::RefreshCw)
                        .label("Everything")
                        .disabled(running)
                        .on_click(move |_, _, cx| {
                            app_state.update(cx, |state, cx| {
                                state.run_full_ingest(cx);
                            });
                        })
                })
                .when(running, |this| {
                    this.child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Spinner::new().small())
                            .child(
                                Label::new("Download in progress…")
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child({
                                let app_state = app_state.clone();
                                Button::new("ingest-cancel")
                                    .ghost()
                                    .with_size(size)
                                    .label("Cancel")
                                    .on_click(move |_, _, cx| {
                                        app_state.update(cx, |state, _| {
                                            state.cancel_ingest();
                                        });
                                    })
                            }),
                    )
                }),
        )
        .children(last_run.map(|text| {
            Label::new(text)
                .text_xs()
                .text_color(cx.theme().muted_foreground)
        }))
        .into_any_element()
}

fn weather_group(this: &SettingsView) -> SettingGroup {
    // Owned NumberInput (not `SettingField::number_input`): the stock field
    // loses stepper changes at this gpui-component rev — see the
    // `weather_input` docs on `SettingsView`.
    let weather_input = this.weather_input.clone();
    SettingGroup::new().title("Weather").item(
        SettingItem::new(
            "Refresh interval (minutes)",
            SettingField::render(move |options, _window, _cx: &mut App| {
                NumberInput::new(&weather_input)
                    .with_size(options.size)
                    .w_32()
            }),
        )
        .description("How often live METAR/TAF/SIGMET data refreshes; applies from the next cycle."),
    )
}

// --- Data sources & attribution -------------------------------------------------

fn sources() -> SettingPage {
    SettingPage::new("Data sources")
        .icon(Icon::new(IconName::Info))
        .resettable(false)
        .group(
            SettingGroup::new()
                .title("Attribution")
                .description(
                    "Everything this app shows comes from these free sources. \
                     Coverage depends on the enabled countries (Countries page); \
                     gridded weather currently covers central Europe (the \
                     ICON-D2 model domain).",
                )
                .item(source_item(
                    "openAIP",
                    "CC BY-NC 4.0",
                    "Aeronautical data: airspaces, airports, navaids, reporting points, \
                     obstacles — openaip.net.",
                ))
                .item(source_item(
                    "OpenStreetMap contributors & Protomaps",
                    "ODbL",
                    "Vector basemap (Protomaps extract of OpenStreetMap data).",
                ))
                .item(source_item(
                    "DWD Open Data",
                    "GeoNutzV — source: Deutscher Wetterdienst",
                    "Gridded weather overlays: ICON-D2 model fields and the RV radar \
                     composite (opendata.dwd.de).",
                ))
                .item(source_item(
                    "NOAA aviationweather.gov",
                    "US Government work, public domain",
                    "Live METAR, TAF and SIGMET reports.",
                ))
                .item(source_item(
                    "Copernicus DEM GLO-30",
                    "© European Union, Copernicus programme",
                    "Terrain elevation used for the hillshade layer.",
                )),
        )
        .group(
            SettingGroup::new().title("Disclaimer").item(SettingItem::render(
                |_options, _window, cx: &mut App| {
                    h_flex()
                        .w_full()
                        .gap_2()
                        .items_start()
                        .text_color(cx.theme().warning)
                        .child(Icon::new(IconName::TriangleAlert).small())
                        .child(
                            // flex_1 + min_w_0 so the text wraps instead of
                            // running off the card.
                            div().flex_1().min_w_0().child(
                                Label::new(
                                    "NOT FOR NAVIGATION — data may be incomplete or out of \
                                     date. Always use official AIP, NOTAM and weather \
                                     briefings for flight preparation.",
                                )
                                .text_sm(),
                            ),
                        )
                },
            )),
        )
}

fn source_item(name: &'static str, license: &'static str, blurb: &'static str) -> SettingItem {
    SettingItem::render(move |_options, _window, cx: &mut App| {
        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_baseline()
                    .justify_between()
                    .child(Label::new(name).text_sm())
                    .child(
                        Label::new(license)
                            .text_xs()
                            .text_color(cx.theme().muted_foreground),
                    ),
            )
            .child(
                Label::new(blurb)
                    .text_sm()
                    .text_color(cx.theme().muted_foreground),
            )
    })
}
