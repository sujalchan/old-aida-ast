use super::*;

#[test]
fn pen_fixture_generates_pen_commands_and_costume_state_without_todos() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/pen-tests.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    for generated in [
        "state.clear_pen();",
        "state.pen_down();",
        "state.pen_up();",
        "state.stamp();",
        "state.set_pen_rgb_color(scratch_pen_color);",
        "state.set_pen_color_parameter(crate::runtime::PenColorParameter::Color,",
        "state.set_pen_size(",
        "state.switch_costume_to(",
        "state.set_visible(true);",
        "state.set_visible(false);",
        "state.costume_number_name(true)",
    ] {
        assert!(
            output.contains(generated),
            "missing generated pen path: {generated}"
        );
    }
    assert!(!output.contains("unsupported statement: pen_"));
    assert!(!output.contains("unsupported reporter: looks_costume"));
    for parameter in ["color", "saturation", "brightness", "transparency"] {
        let variant = match parameter {
            "color" => "Color",
            "saturation" => "Saturation",
            "brightness" => "Brightness",
            "transparency" => "Transparency",
            _ => unreachable!(),
        };
        assert!(output.contains(&format!(
            "state.set_pen_color_parameter(crate::runtime::PenColorParameter::{variant},"
        )));
    }
}

#[test]
fn size_blocks_lower_to_live_runtime_size_updates() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1", "isStage": false,
                "blocks": {
                    "hat": {"opcode": "event_whenflagclicked", "next": "set", "topLevel": true},
                    "set": {"opcode": "looks_setsizeto", "next": "next", "inputs": {"SIZE": [1, [4, "140"]]}},
                    "next": {"opcode": "looks_nextcostume", "next": "change"},
                    "change": {"opcode": "looks_changesizeby", "inputs": {"CHANGE": [1, [4, "-10"]]}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&project);

    assert!(output.contains(
        "let scratch_size_input = scratch_to_number(140.0_f64); state.set_size(scratch_size_input);"
    ));
    assert!(output.contains("state.next_costume();"));
    assert!(output.contains("let scratch_size_input = scratch_to_number(-10.0_f64); state.change_size_by(scratch_size_input);"));
    assert!(!output.contains("unsupported statement: looks_setsize"));
}

#[test]
fn size_inputs_are_evaluated_before_mutating_runtime_state() {
    let project = parse_project(
        &json!({
            "targets": [
                {"name": "Stage", "isStage": true, "variables": {"size_id": ["size", 100]}, "blocks": {}},
                {
                    "name": "Sprite1", "isStage": false,
                    "blocks": {
                    "hat": {"opcode": "event_whenflagclicked", "next": "set", "topLevel": true},
                    "set": {"opcode": "looks_setsizeto", "next": "change", "inputs": {"SIZE": [3, "set_value"]}},
                    "set_value": {"opcode": "data_variable", "parent": "set", "fields": {"VARIABLE": ["size", "size_id"]}},
                    "change": {"opcode": "looks_changesizeby", "inputs": {"CHANGE": [3, "change_value"]}},
                    "change_value": {"opcode": "data_variable", "parent": "change", "fields": {"VARIABLE": ["size", "size_id"]}}
                    }
                }
            ]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&project);

    assert!(output.contains("let scratch_size_input = scratch_to_number(state.get_shared("));
    assert!(output.contains("state.set_size(scratch_size_input);"));
    assert!(output.contains("state.change_size_by(scratch_size_input);"));
    assert!(!output.contains("state.set_size(state.get_shared("));
    assert!(!output.contains("state.change_size_by(state.get_shared("));
}

#[test]
fn looks_size_reporter_reads_the_live_target_size() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1", "isStage": false,
                "blocks": {
                    "hat": {"opcode": "event_whenflagclicked", "next": "say", "topLevel": true},
                    "say": {"opcode": "looks_say", "inputs": {"MESSAGE": [2, "size"]}},
                    "size": {"opcode": "looks_size", "parent": "say", "inputs": {}, "fields": {}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&project);

    assert!(output.contains("state.size_reporter()"));
    assert!(!output.contains("unsupported reporter: looks_size"));
}

// lower the fixture's motion blocks to the shared runtime state methods.

#[test]
fn rotation_direction_fixture_generates_motion_and_rotation_style_paths() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/motion-rotation-direction.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    for generated in [
        "state.move_steps(scratch_to_number(",
        "state.set_rotation_style(\"left-right\");",
        "state.set_rotation_style(\"don't rotate\");",
        "state.set_rotation_style(\"all around\");",
        "state.bounce_on_edge();",
        "state.x_position()",
        "state.y_position()",
    ] {
        assert!(
            output.contains(generated),
            "missing generated motion path: {generated}"
        );
    }
    assert!(!output.contains("unsupported statement: motion_"));
    assert!(!output.contains("unsupported reporter: motion_"));
}

// specialize rgb assignment only when that target cannot use hsv controls.

#[test]
fn pen_rgb_generation_skips_unused_hsv_state_per_target() {
    let make_project = |include_hsv_control: bool| {
        let mut blocks = json!({
            "hat": { "opcode": "event_whenflagclicked", "next": "set_color", "topLevel": true },
            "set_color": { "opcode": "pen_setPenColorToColor", "next": null, "parent": "hat", "inputs": { "COLOR": [1, [10, "16711680"]] } }
        });
        if include_hsv_control {
            blocks["set_color"]["next"] = json!("set_saturation");
            blocks["set_saturation"] = json!({
                "opcode": "pen_setPenColorParamTo",
                "next": null,
                "parent": "set_color",
                "fields": { "COLOR_PARAM": ["saturation", null] },
                "inputs": { "VALUE": [1, [4, "50"]] }
            });
        }
        json!({
            "targets": [
                { "name": "Stage", "isStage": true, "variables": {}, "lists": {}, "blocks": {} },
                { "name": "Sprite1", "isStage": false, "variables": {}, "lists": {}, "blocks": blocks }
            ]
        })
    };

    let direct_project = parse_project(&make_project(false).to_string()).unwrap();
    let direct_output = generate(&direct_project);
    assert!(direct_output.contains("state.set_pen_rgb_color_without_hsv(\"16711680\");"));

    let hsv_project = parse_project(&make_project(true).to_string()).unwrap();
    let hsv_output = generate(&hsv_project);
    assert!(hsv_output.contains("state.set_pen_rgb_color(\"16711680\");"));
    assert!(hsv_output.contains("state.set_pen_color_parameter("));
}

#[test]
fn pen_smoke_loop_uses_enum_menu_values_in_its_hot_path() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/pen-smoke-test.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(
        output.contains("state.set_pen_color_parameter(crate::runtime::PenColorParameter::Color,")
    );
    assert!(!output.contains("scratch_pen_parameter"));
}

