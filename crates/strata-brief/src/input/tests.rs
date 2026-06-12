//! Input contract tests: serde round-trips and the JSON key shape the
//! embedded template depends on.

use crate::fixtures::{full_input, minimal_input};

#[test]
fn full_input_round_trips_through_json() {
    let input = full_input();
    let json = serde_json::to_string(&input).expect("serializes");
    let back: super::BriefingInput = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(input, back);
}

#[test]
fn minimal_input_round_trips_through_json() {
    let input = minimal_input();
    let json = serde_json::to_string(&input).expect("serializes");
    let back: super::BriefingInput = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(input, back);
}

/// The template addresses fields by exact JSON key — pin the names the
/// template uses so a rename here cannot silently break rendering.
#[test]
fn json_shape_matches_the_template_contract() {
    let value = serde_json::to_value(full_input()).expect("serializes");

    assert!(value["flight"]["name"].is_string());
    assert!(value["flight"]["route"].is_array());
    assert!(value["flight"]["total_distance_nm"].is_number());
    assert!(
        value["generated_at"]
            .as_str()
            .expect("generated_at is a string")
            .starts_with("2026-06-11T09:30")
    );

    let rows = value["navlog"]["rows"].as_array().expect("navlog rows");
    assert_eq!(rows[0]["kind"], "waypoint");
    assert_eq!(rows[1]["kind"], "top_of_climb");
    assert!(rows[1]["wind"]["direction_deg"].is_number());
    assert!(value["navlog"]["total_fuel_liters"].is_number());

    assert!(value["fuel"]["minimum_required_liters"].is_number());
    assert!(value["weight_balance"]["states"][0]["within_limits"].is_boolean());
    assert!(value["weight_balance"]["envelope"][0]["arm_m"].is_number());
    assert!(value["weather"]["aerodromes"][0]["metar_raw"].is_string());
    assert!(value["weather"]["winds_aloft"][0]["speed_kt"].is_number());
    assert!(value["weather"]["winds_source_note"].is_string());
    assert!(value["notams"]["notams"][0]["raw"].is_string());
    assert!(value["notams"]["source_note"].is_string());

    // Absent data is an explicit null (the template tests against `none`),
    // not a missing key.
    let minimal = serde_json::to_value(minimal_input()).expect("serializes");
    assert!(minimal["navlog"].is_null());
    assert!(minimal["flight"]["departure_time"].is_null());
}

/// The source-note fields are additive: inputs serialized before they
/// existed still deserialize (serde defaults to `None`).
#[test]
fn source_notes_are_additive_for_older_inputs() {
    let mut value = serde_json::to_value(full_input()).expect("serializes");
    let weather = value["weather"].as_object_mut().expect("weather object");
    weather.remove("winds_source_note");
    let notams = value["notams"].as_object_mut().expect("notams object");
    notams.remove("source_note");

    let back: super::BriefingInput = serde_json::from_value(value).expect("deserializes");
    assert_eq!(back.weather.expect("weather").winds_source_note, None);
    assert_eq!(back.notams.expect("notams").source_note, None);
}
