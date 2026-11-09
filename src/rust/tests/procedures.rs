use super::*;

#[test]
fn emits_custom_blocks_as_resumable_argument_frames() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1",
                "isStage": false,
                "variables": {"result_id": ["result", 0]},
                "blocks": {
                    "hat": {
                        "opcode": "control_start_as_clone",
                        "next": "call",
                        "topLevel": true,
                        "inputs": {},
                        "fields": {}
                    },
                    "call": {
                        "opcode": "procedures_call",
                        "parent": "hat",
                        "inputs": {"argument_id": [1, [10, "hello"]]},
                        "fields": {},
                        "mutation": {
                            "proccode": "echo %s",
                            "argumentids": "[\"argument_id\"]",
                            "warp": "false"
                        }
                    },
                    "definition": {
                        "opcode": "procedures_definition",
                        "next": "set_result",
                        "topLevel": true,
                        "inputs": {"custom_block": [1, "prototype"]},
                        "fields": {}
                    },
                    "prototype": {
                        "opcode": "procedures_prototype",
                        "parent": "definition",
                        "inputs": {"argument_id": [1, "argument_default"]},
                        "fields": {},
                        "mutation": {
                            "proccode": "echo %s",
                            "argumentnames": "[\"value\"]",
                            "argumentids": "[\"argument_id\"]",
                            "argumentdefaults": "[\"\"]",
                            "warp": "false"
                        }
                    },
                    "argument_default": {
                        "opcode": "argument_reporter_string_number",
                        "parent": "prototype",
                        "shadow": true,
                        "inputs": {},
                        "fields": {"VALUE": ["value", null]}
                    },
                    "set_result": {
                        "opcode": "data_setvariableto",
                        "parent": "definition",
                        "inputs": {"VALUE": [3, "argument", [10, ""]]},
                        "fields": {"VARIABLE": ["result", "result_id"]}
                    },
                    "argument": {
                        "opcode": "argument_reporter_string_number",
                        "parent": "set_result",
                        "inputs": {},
                        "fields": {"VALUE": ["value", null]}
                    }
                }
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert!(output.contains("arguments[0]).clone()"));
    assert!(output.contains("\"hello\").to_owned().into()"));
    assert!(output.contains("script.push_frame("));
    assert!(output.contains("new_clone_start_script_sprite1_"));
}

#[test]
fn clone_nested_custom_calls_preserve_warp_mode_and_the_callers_pc() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1",
                    "isStage": false,
                    "variables": {"done_id": ["done", false], "progress_id": ["progress", 0]},
                    "blocks": {
                        "hat": {"opcode": "control_start_as_clone", "next": "call_outer", "topLevel": true, "inputs": {}, "fields": {}},
                        "call_outer": {"opcode": "procedures_call", "next": "after_call", "parent": "hat", "inputs": {}, "fields": {}, "mutation": {"proccode": "warp outer", "argumentids": "[]", "warp": "false"}},
                        "after_call": {"opcode": "looks_say", "parent": "call_outer", "inputs": {"MESSAGE": [1, [10, "returned"]]}, "fields": {}},
                        "inner_definition": {"opcode": "procedures_definition", "next": "inner_change", "topLevel": true, "inputs": {"custom_block": [1, "inner_prototype"]}, "fields": {}},
                        "inner_prototype": {"opcode": "procedures_prototype", "parent": "inner_definition", "inputs": {}, "fields": {}, "mutation": {"proccode": "normal inner", "argumentnames": "[]", "argumentids": "[]", "argumentdefaults": "[]", "warp": "false"}},
                        "inner_change": {"opcode": "data_changevariableby", "parent": "inner_definition", "inputs": {"VALUE": [1, [4, "1"]]}, "fields": {"VARIABLE": ["progress", "progress_id"]}},
                        "outer_definition": {"opcode": "procedures_definition", "next": "outer_repeat", "topLevel": true, "inputs": {"custom_block": [1, "outer_prototype"]}, "fields": {}},
                        "outer_prototype": {"opcode": "procedures_prototype", "parent": "outer_definition", "inputs": {}, "fields": {}, "mutation": {"proccode": "warp outer", "argumentnames": "[]", "argumentids": "[]", "argumentdefaults": "[]", "warp": "true"}},
                        "outer_repeat": {"opcode": "control_repeat", "next": "wait_until", "parent": "outer_definition", "inputs": {"TIMES": [1, [4, "2"]], "SUBSTACK": [2, "call_inner"]}, "fields": {}},
                        "call_inner": {"opcode": "procedures_call", "next": "outer_change", "parent": "outer_repeat", "inputs": {}, "fields": {}, "mutation": {"proccode": "normal inner", "argumentids": "[]", "warp": "false"}},
                        "outer_change": {"opcode": "data_changevariableby", "parent": "call_inner", "inputs": {"VALUE": [1, [4, "10"]]}, "fields": {"VARIABLE": ["progress", "progress_id"]}},
                        "wait_until": {"opcode": "control_wait_until", "parent": "outer_repeat", "inputs": {"CONDITION": [2, "done_reporter"]}, "fields": {}},
                        "done_reporter": {"opcode": "data_variable", "parent": "wait_until", "inputs": {}, "fields": {"VARIABLE": ["done", "done_id"]}}
                    }
                }]
            })
            .to_string(),
        )
        .unwrap();

    let output = generate(&project);
    assert!(output.contains("fn new_clone_scripts(target: usize, _instance: usize)"));
    assert!(output.contains("new_clone_start_script_sprite1_"));
    assert!(output.contains("inherited_warp || true"));
    assert!(output.contains("inherited_warp || false"));
    assert!(output.contains("if !(scratch_bool(state.get(0)))"));
    assert!(output.contains("__aida_set_pc!(script, 1);"));
    assert!(
        output.contains("if script.yield_at_safe_point(clock) { return ScriptStatus::Yield; }")
    );
    assert!(output.contains("_ => { script.pop_frame(); continue; }"));
    assert!(output.contains("print_value!(state, \"say\", scratch_bubble_text.clone());"));
}

