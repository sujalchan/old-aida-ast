use super::*;

#[test]
fn large_list_fixture_uses_compact_initializer_and_preserves_all_items() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/monitor-tests-sensing.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let list = project
        .targets
        .iter()
        .flat_map(|target| target.lists.values())
        .find(|list| list.items.len() == 200_000)
        .expect("fixture should contain its complete 200,000-item list");
    assert!(list.items[0].is_number());
    assert!(list.items[100_000].is_number());
    assert!(list.items[199_999].is_number());

    let (output, list_data) = generate_with_list_data(&project, false, "large.rs.lists.json");
    let serialized_items = serde_json::to_string(&list.items).unwrap();
    assert!(output.contains("include_bytes!(\"large.rs.lists.json\")"));
    assert!(output.contains("ScratchList::from_json_bytes(&initial_list_data["));
    let list_data = list_data.expect("large list data should be externalized");
    assert!(
        list_data
            .windows(serialized_items.len())
            .any(|window| window == serialized_items.as_bytes())
    );
    assert!(
        output.len() < 2_000_000,
        "generated module was {} bytes",
        output.len()
    );
}

#[test]
fn small_list_fixture_keeps_inline_initializers() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/lists.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let (output, list_data) = generate_with_list_data(&project, false, "lists.rs.lists.json");

    assert!(list_data.is_none());
    assert!(!output.contains("include_bytes!("));
    assert!(output.contains("ScratchList::from_json("));
}

#[test]
fn sensing_fixture_lowers_reporters_as_values_and_emits_live_monitor_snapshots() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/monitor-tests-sensing.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    let imports = output.lines().next().unwrap();
    assert!(imports.contains("with_scratch_random"));
    assert!(!imports.split(", ").any(|name| name == "scratch_random"));
    assert!(output.contains("state.set_variable_visible(0, true);"));
    assert!(output.contains("state.set_variable_visible(0, false);"));
    assert!(output.contains("state.days_since_2000()"));
    assert!(output.contains("state.username()"));
    assert!(output.contains("state.current_year()"));
    assert!(output.contains("state.current_day_of_week()"));
    assert!(output.contains("state.mouse_x()"));
    assert!(output.contains("state.mouse_y()"));
    assert!(output.contains("state.mouse_down()"));
    assert!(output.contains("state.loudness()"));
    assert!(output.contains("state.project_timer_seconds(clock.now_millis())"));
    for method in [
        "current_year",
        "current_month",
        "current_date",
        "current_day_of_week",
        "current_hour",
        "current_minute",
        "current_second",
    ] {
        assert!(
            output.contains(&format!("state.{method}()")),
            "missing {method}"
        );
        assert!(
            output.contains(&format!("scratch_string(state.{method}())")),
            "{method} is not used through the shared reporter path in a block input"
        );
    }
    for reporter in [
        "state.days_since_2000()",
        "state.mouse_x()",
        "state.mouse_y()",
        "state.project_timer_seconds(clock.now_millis())",
        "state.username()",
        "state.mouse_down()",
    ] {
        assert!(
            output.contains(&format!("scratch_string({reporter})")),
            "{reporter} is not used through the shared reporter path"
        );
    }
    assert!(output.contains("scratch_subtract(state.days_since_2000(), state.get_shared(0))"));
    for monitor_key in [
        "sensing_dayssince2000/sensing_dayssince2000",
        "sensing_current/current_year",
        "sensing_current/current_month",
        "sensing_current/current_date",
        "sensing_current/current_dayofweek",
        "sensing_current/current_hour",
        "sensing_current/current_minute",
        "sensing_current/current_second",
        "sensing_online/online",
        "sensing_username/sensing_username",
        "sensing_mousex/sensing_mousex",
        "sensing_mousey/sensing_mousey",
        "sensing_mousedown/sensing_mousedown",
    ] {
        assert!(
            output.contains(&format!("__reporter/Stage/{monitor_key}")),
            "missing monitor snapshot for {monitor_key}"
        );
    }
    assert!(output.contains("sensing_online"));
    assert!(output.contains("value: ScratchValue = (false).into()"));
    // assert the generated reporter key and local binding separately to avoid formatting coupling.
    assert!(output.contains("__reporter/Stage/sensing_current/current_year\".to_owned()"));
    assert!(output.contains("value: ScratchValue = (self.state.current_year()).into()"));
    assert!(output.contains("__reporter/Stage/sensing_username/sensing_username\".to_owned()"));
    assert!(output.contains("value: ScratchValue = (self.state.username()).into()"));
    assert!(output.contains("ReporterSnapshot"));
    assert!(output.contains("pub fn has_live_sensing_monitors(&self) -> bool { true }"));
    assert!(!output.contains("unsupported reporter: sensing_dayssince2000"));
    assert!(!output.contains("unsupported reporter: sensing_username"));
    assert!(!output.contains("unsupported reporter: sensing_online"));
    assert!(!output.contains("unsupported statement: data_showvariable"));
    assert!(!output.contains("unsupported statement: data_hidevariable"));
}

