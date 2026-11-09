use super::*;

#[test]
fn clone_fixture_lowers_clone_creation_and_clone_start_hats() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/clone-with-variables.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(output.contains("state.create_clone(source, target, 300);"));
    assert!(output.contains("fn new_clone_scripts(target: usize, _instance: usize)"));
    assert!(output.contains("new_clone_start_script_sprite1_"));
    assert!(output.contains("take_pending_clones"));
    assert!(output.contains("remove_target_scripts"));
    assert!(output.contains("ScratchVariableMetadata::for_target(\"`jEk@4|i[#Fk?(8x)AV.-my variable\", \"my variable\", \"Stage\", 0, true)"));
}

// exercise selector strings through list mutation and reporter code generation.

#[test]
fn broadcast_fixture_resolves_message_ids_and_emits_restartable_receiver_hats() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/broadcast-test.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(
        output
            .contains("message: broadcast_message_key(scratch_string(\"message1\")), wait: false")
    );
    assert!(
        output.contains("message: broadcast_message_key(scratch_string(\"message1\")), wait: true")
    );
    assert!(
        output
            .contains("message: broadcast_message_key(scratch_string(\"message2\")), wait: false")
    );
    assert!(output.contains("1 => { for instance in state.instances_for_target(2)"));
    assert!(output.contains("2 => { for instance in state.instances_for_target(1)"));
    assert!(output.contains("fn broadcast_receiver(&self) -> Option<(usize, usize, usize)>"));
    assert!(!output.contains(
        "self.scheduler.append_new_script_ids_since(&mut self.frame_order, next_new_id)"
    ));
    assert!(!output.contains("unsupported statement: event_broadcast"));
    assert!(!output.contains("unsupported statement: event_broadcastandwait"));
}

#[test]
fn backdrop_switch_fixture_routes_events_to_stage_backdrop_hats() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/think.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let mut project = parse_project(&json).unwrap();
    let stage = project
        .targets
        .iter_mut()
        .find(|target| target.is_stage)
        .unwrap();
    stage.blocks.insert(
        "backdrop_hat".to_owned(),
        ScratchBlock {
            opcode: "event_whenbackdropswitchesto".to_owned(),
            top_level: true,
            fields: [("BACKDROP".to_owned(), json!(["backdrop1", null]))].into(),
            ..ScratchBlock::default()
        },
    );

    let output = generate(&project);

    assert!(output.contains("fn new_backdrop_scripts(backdrop: &str, state: &ScratchState)"));
    assert!(output.contains("\"backdrop1\" => { for instance in state.instances_for_target("));
    assert!(output.contains("backdrop_receiver_script_stage_"));
    // redraw stops another scheduler round but does not interrupt the current script turn.
    assert!(!output.contains("if state.redraw_requested() { return ScriptStatus::Yield; }"));
}

#[test]
fn keypress_hats_start_generated_scripts_from_input_edges() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1", "isStage": false, "variables": {},
                    "blocks": {
                        "hat": {"opcode":"event_whenkeypressed", "next":"say", "topLevel":true, "inputs":{}, "fields":{"KEY_OPTION":["space",null]}},
                        "say": {"opcode":"looks_say", "next":null, "parent":"hat", "inputs":{"MESSAGE":[1,[10,"key"]]}, "fields":{}}
                    }
                }]
            }).to_string(),
        ).unwrap();
    let output = generate(&project);
    assert!(output.contains("fn new_keypress_scripts(key: &str, state: &ScratchState)"));
    assert!(output.contains("\"space\" => { for instance in state.instances_for_target(0)"));
    assert!(output.contains("self.scheduler.start_key_hats(scripts)"));
    assert!(output.lines().any(|line| {
        line.starts_with("fn new_key_pressed_script_") && line.contains("_for(instance")
    }));
    assert!(
        !output.lines().any(|line| {
            line.starts_with("fn new_key_pressed_script_") && !line.contains("_for(")
        })
    );
}

// check that the special any selector is emitted before key-specific hats.