#[test]
fn emits_and_invokes_each_green_flag_script_separately() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1",
                "isStage": false,
                "variables": {"count_id": ["count", 0]},
                "blocks": {
                    "first_hat": {
                        "opcode": "event_whenflagclicked",
                        "next": "first_repeat",
                        "topLevel": true,
                        "inputs": {},
                        "fields": {}
                    },
                    "first_repeat": {
                        "opcode": "control_repeat",
                        "next": null,
                        "parent": "first_hat",
                        "inputs": {"TIMES": [1, [4, "1"]], "SUBSTACK": [2, "first_set"]},
                        "fields": {}
                    },
                    "first_set": {
                        "opcode": "data_setvariableto",
                        "parent": "first_repeat",
                        "inputs": {"VALUE": [1, [4, "1"]]},
                        "fields": {"VARIABLE": ["count", "count_id"]}
                    },
                    "second_hat": {
                        "opcode": "event_whenflagclicked",
                        "next": "second_repeat",
                        "topLevel": true,
                        "inputs": {},
                        "fields": {}
                    },
                    "second_repeat": {
                        "opcode": "control_repeat",
                        "next": null,
                        "parent": "second_hat",
                        "inputs": {"TIMES": [1, [4, "1"]], "SUBSTACK": [2, "second_set"]},
                        "fields": {}
                    },
                    "second_set": {
                        "opcode": "data_setvariableto",
                        "parent": "second_repeat",
                        "inputs": {"VALUE": [1, [4, "2"]]},
                        "fields": {"VARIABLE": ["count", "count_id"]}
                    }
                }
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert!(output.matches("fn step_green_flag_script_sprite1_").count() >= 2);
    // each flag script keeps its own native write while sharing the variable slot.
    assert!(output.contains("state.set_local_number(0, 1.0_f64);"));
    assert!(output.contains("state.set_local_number(0, 2.0_f64);"));
    assert!(output.contains(
        "fn new_scripts() -> Vec<GeneratedScript> { vec![new_green_flag_script_sprite1_"
    ));
    assert!(output.contains("pub fn run_until_complete(&mut self)"));
    assert!(output.contains("pub fn step(&mut self) -> bool"));
    assert!(output.contains("pub fn restart(&mut self)"));
    // generated repeat branches may emit more than one valid yield site per script.
    assert!(output.matches("return ScriptStatus::Yield;").count() >= 2);
}