#[test]
fn list_index_dropdowns_flow_into_list_mutations_and_item_reporters() {
    let project = parse_project(
            r#"{
                "targets": [{
                    "name": "Stage",
                    "isStage": true,
                    "lists": { "list-id": ["items", ["a", "b"]] },
                    "blocks": {
                        "hat": { "opcode": "event_whenflagclicked", "next": "delete", "topLevel": true },
                        "delete": { "opcode": "data_deleteoflist", "next": "insert", "parent": "hat", "fields": { "LIST": ["items", "list-id"] }, "inputs": { "INDEX": [3, "all", [10, ""]] } },
                        "all": { "opcode": "data_listindexall", "shadow": true, "fields": { "INDEX": ["all", null] } },
                        "insert": { "opcode": "data_insertatlist", "next": "replace", "parent": "delete", "fields": { "LIST": ["items", "list-id"] }, "inputs": { "ITEM": [1, [10, "x"]], "INDEX": [3, "random", [10, ""]] } },
                        "random": { "opcode": "data_listindexrandom", "shadow": true, "fields": { "INDEX": ["random", null] } },
                        "replace": { "opcode": "data_replaceitemoflist", "next": "say", "parent": "insert", "fields": { "LIST": ["items", "list-id"] }, "inputs": { "INDEX": [3, "last", [10, ""]], "ITEM": [1, [10, "y"]] } },
                        "last": { "opcode": "data_listindexall", "shadow": true, "fields": { "INDEX": ["last", null] } },
                        "say": { "opcode": "looks_say", "parent": "replace", "inputs": { "MESSAGE": [3, "item", [10, ""]] } },
                        "item": { "opcode": "data_itemoflist", "parent": "say", "fields": { "LIST": ["items", "list-id"] }, "inputs": { "INDEX": [3, "get_index", [10, ""]] } },
                        "get_index": { "opcode": "data_listindexrandom", "shadow": true, "fields": { "INDEX": ["last", null] } }
                    }
                }]
            }"#,
        )
        .unwrap();
    let output = generate(&project);

    assert!(output.contains("let scratch_list_index: ScratchValue = (\"all\").into();"));
    assert!(output.contains("let scratch_list_index: ScratchValue = (\"random\").into();"));
    assert!(output.contains("let scratch_list_index: ScratchValue = (\"last\").into();"));
    assert!(output.contains("state.list(0).get_value(\"last\")"));
    assert!(
        output.contains("let scratch_list_changed = state.list_mut(0).delete(scratch_list_index);")
    );
    assert!(output.contains("state.list_changed_result(0, scratch_list_changed);"));
    assert!(!output.contains("list_before_change"));
    assert!(!output.contains("unsupported reporter: data_listindex"));
}

#[test]
fn item_replace_fixture_uses_guarded_numeric_list_paths() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/item-replace-smoke.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let list = project
        .targets
        .iter()
        .flat_map(|target| target.lists.values())
        .find(|list| list.items.len() == 172_800)
        .expect("replacement fixture should retain all 172,800 initial list items");
    // confirm project parsing retains the beginning, middle, and final items.
    assert!(list.items[0].is_number());
    assert!(list.items[86_400].is_number());
    assert!(list.items[172_799].is_number());
    let output = generate(&project);

    // numeric inputs use one validated replacement and read-back operation.
    assert!(output.contains("replace_number_and_equals"));
    assert!(output.contains("with_stage_numeric_list_batch"));
    assert!(output.contains("scratch_batch_value_"));
    assert!(output.contains("scratch_list_replace_matches"));
    assert!(output.contains("state.change_shared_number(1, scratch_to_number(1.0_f64));"));
    assert!(output.contains("repeat_remaining -= 1;"));
    assert!(output.contains("repeat_counts[0] = repeat_remaining;"));
    assert!(output.contains("if script.yield_at_safe_point(clock)"));
    assert!(output.contains("return ScriptStatus::StopAll;"));
    assert!(output.contains("with_scratch_random(|loop_random|"));
    assert!(output.contains("loop_random.between_with_integer_bounds"));
}

