use super::*;

#[test]
fn legacy_vm_noops_keep_their_command_and_undefined_reporter_behavior() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1", "isStage": false,
                    "blocks": {
                        "hat": {"opcode":"event_whenflagclicked", "next":"hide_all", "topLevel":true},
                        "hide_all": {"opcode":"looks_hideallsprites", "next":"change_stretch", "parent":"hat"},
                        "change_stretch": {"opcode":"looks_changestretchby", "next":"set_stretch", "parent":"hide_all"},
                        "set_stretch": {"opcode":"looks_setstretchto", "next":"scroll_right", "parent":"change_stretch"},
                        "scroll_right": {"opcode":"motion_scroll_right", "next":"scroll_up", "parent":"set_stretch"},
                        "scroll_up": {"opcode":"motion_scroll_up", "next":"align_scene", "parent":"scroll_right"},
                        "align_scene": {"opcode":"motion_align_scene", "next":"say_userid", "parent":"scroll_up"},
                        "say_userid": {"opcode":"looks_say", "next":"say_xscroll", "parent":"align_scene", "inputs":{"MESSAGE":[2,"reporter_userid"]}},
                        "reporter_userid": {"opcode":"sensing_userid", "parent":"say_userid"},
                        "say_xscroll": {"opcode":"looks_say", "next":"say_yscroll", "parent":"say_userid", "inputs":{"MESSAGE":[2,"reporter_xscroll"]}},
                        "reporter_xscroll": {"opcode":"motion_xscroll", "parent":"say_xscroll"},
                        "say_yscroll": {"opcode":"looks_say", "parent":"say_xscroll", "inputs":{"MESSAGE":[2,"reporter_yscroll"]}},
                        "reporter_yscroll": {"opcode":"motion_yscroll", "parent":"say_yscroll"}
                    }
                }]
            })
            .to_string(),
        )
        .unwrap();
    let output = generate(&project);

    assert_eq!(output.matches("let _ = ();\n").count(), 6);
    assert_eq!(output.matches("ScratchValue::Undefined").count(), 3);
    for opcode in [
        "looks_hideallsprites",
        "looks_changestretchby",
        "looks_setstretchto",
        "motion_scroll_right",
        "motion_scroll_up",
        "motion_align_scene",
        "sensing_userid",
        "motion_xscroll",
        "motion_yscroll",
    ] {
        assert!(!output.contains(&format!("unsupported statement: {opcode}")));
        assert!(!output.contains(&format!("unsupported reporter: {opcode}")));
    }
}

#[test]
fn stop_loop_fixture_lowers_this_script_inside_a_custom_block_and_stop_all() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/stop-loop-test.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(output.contains("if script.frames.len() > 1 { script.pop_frame(); continue; }"));
    assert!(output.contains("return ScriptStatus::StopAll;"));
    assert!(!output.contains("unsupported statement: control_stop"));
}

#[test]
fn consecutive_timed_think_blocks_resume_at_each_block_and_reach_following_work() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/think.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(
        output.contains("\n                1 => {\n                    let scratch_bubble_text")
    );
    assert!(
        output.contains("\n                2 => {\n                    let scratch_bubble_text")
    );
    assert_eq!(
        output
            .matches("state.set_speech_bubble(BubbleKind::Think")
            .count(),
        2
    );
    assert!(output.contains("state.set_shared(0, scratch_new_value);"));
}

#[test]
fn stop_other_scripts_in_sprite_keeps_the_current_script_continuation() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1",
                    "isStage": false,
                    "variables": {},
                    "blocks": {
                        "hat": {"opcode": "event_whenflagclicked", "next": "stop", "topLevel": true, "inputs": {}, "fields": {}},
                        "stop": {"opcode": "control_stop", "next": "say", "parent": "hat", "inputs": {}, "fields": {"STOP_OPTION": ["other scripts in sprite", null]}},
                        "say": {"opcode": "looks_say", "next": null, "parent": "stop", "inputs": {"MESSAGE": [1, [10, "after stop"]]}, "fields": {}}
                    }
                }]
            })
            .to_string(),
        )
        .unwrap();
    let output = generate(&project);

    assert!(output.contains("return ScriptStatus::StopOtherScripts;"));
    assert!(output.contains("__aida_set_pc!(script, 1);"));
    assert!(!output.contains("unsupported statement: control_stop"));
}