#[test]
fn identical_event_scripts_reuse_one_statically_dispatched_step_body() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1",
                "isStage": false,
                "variables": {},
                "blocks": {
                    "first_hat": {"opcode":"event_whenflagclicked", "next":"first_say", "topLevel":true, "inputs":{}, "fields":{}},
                    "first_say": {"opcode":"looks_say", "next":null, "parent":"first_hat", "inputs":{"MESSAGE":[1,[10,"same"]]}, "fields":{}},
                    "second_hat": {"opcode":"event_whenflagclicked", "next":"second_say", "topLevel":true, "inputs":{}, "fields":{}},
                    "second_say": {"opcode":"looks_say", "next":null, "parent":"second_hat", "inputs":{"MESSAGE":[1,[10,"same"]]}, "fields":{}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert_eq!(
        output.matches("fn step_green_flag_script_sprite1_").count(),
        1
    );
    assert!(output.contains("use self::step_green_flag_script_sprite1_"));
    assert!(output.contains(" as step_green_flag_script_sprite1_"));
}

#[test]
fn nested_branch_only_warp_calls_use_a_static_path_with_resumable_watchdog_fallback() {
    let mut blocks = json!({
        "hat": {"opcode":"event_whenflagclicked", "next":"call_outer", "topLevel":true, "inputs":{}, "fields":{}},
        "call_outer": {"opcode":"procedures_call", "parent":"hat", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentids":"[]", "warp":"true"}},
        "outer_definition": {"opcode":"procedures_definition", "next":"outer_branch", "topLevel":true, "inputs":{"custom_block":[1,"outer_prototype"]}, "fields":{}},
        "outer_prototype": {"opcode":"procedures_prototype", "parent":"outer_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
        "outer_branch": {"opcode":"control_if_else", "parent":"outer_definition", "inputs":{"CONDITION":[2,"outer_condition"],"SUBSTACK":[2,"call_large"],"SUBSTACK2":[2,"call_small"]}, "fields":{}},
        "outer_condition": {"opcode":"operator_equals", "parent":"outer_branch", "inputs":{"OPERAND1":[1,[4,"1"]],"OPERAND2":[1,[4,"1"]]}, "fields":{}},
        "call_large": {"opcode":"procedures_call", "parent":"outer_branch", "inputs":{}, "fields":{}, "mutation":{"proccode":"large", "argumentids":"[]", "warp":"true"}},
        "call_small": {"opcode":"procedures_call", "parent":"outer_branch", "inputs":{}, "fields":{}, "mutation":{"proccode":"small", "argumentids":"[]", "warp":"true"}},
        "large_definition": {"opcode":"procedures_definition", "next":"large_if", "topLevel":true, "inputs":{"custom_block":[1,"large_prototype"]}, "fields":{}},
        "large_prototype": {"opcode":"procedures_prototype", "parent":"large_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"large", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
        "large_if": {"opcode":"control_if_else", "parent":"large_definition", "inputs":{"CONDITION":[2,"large_condition"],"SUBSTACK":[2,"large_yes_0"],"SUBSTACK2":[2,"large_no_0"]}, "fields":{}},
        "large_condition": {"opcode":"operator_equals", "parent":"large_if", "inputs":{"OPERAND1":[1,[4,"1"]],"OPERAND2":[1,[4,"1"]]}, "fields":{}},
        "small_definition": {"opcode":"procedures_definition", "next":"small_change", "topLevel":true, "inputs":{"custom_block":[1,"small_prototype"]}, "fields":{}},
        "small_prototype": {"opcode":"procedures_prototype", "parent":"small_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"small", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
        "small_change": {"opcode":"data_changevariableby", "next":"small_stop", "parent":"small_definition", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["count","count_id"]}},
        "small_stop": {"opcode":"control_stop", "parent":"small_change", "inputs":{}, "fields":{"STOP_OPTION":["this script", null]}}
    });
    let blocks = blocks.as_object_mut().unwrap();
    for (branch, count) in [("large_yes", 42), ("large_no", 41)] {
        for index in 0..count {
            let id = format!("{branch}_{index}");
            let next = (index + 1 < count).then(|| format!("{branch}_{}", index + 1));
            blocks.insert(
                id,
                json!({
                    "opcode":"data_changevariableby",
                    "next":next,
                    "parent":if index == 0 {"large_if".to_owned()} else {format!("{branch}_{}", index - 1)},
                    "inputs":{"VALUE":[1,[4,"1"]]},
                    "fields":{"VARIABLE":["count","count_id"]}
                }),
            );
        }
    }

    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1",
                "isStage": false,
                "variables": {"count_id": ["count", 0]},
                "blocks": blocks
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert!(output.contains("fn direct_warp_procedure_0_"));
    assert!(output.contains("if script.warp_budget.expired(clock) {"));
    assert!(output.contains("script.push_frame("));
    assert!(output.contains("direct_warp_procedure_0_"));
    assert!(output.contains("-> bool {"));
    assert!(output.contains("return false;"));
    assert!(output.contains("return ScriptStatus::Complete;"));
    assert!(output.contains("if inherited_warp || true {"));
}

#[test]
fn wait_in_custom_block_advances_its_frame_before_scheduler_suspension() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1",
                    "isStage": false,
                    "variables": {},
                    "blocks": {
                        "hat": {"opcode": "control_start_as_clone", "next": "call", "topLevel": true, "inputs": {}, "fields": {}},
                        "call": {"opcode": "procedures_call", "next": null, "parent": "hat", "inputs": {}, "fields": {}, "mutation": {"proccode": "delay", "argumentids": "[]", "warp": "true"}},
                        "definition": {"opcode": "procedures_definition", "next": "wait", "topLevel": true, "inputs": {"custom_block": [1, "prototype"]}, "fields": {}},
                        "prototype": {"opcode": "procedures_prototype", "parent": "definition", "inputs": {}, "fields": {}, "mutation": {"proccode": "delay", "argumentnames": "[]", "argumentids": "[]", "argumentdefaults": "[]", "warp": "true"}},
                        "wait": {"opcode": "control_wait", "next": "say", "parent": "definition", "inputs": {"DURATION": [1, [4, "0.25"]]}, "fields": {}},
                        "say": {"opcode": "looks_say", "next": null, "parent": "wait", "inputs": {"MESSAGE": [1, [10, "done"]]}, "fields": {}}
                    }
                }]
            }).to_string(),
        ).unwrap();

    let output = generate(&project);
    assert!(!output.contains("wait_deadlines"));
    assert!(output.contains("__aida_set_pc!(script, 1);"));
    assert!(output.contains("ScriptStatus::WaitUntil(wait_deadline)"));
    assert!(output.contains("clock.now_millis().saturating_add(duration_ms)"));
    assert!(!output.contains("clock.now_millis() < wait_deadline"));
    assert!(output.contains("inherited_warp || true"));
    assert!(output.contains("fn new_clone_scripts(target: usize, _instance: usize)"));
    assert!(!output.contains("if script.warp_budget_expired(clock) { return ScriptStatus::Yield; }\n                            continue;"));
    assert!(output.contains("pub fn advance(&mut self) -> SchedulerStatus"));
}