#[test]
fn reporter_connected_pen_color_parameter_keeps_dynamic_conversion() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/pen-tests.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let mut project = parse_project(&json).unwrap();
    let (variable_id, variable_name) = project
        .targets
        .iter()
        .find(|target| target.is_stage)
        .and_then(|stage| stage.variables.iter().next())
        .map(|(id, variable)| (id.clone(), variable.name.clone()))
        .unwrap();
    let target = project
        .targets
        .iter_mut()
        .find(|target| target.name == "Sprite1")
        .unwrap();
    target.blocks.insert(
        "dynamic_pen_parameter".to_owned(),
        ScratchBlock {
            opcode: "data_variable".to_owned(),
            fields: std::collections::HashMap::from([(
                "VARIABLE".to_owned(),
                json!([variable_name, variable_id]),
            )]),
            ..Default::default()
        },
    );
    let pen_block = target
        .blocks
        .values_mut()
        .find(|block| block.opcode == "pen_changePenColorParamBy")
        .unwrap();
    pen_block.inputs.insert(
        "COLOR_PARAM".to_owned(),
        json!([1, "dynamic_pen_parameter"]),
    );

    let output = generate(&project);
    assert!(output.contains("state.set_pen_color_parameter(scratch_string(state.get_"));
}

#[test]
fn clone_stop_fixture_lowers_motion_for_target_instances() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/clone-stop-test.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(
        output.contains("state.set_xy(scratch_to_number(-240.0_f64), scratch_to_number(0.0_f64));")
    );
    assert!(output.contains("state.change_x_by(scratch_to_number(1.0_f64));"));
    assert!(!output.contains("state.request_redraw();"));
    assert!(
        output.contains("self.scheduler.script_ids_in_frame_order_into(&mut self.frame_order)")
    );
    assert!(output.contains("self.scheduler.advance_script_id(id, &mut self.state)"));
    assert!(!output.contains(
        "self.scheduler.append_new_script_ids_since(&mut self.frame_order, next_new_id)"
    ));
    assert!(!output.contains("unsupported statement: motion_gotoxy"));
    assert!(!output.contains("unsupported statement: motion_changexby"));
}