#[test]
fn replacement_batch_rejects_branch_that_reads_the_list() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/item-replace-smoke.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let mut project = parse_project(&json).unwrap();
    let sprite = project
        .targets
        .iter_mut()
        .find(|target| target.blocks.contains_key("i"))
        .unwrap();
    sprite
        .blocks
        .get_mut("i")
        .unwrap()
        .inputs
        .insert("MESSAGE".into(), serde_json::json!([2, "u"]));
    let output = generate(&project);
    assert!(!output.contains("with_stage_numeric_list_batch"));
    assert!(output.contains("scratch_list_replace_result.unwrap_or_else"));
    assert!(output.contains("state.list(0).get("));
}

// verify simple warp list fills use a bounded generated batch path and retain resumable state.

#[test]
fn list_smoke_warp_append_loop_is_fused_with_watchdog_chunks() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/list-smoke-test.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(output.contains("with_generated_finite_number_appender(repeat_count"));
    assert!(output.contains("scratch_list_append.add_finite_numbers(repeat_chunk"));
    assert!(output.contains("with_scratch_random(|scratch_rng|"));
    assert!(output.contains("between_with_integer_bounds(11.0_f64, 20.0_f64, true)"));
    assert!(output.contains("scratch_random_constant(11.0_f64, 20.0_f64, true)"));
    assert!(output.contains("script.warp_budget.expired_now(clock)"));
    assert!(output.contains("repeat_counts[0] = repeat_remaining"));
    assert!(!output.contains("list_before_change"));
}

#[test]
fn input_reporter_monitor_uses_its_block_input_and_target_context() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Stage", "isStage": true, "variables": {}, "blocks": {}
                }, {
                    "name": "Sprite1", "isStage": false, "variables": {},
                    "blocks": {
                        "key": {"opcode":"sensing_keypressed", "inputs":{"KEY_OPTION":[3,"costume",[10,"w"]]}, "fields":{}, "parent":null, "next":null},
                        "costume": {"opcode":"looks_costumenumbername", "inputs":{}, "fields":{"NUMBER_NAME":["name",null]}, "parent":"key", "next":null}
                    }
                }],
                "monitors": [{
                    "id":"key", "opcode":"sensing_keypressed", "params":{"KEY_OPTION":"space"},
                    "spriteName":"Sprite1", "visible":true
                }]
            }).to_string(),
        ).unwrap();
    let output = generate(&project);
    assert!(output.contains("state.key_pressed(state.costume_number_name(false))"));
    assert!(output.contains("self.state.original_instance(1)"));
    assert!(output.contains("self.state.set_current_instance(instance)"));
}

#[test]
fn stage_changes_use_the_shared_numeric_update_path() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/addition-loop.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    // numeric shared-variable updates preserve the runtime monitor path.
    assert!(output.contains("let scratch_change = scratch_to_number(1.0_f64);"));
    assert!(output.contains("state.change_shared_number(0, scratch_change);"));
}

#[test]
fn shared_state_slots_keep_same_named_target_variables_distinct() {
    let project = parse_project(
            &json!({
                "targets": [
                    {
                        "name": "Stage",
                        "isStage": true,
                        "variables": {"stage_score_id": ["score", 0]},
                        "blocks": {
                            "stage_hat": {"opcode": "event_whenflagclicked", "next": "stage_set", "topLevel": true, "inputs": {}, "fields": {}},
                            "stage_set": {"opcode": "data_setvariableto", "next": null, "parent": "stage_hat", "inputs": {"VALUE": [1, [4, "1"]]}, "fields": {"VARIABLE": ["score", "stage_score_id"]}}
                        }
                    },
                    {
                        "name": "Sprite1",
                        "isStage": false,
                        "variables": {"sprite_score_id": ["score", 0]},
                        "blocks": {
                            "sprite_hat": {"opcode": "event_whenflagclicked", "next": "sprite_set", "topLevel": true, "inputs": {}, "fields": {}},
                            "sprite_set": {"opcode": "data_setvariableto", "next": null, "parent": "sprite_hat", "inputs": {"VALUE": [1, [4, "2"]]}, "fields": {"VARIABLE": ["score", "sprite_score_id"]}}
                        }
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

    let output = generate(&project);
    assert!(output.contains(
        "ScratchVariableMetadata::for_target(\"stage_score_id\", \"score\", \"Stage\", 0, true)"
    ));
    assert!(output.contains(
        "ScratchVariableMetadata::for_target(\"sprite_score_id\", \"score\", \"Sprite1\", 1, false)"
    ));
    // numeric writes retain stage and sprite ownership in separate fast paths.
    assert!(output.contains("state.set_shared_number(1, 1.0_f64);"));
    assert!(output.contains("state.set_local_number(0, 2.0_f64);"));
}