#[test]
fn resumable_procedure_body_is_emitted_once_for_multiple_scripts() {
    let mut blocks = json!({
        "first_hat": {"opcode":"event_whenflagclicked", "next":"first_call", "topLevel":true, "inputs":{}, "fields":{}},
        "first_call": {"opcode":"procedures_call", "next":null, "parent":"first_hat", "inputs":{}, "fields":{}, "mutation":{"proccode":"shared delay", "argumentids":"[]", "warp":"false"}},
        "second_hat": {"opcode":"event_whenflagclicked", "next":"second_call", "topLevel":true, "inputs":{}, "fields":{}},
        "second_call": {"opcode":"procedures_call", "next":null, "parent":"second_hat", "inputs":{}, "fields":{}, "mutation":{"proccode":"shared delay", "argumentids":"[]", "warp":"false"}},
        "definition": {"opcode":"procedures_definition", "next":"wait0", "topLevel":true, "inputs":{"custom_block":[1,"prototype"]}, "fields":{}},
        "prototype": {"opcode":"procedures_prototype", "parent":"definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"shared delay", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"false"}}
    });
    for index in 0..12 {
        let next = (index < 11).then(|| format!("wait{}", index + 1));
        blocks.as_object_mut().unwrap().insert(
            format!("wait{index}"),
            json!({
                "opcode":"control_wait",
                "next":next,
                "parent":if index == 0 {"definition".to_owned()} else {"wait".to_owned() + &(index - 1).to_string()},
                "inputs":{"DURATION":[1,[4,"0.25"]]},
                "fields":{}
            }),
        );
    }
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1",
                "isStage": false,
                "variables": {},
                "blocks": blocks
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert_eq!(output.matches("fn step_procedures_0(").count(), 1);
    assert_eq!(output.matches("=> match pc {").count(), 1);
    assert_eq!(output.matches("script.push_frame(").count(), 1);
    assert!(output.contains("use self::step_green_flag_script_sprite1_"));
    assert!(output.contains(" as step_green_flag_script_sprite1_"));

    let step_start = output.find("fn step_green_flag_script_sprite1_").unwrap();
    let step_end = output[step_start..].find("\n}\n").unwrap() + step_start;
    let first_script = &output[step_start..step_end];
    assert!(!first_script.contains("ScriptStatus::WaitUntil"));
}