#[test]
fn if_else_fixture_lowers_conditional_branches() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/if-else-test.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(output.contains("if scratch_and("));
    assert!(output.contains("} else {"));
    assert!(!output.contains("unsupported statement: control_if"));
    assert!(!output.contains("unsupported statement: control_if_else"));
}

#[test]
fn synchronous_conditional_branches_keep_valid_dispatch_states() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1",
                "isStage": false,
                "variables": {"flag": ["flag", 0], "value": ["value", 0]},
                "blocks": {
                    "hat": {"opcode": "event_whenflagclicked", "next": "choose", "topLevel": true, "inputs": {}, "fields": {}},
                    "choose": {"opcode": "control_if_else", "parent": "hat", "next": "finish", "inputs": {"CONDITION": [2, "flag_value"], "SUBSTACK": [2, "set_yes"], "SUBSTACK2": [2, "set_no"]}, "fields": {}},
                    "flag_value": {"opcode": "data_variable", "parent": "choose", "inputs": {}, "fields": {"VARIABLE": ["flag", "flag"]}},
                    "set_yes": {"opcode": "data_setvariableto", "parent": "choose", "inputs": {"VALUE": [1, [4, "1"]]}, "fields": {"VARIABLE": ["value", "value"]}},
                    "set_no": {"opcode": "data_setvariableto", "parent": "choose", "inputs": {"VALUE": [1, [4, "2"]]}, "fields": {"VARIABLE": ["value", "value"]}},
                    "finish": {"opcode": "data_changevariableby", "parent": "choose", "inputs": {"VALUE": [1, [4, "1"]]}, "fields": {"VARIABLE": ["value", "value"]}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&project);

    assert!(output.contains("if scratch_bool(state.get("));
    assert!(output.contains("state.set_local_number("));
    assert_eq!(count_resumable_pc_arms(&output), 4);
}

#[test]
fn loop_body_keeps_states_needed_after_a_warp_call_yields() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1",
                "isStage": false,
                "variables": {"count": ["count", 0]},
                "blocks": {
                    "hat": {"opcode": "event_whenflagclicked", "next": "call_caller", "topLevel": true, "inputs": {}, "fields": {}},
                    "call_caller": {"opcode": "procedures_call", "parent": "hat", "inputs": {}, "fields": {}, "mutation": {"proccode": "caller", "argumentids": "[]", "warp": "false"}},
                    "caller_definition": {"opcode": "procedures_definition", "next": "repeat", "topLevel": true, "inputs": {"custom_block": [1, "caller_prototype"]}, "fields": {}},
                    "caller_prototype": {"opcode": "procedures_prototype", "parent": "caller_definition", "inputs": {}, "fields": {}, "mutation": {"proccode": "caller", "argumentnames": "[]", "argumentids": "[]", "argumentdefaults": "[]", "warp": "true"}},
                    "repeat": {"opcode": "control_repeat", "parent": "caller_definition", "inputs": {"TIMES": [1, [4, "2"]], "SUBSTACK": [2, "call_helper"]}, "fields": {}},
                    "call_helper": {"opcode": "procedures_call", "parent": "repeat", "next": "increment", "inputs": {}, "fields": {}, "mutation": {"proccode": "helper", "argumentids": "[]", "warp": "false"}},
                    "increment": {"opcode": "data_changevariableby", "parent": "call_helper", "inputs": {"VALUE": [1, [4, "1"]]}, "fields": {"VARIABLE": ["count", "count"]}},
                    "helper_definition": {"opcode": "procedures_definition", "next": "helper_increment", "topLevel": true, "inputs": {"custom_block": [1, "helper_prototype"]}, "fields": {}},
                    "helper_prototype": {"opcode": "procedures_prototype", "parent": "helper_definition", "inputs": {}, "fields": {}, "mutation": {"proccode": "helper", "argumentnames": "[]", "argumentids": "[]", "argumentdefaults": "[]", "warp": "false"}},
                    "helper_increment": {"opcode": "data_changevariableby", "parent": "helper_definition", "inputs": {"VALUE": [1, [4, "1"]]}, "fields": {"VARIABLE": ["count", "count"]}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&project);

    assert!(output.contains("fn direct_procedure_"));
    assert!(output.contains("script.push_frame("));
    assert!(output.contains("repeat_counts["));
    assert!(count_resumable_pc_arms(&output) >= 4);
}

