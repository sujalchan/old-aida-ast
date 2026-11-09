use super::*;
use crate::parser::ScratchProject;

fn numeric_argument_project(mixed_callsite: bool) -> ScratchProject {
    let mut blocks = json!({
        "hat": {"opcode":"event_whenflagclicked", "next":"call_number", "topLevel":true, "inputs":{}, "fields":{}},
        "call_number": {"opcode":"procedures_call", "parent":"hat", "inputs":{"argument_id":[1,[4,"2"]]}, "fields":{}, "mutation":{"proccode":"add one %s", "argumentids":"[\"argument_id\"]", "warp":"true"}},
        "definition": {"opcode":"procedures_definition", "next":"set_result", "topLevel":true, "inputs":{"custom_block":[1,"prototype"]}, "fields":{}},
        "prototype": {"opcode":"procedures_prototype", "parent":"definition", "inputs":{"argument_id":[1,"argument_default"]}, "fields":{}, "mutation":{"proccode":"add one %s", "argumentnames":"[\"value\"]", "argumentids":"[\"argument_id\"]", "argumentdefaults":"[\"\"]", "warp":"true"}},
        "argument_default": {"opcode":"argument_reporter_string_number", "parent":"prototype", "shadow":true, "inputs":{}, "fields":{"VALUE":["value",null]}},
        "set_result": {"opcode":"data_setvariableto", "parent":"definition", "inputs":{"VALUE":[3,"add",[4,"1"]]}, "fields":{"VARIABLE":["result","result_id"]}},
        "add": {"opcode":"operator_add", "parent":"set_result", "inputs":{"NUM1":[2,"argument"], "NUM2":[1,[4,"1"]]}, "fields":{}},
        "argument": {"opcode":"argument_reporter_string_number", "parent":"add", "inputs":{}, "fields":{"VALUE":["value",null]}}
    });
    if mixed_callsite {
        blocks.as_object_mut().unwrap().insert(
            "second_hat".to_owned(),
                json!({"opcode":"event_whenflagclicked", "next":"call_text", "topLevel":true, "inputs":{}, "fields":{}}),
        );
        blocks.as_object_mut().unwrap().insert(
            "call_text".to_owned(),
            json!({"opcode":"procedures_call", "parent":"second_hat", "inputs":{"argument_id":[1,[10,"1"]]}, "fields":{}, "mutation":{"proccode":"add one %s", "argumentids":"[\"argument_id\"]", "warp":"true"}}),
        );
    }
    parse_project(
        &json!({
            "targets": [{
                "name":"Sprite1",
                "isStage":false,
                "variables":{"result_id":["result",0]},
                "blocks":blocks
            }]
        })
        .to_string(),
    )
    .unwrap()
}