#[test]
fn any_key_hat_is_started_for_each_recognized_key_event() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name":"Stage", "isStage":true, "variables":{}, "blocks":{}
                }, {
                    "name":"Sprite1", "isStage":false, "variables":{},
                    "blocks": {
                        "any_hat": {"opcode":"event_whenkeypressed", "next":"any_say", "topLevel":true, "inputs":{}, "fields":{"KEY_OPTION":["any",null]}},
                        "any_say": {"opcode":"looks_say", "next":null, "parent":"any_hat", "inputs":{"MESSAGE":[1,[10,"any"]]}, "fields":{}},
                        "letter_hat": {"opcode":"event_whenkeypressed", "next":"letter_say", "topLevel":true, "inputs":{}, "fields":{"KEY_OPTION":["a",null]}},
                        "letter_say": {"opcode":"looks_say", "next":null, "parent":"letter_hat", "inputs":{"MESSAGE":[1,[10,"letter"]]}, "fields":{}}
                    }
                }]
            }).to_string(),
        ).unwrap();
    let output = generate(&project);
    let start = output.find("fn new_keypress_scripts").unwrap();
    let body = &output[start..];
    let match_at = body.find("match key").unwrap();
    assert!(body[..match_at].contains("state.instances_for_target(1)"));
    assert!(body[match_at..].contains("\"a\" =>"));
}

// verify the supplied project keeps its active costume reporter and shadow selector distinct.

#[test]
fn detect_keys_fixture_uses_connected_reporter_and_shadow_fallback() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/detect-keys.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let sprite = project
        .targets
        .iter()
        .find(|target| target.name == "Sprite1")
        .unwrap();
    assert_eq!(sprite.costumes[0].name, "w");

    let output = generate(&project);
    assert!(output.contains("state.key_pressed(state.costume_number_name(false))"));
    assert!(output.contains("state.key_pressed(\"up arrow\")"));
    assert!(!output.contains("state.key_pressed(\"w\")"));
    assert!(output.contains("\"up arrow\" => { for instance in state.instances_for_target(1)"));
    assert!(output.contains("\"w\" => { for instance in state.instances_for_target(1)"));
    assert!(output.contains("fn key_hat_key(&self) -> Option<(usize, usize)>"));
    assert!(output.contains("self.scheduler.start_key_hats(scripts)"));
    let key_hat_impl = output
        .split("fn key_hat_key(&self) -> Option<(usize, usize)>")
        .nth(1)
        .unwrap()
        .split("fn restart_for_broadcast")
        .next()
        .unwrap();
    assert!(!key_hat_impl.contains("_ => None"));
}

// ensure an input-bearing reporter monitor evaluates in the monitor target's state context.

#[test]
fn sprite_backdrop_event_fixture_generates_scoped_clicks_and_edge_hats() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/sprite-backdrop-events.sb3"
    );
    let json = crate::extractor::read_project_json(path).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(output.contains("state.loudness() > scratch_to_number(-10.0_f64)"));
    assert!(
        output.contains("state.project_timer_seconds(now_millis) > scratch_to_number(10.0_f64)")
    );
    assert!(output.contains("previous.rising_edge("));
    assert!(output.contains("self.scheduler.start_edge_hats(starters)"));
    assert!(output.contains("pointer_click_activates("));
    assert!(output.contains("state.instance_target(instance)"));
    assert!(output.contains("None => { if let Some(instance) = state.original_instance(0)"));
    assert!(output.contains("sprite_clicked_script_sprite1"));
    assert!(output.contains("stage_clicked_script_stage"));
    assert!(output.contains("state.reset_project_timer(clock.now_millis());"));
    assert!(output.contains("state.reset_project_timer(self.scheduler.now_millis());"));
    assert!(output.contains("pub fn restart(&mut self) { self.edge_hat_previous.clear();"));
    assert!(
        output.contains("pub fn stop_all(&mut self) { self.scheduler.stop_all(&mut self.state); }")
    );
    assert!(!output.contains("-> GeneratedScript { new_greater_than_script_"));
    assert!(!output.contains("-> GeneratedScript { new_sprite_clicked_script_"));
}

#[test]
fn greater_than_hat_defaults_match_scratch_block_defaults() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Stage",
                "isStage": true,
                "blocks": {
                    "threshold": {
                        "opcode": "event_whengreaterthan",
                        "topLevel": true,
                        "inputs": {},
                        "fields": {}
                    }
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&project);
    assert!(output.contains("state.loudness() > scratch_to_number(10.0_f64)"));
}