#[test]
fn straight_line_blocks_share_one_resumable_dispatch_arm() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1",
                    "isStage": false,
                    "variables": {"first": ["first", 0], "second": ["second", 0]},
                    "blocks": {
                        "hat": {"opcode": "event_whenflagclicked", "next": "set_first", "topLevel": true, "inputs": {}, "fields": {}},
                        "set_first": {"opcode": "data_setvariableto", "next": "set_second", "inputs": {"VALUE": [1, [4, "1"]]}, "fields": {"VARIABLE": ["first", "first"]}},
                        "set_second": {"opcode": "data_setvariableto", "next": null, "inputs": {"VALUE": [1, [4, "2"]]}, "fields": {"VARIABLE": ["second", "second"]}}
                    }
                }]
            })
            .to_string(),
        )
        .unwrap();

    let output = generate(&project);
    // straight-line numeric assignments avoid temporary scratch values.
    assert!(output.contains("state.set_local_number(0, 1.0_f64);"));
    assert!(output.contains("state.set_local_number(1, 2.0_f64);"));
    assert!(!output.contains("__aida_set_pc!(script, 1);"));
    assert!(output.contains("__aida_continue_at!(script, usize::MAX);"));
    assert!(output.contains("fn frame(&self) -> &ProcedureFrame"));
    assert!(output.contains("fn frame_mut(&mut self) -> &mut ProcedureFrame"));
    assert!(!output.contains("script.frames.last().unwrap()"));
    assert!(!output.contains("script.frames.last_mut().unwrap()"));
}

#[test]
fn lowers_nested_repeat_until_and_forever_with_resumable_loop_state() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1",
                    "isStage": false,
                    "variables": {
                        "count_id": ["count", 2],
                        "done_id": ["done", false]
                    },
                    "blocks": {
                        "hat": {
                            "opcode": "event_whenflagclicked",
                            "next": "outer_repeat",
                            "topLevel": true,
                            "inputs": {},
                            "fields": {}
                        },
                        "outer_repeat": {
                            "opcode": "control_repeat",
                            "next": null,
                            "parent": "hat",
                            "inputs": {"TIMES": [1, [4, "2"]], "SUBSTACK": [2, "inner_repeat"]},
                            "fields": {}
                        },
                        "inner_repeat": {
                            "opcode": "control_repeat",
                            "next": null,
                            "parent": "outer_repeat",
                            "inputs": {"TIMES": [3, "count_reporter", [10, "0"]], "SUBSTACK": [2, "until"]},
                            "fields": {}
                        },
                        "count_reporter": {
                            "opcode": "data_variable",
                            "parent": "inner_repeat",
                            "inputs": {},
                            "fields": {"VARIABLE": ["count", "count_id"]}
                        },
                        "until": {
                            "opcode": "control_repeat_until",
                            "next": null,
                            "parent": "inner_repeat",
                            "inputs": {"CONDITION": [2, "done_reporter"], "SUBSTACK": [2, "forever"]},
                            "fields": {}
                        },
                        "done_reporter": {
                            "opcode": "data_variable",
                            "parent": "until",
                            "inputs": {},
                            "fields": {"VARIABLE": ["done", "done_id"]}
                        },
                        "forever": {
                            "opcode": "control_forever",
                            "next": null,
                            "parent": "until",
                            "inputs": {"SUBSTACK": [2, "set_done"]},
                            "fields": {}
                        },
                        "set_done": {
                            "opcode": "data_setvariableto",
                            "next": null,
                            "parent": "forever",
                            "inputs": {"VALUE": [1, [4, "1"]]},
                            "fields": {"VARIABLE": ["done", "done_id"]}
                        }
                    }
                }]
            })
            .to_string(),
        )
        .unwrap();

    let output = generate(&project);
    assert!(output.contains("smallvec::smallvec![0; 2]"));
    assert!(output.contains("script.frame_mut().repeat_counts[0] = repeat_count;"));
    assert!(output.contains("script.frame_mut().repeat_counts[1] = repeat_count;"));
    assert_eq!(
        output.matches("let repeat_count = scratch_round").count(),
        2
    );
    assert!(output.contains("if !(scratch_bool(state.get(1)))"));
    assert!(output.contains("__aida_continue_at!(script, 8);"));
    assert!(output.contains("return ScriptStatus::Yield;"));
}