fn numeric_warp_argument_project(mixed_callsite: bool) -> ScratchProject {
    let mut variables = serde_json::Map::new();
    let mut blocks = serde_json::Map::new();
    for index in 0..12 {
        variables.insert(
            format!("result_{index}_id"),
            json!([format!("result {index}"), 0]),
        );
    }
    blocks.insert(
        "hat".to_owned(),
        json!({"opcode":"event_whenflagclicked", "next":"call_number", "topLevel":true, "inputs":{}, "fields":{}}),
    );
    blocks.insert(
        "call_number".to_owned(),
        json!({"opcode":"procedures_call", "parent":"hat", "inputs":{"argument_id":[1,[4,"2"]]}, "fields":{}, "mutation":{"proccode":"warp numeric %s", "argumentids":"[\"argument_id\"]", "warp":"true"}}),
    );
    blocks.insert(
        "outer_definition".to_owned(),
        json!({"opcode":"procedures_definition", "next":"call_inner", "topLevel":true, "inputs":{"custom_block":[1,"outer_prototype"]}, "fields":{}}),
    );
    blocks.insert(
        "outer_prototype".to_owned(),
        json!({"opcode":"procedures_prototype", "parent":"outer_definition", "inputs":{"argument_id":[1,"argument_default"]}, "fields":{}, "mutation":{"proccode":"warp numeric %s", "argumentnames":"[\"value\"]", "argumentids":"[\"argument_id\"]", "argumentdefaults":"[\"\"]", "warp":"true"}}),
    );
    blocks.insert(
        "argument_default".to_owned(),
        json!({"opcode":"argument_reporter_string_number", "parent":"outer_prototype", "shadow":true, "inputs":{}, "fields":{"VALUE":["value",null]}}),
    );
    blocks.insert(
        "call_inner".to_owned(),
        json!({"opcode":"procedures_call", "next":"set_0", "parent":"outer_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentids":"[]", "warp":"true"}}),
    );
    blocks.insert(
        "inner_definition".to_owned(),
        json!({"opcode":"procedures_definition", "topLevel":true, "inputs":{"custom_block":[1,"inner_prototype"]}, "fields":{}}),
    );
    blocks.insert(
        "inner_prototype".to_owned(),
        json!({"opcode":"procedures_prototype", "parent":"inner_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}}),
    );
    for index in 0..12 {
        let next = (index < 11).then(|| format!("set_{}", index + 1));
        let parent = if index == 0 {
            "call_inner".to_owned()
        } else {
            format!("set_{}", index - 1)
        };
        blocks.insert(
            format!("set_{index}"),
            json!({"opcode":"data_setvariableto", "next":next, "parent":parent, "inputs":{"VALUE":[3,format!("sum_{index}")]}, "fields":{"VARIABLE":[format!("result {index}"),format!("result_{index}_id")]}}),
        );
        blocks.insert(
            format!("sum_{index}"),
            json!({"opcode":"operator_add", "parent":format!("set_{index}"), "inputs":{"NUM1":[2,"argument"], "NUM2":[1,[4,"1"]]}, "fields":{}}),
        );
    }
    blocks.insert(
        "argument".to_owned(),
        json!({"opcode":"argument_reporter_string_number", "parent":"sum_0", "inputs":{}, "fields":{"VALUE":["value",null]}}),
    );
    if mixed_callsite {
        blocks.insert(
            "second_hat".to_owned(),
            json!({"opcode":"event_whenflagclicked", "next":"call_text", "topLevel":true, "inputs":{}, "fields":{}}),
        );
        blocks.insert(
            "call_text".to_owned(),
            json!({"opcode":"procedures_call", "parent":"second_hat", "inputs":{"argument_id":[1,[10,"text"]]}, "fields":{}, "mutation":{"proccode":"warp numeric %s", "argumentids":"[\"argument_id\"]", "warp":"true"}}),
        );
    }
    parse_project(
        &json!({
            "targets": [{
                "name":"Sprite1",
                "isStage":false,
                "variables":variables,
                "blocks":blocks
            }]
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn resumable_procedure_arguments_use_numeric_locals_when_all_callers_prove_numbers() {
    let output = generate(&numeric_argument_project(false));
    assert!(output.contains("let __aida_numeric_arg_0__ = script.numeric_argument(0);"));
    assert!(output.contains("(scratch_number_operand(__aida_numeric_arg_0__) + 1.0_f64)"));
    assert!(!output.contains("scratch_add(&__aida_numeric_arg_0__"));
    assert!(output.contains("_numbers(state, clock, numeric_value_0);"));
    assert!(output.contains("let numeric_value_0: f64 = (2.0_f64);"));
}

#[test]
fn resumable_procedure_arguments_keep_dynamic_paths_for_text_callers() {
    let output = generate(&numeric_argument_project(true));
    assert!(output.contains("__aida_numeric_arg_0__"));
    assert!(output.contains("scratch_add(&argument_0, 1.0_f64)"));
}

#[test]
fn bounded_warp_with_proven_numeric_arguments_generates_a_numeric_only_call_path() {
    let output = generate(&numeric_warp_argument_project(false));
    assert!(output.contains("fn direct_warp_procedure_0_"));
    assert!(output.contains("argument_0: f64"));
    assert!(output.contains("let direct_warp_arg_0: f64 = 2.0_f64;"));
    assert!(output.contains("state, clock, direct_warp_arg_0);"));
    assert!(output.contains("set_existing_local_number"));
    assert!(
        output.contains(
            "let scratch_operand_argument_0 = scratch_number_operand(numeric_argument_0);"
        )
    );
    assert!(!output.contains("let Some(numeric_argument_0) = argument_0.canonical_number()"));
    assert!(!output.contains("_dynamic(state"));
    assert!(output.contains("if scratch_defer_writes { scratch_dirty_0 = true; } else { state.set_existing_local_number(0, numeric_new); }"));
    assert!(!output.contains("if scratch_defer_writes { numeric_0 = numeric_new;"));
}

#[test]
fn bounded_warp_keeps_dynamic_fallback_when_a_text_callsite_exists() {
    let output = generate(&numeric_warp_argument_project(true));
    assert!(output.contains("let Some(numeric_argument_0) = argument_0.canonical_number()"));
    assert!(output.contains("fn direct_warp_procedure_0_") && output.contains("_dynamic(state"));
}

#[test]
fn bounded_warp_procedure_calls_run_directly_with_budget_checkpoints() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name":"Sprite1",
                "isStage":false,
                "variables":{"result_id":["result",0]},
                "blocks":{
                    "hat":{"opcode":"event_whenflagclicked", "next":"call_outer", "topLevel":true, "inputs":{}, "fields":{}},
                    "call_outer":{"opcode":"procedures_call", "parent":"hat", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentids":"[]", "warp":"true"}},
                    "outer_definition":{"opcode":"procedures_definition", "next":"call_inner", "topLevel":true, "inputs":{"custom_block":[1,"outer_prototype"]}, "fields":{}},
                    "outer_prototype":{"opcode":"procedures_prototype", "parent":"outer_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
                    "call_inner":{"opcode":"procedures_call", "parent":"outer_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentids":"[]", "warp":"true"}},
                    "inner_definition":{"opcode":"procedures_definition", "next":"set_result", "topLevel":true, "inputs":{"custom_block":[1,"inner_prototype"]}, "fields":{}},
                    "inner_prototype":{"opcode":"procedures_prototype", "parent":"inner_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
                    "set_result":{"opcode":"data_setvariableto", "parent":"inner_definition", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["result","result_id"]}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert!(output.contains("fn direct_warp_procedure_0_"));
    assert!(output.contains("direct_warp_procedure_0_"));
    assert!(output.matches("script.warp_budget.expired(clock)").count() >= 2);
}

#[test]
fn resumable_warp_calls_skip_recursive_scan_when_warp_is_guaranteed() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name":"Sprite1",
                "isStage":false,
                "variables":{"result_id":["result",0]},
                "blocks":{
                    "hat":{"opcode":"event_whenflagclicked", "next":"call_outer", "topLevel":true, "inputs":{}, "fields":{}},
                    "call_outer":{"opcode":"procedures_call", "parent":"hat", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentids":"[]", "warp":"true"}},
                    "outer_definition":{"opcode":"procedures_definition", "next":"outer_repeat", "topLevel":true, "inputs":{"custom_block":[1,"outer_prototype"]}, "fields":{}},
                    "outer_prototype":{"opcode":"procedures_prototype", "parent":"outer_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
                    "outer_repeat":{"opcode":"control_repeat", "next":null, "parent":"outer_definition", "inputs":{"TIMES":[1,[4,"2"]],"SUBSTACK":[2,"call_inner"]}, "fields":{}},
                    "call_inner":{"opcode":"procedures_call", "next":"outer_change", "parent":"outer_repeat", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentids":"[]", "warp":"true"}},
                    "outer_change":{"opcode":"data_changevariableby", "parent":"call_inner", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["result","result_id"]}},
                    "inner_definition":{"opcode":"procedures_definition", "next":"inner_repeat", "topLevel":true, "inputs":{"custom_block":[1,"inner_prototype"]}, "fields":{}},
                    "inner_prototype":{"opcode":"procedures_prototype", "parent":"inner_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
                    "inner_repeat":{"opcode":"control_repeat", "next":null, "parent":"inner_definition", "inputs":{"TIMES":[1,[4,"2"]],"SUBSTACK":[2,"inner_change"]}, "fields":{}},
                    "inner_change":{"opcode":"data_changevariableby", "parent":"inner_repeat", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["result","result_id"]}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert!(!output.contains("let recursive_call = script.frames.iter()"));
    assert!(output.contains("scratch_tail_call_arguments"));
    assert!(output.contains("script.push_frame("));
    assert!(
        output.contains("if script.warp_budget.expired(clock) { return ScriptStatus::Yield; }")
    );
}

#[test]
fn bounded_warp_stop_blocks_keep_status_when_called_directly() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name":"Sprite1",
                "isStage":false,
                "variables":{"result_id":["result",0]},
                "blocks":{
                    "hat":{"opcode":"event_whenflagclicked", "next":"call_outer", "topLevel":true, "inputs":{}, "fields":{}},
                    "call_outer":{"opcode":"procedures_call", "parent":"hat", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentids":"[]", "warp":"true"}},
                    "outer_definition":{"opcode":"procedures_definition", "next":"call_inner", "topLevel":true, "inputs":{"custom_block":[1,"outer_prototype"]}, "fields":{}},
                    "outer_prototype":{"opcode":"procedures_prototype", "parent":"outer_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
                    "call_inner":{"opcode":"procedures_call", "next":"stop_this", "parent":"outer_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentids":"[]", "warp":"true"}},
                    "stop_this":{"opcode":"control_stop", "next":"stop_all", "parent":"outer_definition", "inputs":{}, "fields":{"STOP_OPTION":["this script",null]}},
                    "stop_all":{"opcode":"control_stop", "parent":"outer_definition", "inputs":{}, "fields":{"STOP_OPTION":["all",null]}},
                    "inner_definition":{"opcode":"procedures_definition", "next":"set_result", "topLevel":true, "inputs":{"custom_block":[1,"inner_prototype"]}, "fields":{}},
                    "inner_prototype":{"opcode":"procedures_prototype", "parent":"inner_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
                    "set_result":{"opcode":"data_setvariableto", "parent":"inner_definition", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["result","result_id"]}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert!(output.contains("-> DirectWarpStatus"));
    assert!(output.contains("return DirectWarpStatus::ProcedureReturn;"));
    assert!(output.contains("return DirectWarpStatus::StopAll;"));
    assert!(output.contains("matches!(direct_warp_procedure_0_"));
    assert!(output.contains("return ScriptStatus::StopAll;"));
}

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
    assert!(output.contains("script.argument(0)).clone()"));
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

#[test]
fn loopless_warp_procedure_is_inlined_into_its_unique_warp_caller() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1",
                "isStage": false,
                "variables": {},
                "blocks": {
                    "hat": {"opcode":"event_whenflagclicked", "next":"call_outer", "topLevel":true, "inputs":{}, "fields":{}},
                    "call_outer": {"opcode":"procedures_call", "parent":"hat", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentids":"[]", "warp":"true"}},
                    "outer_definition": {"opcode":"procedures_definition", "next":"call_inner", "topLevel":true, "inputs":{"custom_block":[1,"outer_prototype"]}, "fields":{}},
                    "outer_prototype": {"opcode":"procedures_prototype", "parent":"outer_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
                    "call_inner": {"opcode":"procedures_call", "next":"call_looping", "parent":"outer_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentids":"[]", "warp":"true"}},
                    "call_looping": {"opcode":"procedures_call", "next":"outer_say", "parent":"call_inner", "inputs":{}, "fields":{}, "mutation":{"proccode":"looping", "argumentids":"[]", "warp":"true"}},
                    "outer_say": {"opcode":"looks_say", "parent":"call_looping", "inputs":{"MESSAGE":[1,[10,"after"]]}, "fields":{}},
                    "inner_definition": {"opcode":"procedures_definition", "next":"inner_say", "topLevel":true, "inputs":{"custom_block":[1,"inner_prototype"]}, "fields":{}},
                    "inner_prototype": {"opcode":"procedures_prototype", "parent":"inner_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
                    "inner_say": {"opcode":"looks_say", "parent":"inner_definition", "inputs":{"MESSAGE":[1,[10,"inside"]]}, "fields":{}},
                    "looping_definition": {"opcode":"procedures_definition", "next":"loop", "topLevel":true, "inputs":{"custom_block":[1,"looping_prototype"]}, "fields":{}},
                    "looping_prototype": {"opcode":"procedures_prototype", "parent":"looping_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"looping", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
                    "loop": {"opcode":"control_repeat", "parent":"looping_definition", "inputs":{"TIMES":[1,[4,"2"]], "SUBSTACK":[2,"loop_say"]}, "fields":{}},
                    "loop_say": {"opcode":"looks_say", "parent":"loop", "inputs":{"MESSAGE":[1,[10,"repeat"]]}, "fields":{}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);

    // Stable frame ID for `inner_definition` in this fixture.
    assert!(!output.contains("script.push_frame(1891945689,"));
    assert!(!output.contains("1891945689 => match pc"));
    // The looping procedure keeps its resumable frame and loop boundary.
    assert!(output.contains("script.push_frame(291600351,"));
    assert!(output.contains("if script.warp_budget.expired(clock) {"));
    assert!(output.contains("state.set_speech_bubble"));
}

#[test]
fn inlined_stop_this_script_returns_to_the_inline_callsite_continuation() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1",
                "isStage": false,
                "variables": {},
                "blocks": {
                    "hat": {"opcode":"event_whenflagclicked", "next":"call_outer", "topLevel":true, "inputs":{}, "fields":{}},
                    "call_outer": {"opcode":"procedures_call", "parent":"hat", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentids":"[]", "warp":"true"}},
                    "outer_definition": {"opcode":"procedures_definition", "next":"call_inner", "topLevel":true, "inputs":{"custom_block":[1,"outer_prototype"]}, "fields":{}},
                    "outer_prototype": {"opcode":"procedures_prototype", "parent":"outer_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"outer", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
                    "call_inner": {"opcode":"procedures_call", "next":"after_inner", "parent":"outer_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentids":"[]", "warp":"true"}},
                    "after_inner": {"opcode":"looks_say", "parent":"call_inner", "inputs":{"MESSAGE":[1,[10,"after"]]}, "fields":{}},
                    "inner_definition": {"opcode":"procedures_definition", "next":"inner_say", "topLevel":true, "inputs":{"custom_block":[1,"inner_prototype"]}, "fields":{}},
                    "inner_prototype": {"opcode":"procedures_prototype", "parent":"inner_definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"inner", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"true"}},
                    "inner_say": {"opcode":"looks_say", "next":"stop_this", "parent":"inner_definition", "inputs":{"MESSAGE":[1,[10,"inside"]]}, "fields":{}},
                    "stop_this": {"opcode":"control_stop", "parent":"inner_definition", "inputs":{}, "fields":{"STOP_OPTION":["this script",null]}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert!(output.contains("__aida_continue_at!(script, 1);"));
    assert!(!output.contains("if script.frames.len() > 1 { script.pop_frame(); continue; } return ScriptStatus::Complete;"));
}