#[test]
fn go_to_fixture_lowers_random_moves_and_resumable_glides() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/go-to-tests.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(output.contains("scratch_random(-240.5_f64, 240.5_f64)"));
    assert!(output.contains("scratch_random(-179.5_f64, 180.5_f64)"));
    assert!(output.contains("state.change_x_by(scratch_to_number(1.0_f64));"));
    assert!(output.contains("state.change_y_by(scratch_to_number(1.0_f64));"));
    assert!(output.contains("GlideState::new(clock.now_millis()"));
    assert!(output.contains("motion_glide.position_at(clock.now_millis())"));
    assert!(output.contains("return ScriptStatus::Yield;"));
    assert!(output.contains("state.set_xy(motion_x, motion_y);"));
    assert!(!output.contains("state.request_redraw();"));
    // statically numeric sprite updates use the allocation-free runtime entry point.
    assert!(output.contains("state.change_local_number(1, scratch_change);"));
    assert!(!output.contains(
        "self.scheduler.append_new_script_ids_since(&mut self.frame_order, next_new_id)"
    ));
    let first_glide = output.find("GlideState::new(clock.now_millis()").unwrap();
    let second_glide = output[first_glide + 1..]
        .find("GlideState::new(clock.now_millis()")
        .map(|offset| first_glide + 1 + offset)
        .unwrap();
    assert!(first_glide < second_glide);
    for opcode in [
        "motion_goto",
        "motion_changeyby",
        "motion_glideto",
        "motion_glidesecstoxy",
    ] {
        assert!(!output.contains(&format!("unsupported statement: {opcode}")));
    }
}

#[test]
fn motion_point_fixture_lowers_set_x_y_and_clone_direction_blocks() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/motion-point-test.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);
    assert!(output.contains("state.set_xy(scratch_to_number(0.0_f64), motion_y);"));
    assert!(output.contains("state.set_xy(motion_x, scratch_to_number(0.0_f64));"));
    assert!(output.contains("state.set_direction(scratch_to_number(90.0_f64));"));
    assert!(output.contains("scratch_random(-180.0_f64, 180.0_f64)"));
    for opcode in [
        "motion_setx",
        "motion_sety",
        "motion_pointindirection",
        "motion_pointtowards",
    ] {
        assert!(!output.contains(&format!("unsupported statement: {opcode}")));
    }
}

#[test]
fn motion_position_reporters_read_the_scheduled_instance() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/pen-smoke-test-two.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    // position reporters now use the target-aware state accessors directly.
    assert!(output.contains("state.x_position()"));
    assert!(output.contains("state.y_position()"));
}

#[test]
fn position_direction_clone_fixture_lowers_turns_and_direction_reporter() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/position-direction-clone.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(
        output.contains("state.set_direction(state.direction() + scratch_to_number(15.0_f64));")
    );
    assert!(
        output.contains("state.set_direction(state.direction() - scratch_to_number(15.0_f64));")
    );
    assert!(output.contains("scratch_string(state.direction())"));
    for opcode in ["motion_turnright", "motion_turnleft", "motion_direction"] {
        assert!(!output.contains(&format!("unsupported statement: {opcode}")));
        assert!(!output.contains(&format!("unsupported reporter: {opcode}")));
    }
}

#[test]
fn say_think_fixture_generates_bubbles_and_timed_waits() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/say-think.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(output.contains("state.set_speech_bubble(BubbleKind::Say"));
    assert!(output.contains("state.set_speech_bubble(BubbleKind::Think"));
    assert!(output.contains("return ScriptStatus::WaitUntil(wait_deadline);"));
    assert!(output.contains("self.state.expire_speech_bubbles(self.scheduler.now_millis());"));
    for opcode in [
        "looks_say",
        "looks_sayforsecs",
        "looks_think",
        "looks_thinkforsecs",
    ] {
        assert!(!output.contains(&format!("unsupported statement: {opcode}")));
    }
}

#[test]
fn sound_fixture_generates_resumable_playback_and_tick_yielding_effects() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/sound.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(output.contains("pub fn uses_sound() -> bool { true }"));
    assert!(output.contains("state.set_original_volume(1, 100_f64);"));
    assert!(output.contains("state.start_sound(scratch_sound_selector, clock.now_millis())"));
    assert!(output.contains("ScriptStatus::WaitForSignal(sound_wait)"));
    assert!(output.contains("ScratchSoundEffect::Pitch"));
    assert!(output.contains("ScratchSoundEffect::Pan"));
    assert!(output.contains("state.set_sound_volume(scratch_to_number("));
    assert!(output.contains("sound_volume_for_target(1)"));
    assert!(output.contains("const REFRESH_SOUND_VOLUME_MONITOR: bool = true;"));
    assert!(output.contains("if REFRESH_SOUND_VOLUME_MONITOR { state.request_presentation(); }"));
    assert!(output.contains("return ScriptStatus::Yield;"));
    assert!(!output.contains("unsupported statement: sound_"));
    assert!(!output.contains("unsupported reporter: sound_"));
}