#[test]
fn structured_loop_merges_nested_numeric_branches_and_commits_before_yield() {
    let project = parse_project(&json!({
            "targets": [{
                "name": "Sprite1", "isStage": false,
                "variables": {"counter": ["counter", "0"], "flag": ["flag", "0"], "copy": ["copy", ""]},
                "blocks": {
                    "hat": {"opcode": "event_whenflagclicked", "next": "repeat", "topLevel": true, "inputs": {}, "fields": {}},
                    "repeat": {"opcode": "control_repeat", "parent": "hat", "inputs": {"TIMES": [1, [4, "3"]], "SUBSTACK": [2, "if"]}, "fields": {}},
                    "if": {"opcode": "control_if_else", "parent": "repeat", "next": "change", "inputs": {"CONDITION": [2, "equal"], "SUBSTACK": [2, "yes"], "SUBSTACK2": [2, "no"]}, "fields": {}},
                    "equal": {"opcode": "operator_equals", "parent": "if", "inputs": {"OPERAND1": [3, [12, "flag", "flag"], [4, "0"]], "OPERAND2": [1, [4, "0"]]}, "fields": {}},
                    "yes": {"opcode": "data_setvariableto", "parent": "if", "inputs": {"VALUE": [1, [10, "1"]]}, "fields": {"VARIABLE": ["flag", "flag"]}},
                    "no": {"opcode": "data_setvariableto", "parent": "if", "inputs": {"VALUE": [1, [10, "0"]]}, "fields": {"VARIABLE": ["flag", "flag"]}},
                    "change": {"opcode": "data_changevariableby", "parent": "repeat", "inputs": {"VALUE": [1, [4, "1"]]}, "fields": {"VARIABLE": ["counter", "counter"]}}
                }
            }]
        }).to_string()).unwrap();
    let output = generate(&project);
    assert!(output.contains("state.can_defer_variable_write_mask(&[0x0000000000000006_u64])"));
    assert!(output.contains("scratch_typed_2 = 1.0_f64;"));
    assert!(output.contains("scratch_typed_2 = 0.0_f64;"));
    assert!(output.contains("scratch_dirty_1 = true;"));
    let normalized = output.split_whitespace().collect::<Vec<_>>().join(" ");
    let yield_branch = normalized
        .split("if !scratch_warp_mode || script.warp_budget.expired(clock) {")
        .nth(1)
        .unwrap();
    let before_yield = yield_branch
        .split("return ScriptStatus::Yield;")
        .next()
        .unwrap();
    assert!(
        before_yield.contains("if scratch_dirty_1 { state.set_local_number(1, scratch_typed_1); }")
    );
    assert!(before_yield.contains("ScratchValue::Text(scratch_string(scratch_typed_2))"));
    assert!(
        output.contains("state.set_local_number(1, scratch_typed_1);"),
        "proven numeric writes should commit the numeric local directly"
    );
    assert_eq!(count_resumable_pc_arms(&output), 2);
}

#[test]
fn structured_loop_uses_numeric_codegen_when_type_is_proven_even_if_nan_is_possible() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1", "isStage": false,
                "variables": {"counter": ["counter", 0], "flag": ["flag", 0]},
                "blocks": {
                    "hat": {"opcode":"event_whenflagclicked", "next":"repeat", "topLevel":true},
                    "repeat": {"opcode":"control_repeat", "parent":"hat", "inputs":{"TIMES":[1,[4,"20"]],"SUBSTACK":[2,"if"]}},
                    "if": {"opcode":"control_if_else", "parent":"repeat", "next":"change", "inputs":{"CONDITION":[2,"equal"],"SUBSTACK":[2,"yes"],"SUBSTACK2":[2,"no"]}},
                    "equal": {"opcode":"operator_equals", "parent":"if", "inputs":{"OPERAND1":[3,[12,"flag","flag"],[4,"0"]],"OPERAND2":[1,[4,"0"]]}},
                    "yes": {"opcode":"data_setvariableto", "parent":"if", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["flag","flag"]}},
                    "no": {"opcode":"data_setvariableto", "parent":"if", "inputs":{"VALUE":[1,[4,"0"]]}, "fields":{"VARIABLE":["flag","flag"]}},
                    "change": {"opcode":"data_changevariableby", "parent":"repeat", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["counter","counter"]}},
                    "nan_hat": {"opcode":"event_whenkeypressed", "next":"set_nan", "topLevel":true, "fields":{"KEY_OPTION":["space",null]}},
                    "set_nan": {"opcode":"data_setvariableto", "parent":"nan_hat", "inputs":{"VALUE":[3,"divide"]}, "fields":{"VARIABLE":["counter","counter"]}},
                    "divide": {"opcode":"operator_divide", "parent":"set_nan", "inputs":{"NUM1":[1,[4,"0"]],"NUM2":[1,[4,"0"]]}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&project);

    assert!(output.contains("number_or_nan()"));
    assert!(!output.contains("canonical_number()"));
    assert!(!output.contains("state.change_local_number("));
    assert!(output.contains("let scratch_defer_writes = state.can_defer_variable_write_mask(&["));
    assert!(output.contains("if scratch_defer_writes { scratch_dirty_"));
    assert!(output.contains("else { state.set_local_number("));
}

#[test]
fn structured_loop_keeps_a_dynamic_fallback_for_variables_written_as_text_elsewhere() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1", "isStage": false,
                "variables": {"counter": ["counter", 0], "flag": ["flag", 0]},
                "blocks": {
                    "hat": {"opcode":"event_whenflagclicked", "next":"repeat", "topLevel":true},
                    "repeat": {"opcode":"control_repeat", "parent":"hat", "inputs":{"TIMES":[1,[4,"20"]],"SUBSTACK":[2,"if"]}},
                    "if": {"opcode":"control_if_else", "parent":"repeat", "next":"change", "inputs":{"CONDITION":[2,"equal"],"SUBSTACK":[2,"yes"],"SUBSTACK2":[2,"no"]}},
                    "equal": {"opcode":"operator_equals", "parent":"if", "inputs":{"OPERAND1":[3,[12,"flag","flag"],[4,"0"]],"OPERAND2":[1,[4,"0"]]}},
                    "yes": {"opcode":"data_setvariableto", "parent":"if", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["flag","flag"]}},
                    "no": {"opcode":"data_setvariableto", "parent":"if", "inputs":{"VALUE":[1,[4,"0"]]}, "fields":{"VARIABLE":["flag","flag"]}},
                    "change": {"opcode":"data_changevariableby", "parent":"repeat", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["counter","counter"]}},
                    "other_hat": {"opcode":"event_whenkeypressed", "next":"set_text", "topLevel":true, "fields":{"KEY_OPTION":["space",null]}},
                    "set_text": {"opcode":"data_setvariableto", "parent":"other_hat", "inputs":{"VALUE":[1,[10,"text"]]}, "fields":{"VARIABLE":["counter","counter"]}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&project);

    assert!(output.contains("canonical_number()"));
    assert!(output.contains("state.change_local_number("));
    let guard = output
        .lines()
        .find(|line| line.contains("canonical_number()") && line.contains("if let"))
        .unwrap();
    assert!(!guard.contains("can_defer_variable_writes"));
    assert!(output.contains("let scratch_defer_writes = state.can_defer_variable_write_mask(&["));
}

fn count_resumable_pc_arms(source: &str) -> usize {
    source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            let indentation = line.len() - trimmed.len();
            indentation >= 16
                && trimmed
                    .split_once(" => {")
                    .is_some_and(|(pc, _)| pc.parse::<usize>().is_ok())
        })
        .count()
}
