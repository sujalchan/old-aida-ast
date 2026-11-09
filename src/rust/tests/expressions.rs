use super::*;

#[test]
fn folds_finite_literal_arithmetic_at_compile_time() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1",
                "isStage": false,
                "variables": {"result_id": ["result", 0]},
                "blocks": {
                    "hat": {"opcode":"event_whenflagclicked", "next":"set", "topLevel":true, "inputs":{}, "fields":{}},
                    "set": {"opcode":"data_setvariableto", "next":null, "parent":"hat", "inputs":{"VALUE":[3,"sum",[4,"0"]]}, "fields":{"VARIABLE":["result","result_id"]}},
                    "sum": {"opcode":"operator_add", "parent":"set", "inputs":{"NUM1":[3,"product",[4,"0"]], "NUM2":[1,[4,"1.25"]]}, "fields":{}},
                    "product": {"opcode":"operator_multiply", "parent":"sum", "inputs":{"NUM1":[1,[4,"2"]], "NUM2":[1,[4,"3"]]}, "fields":{}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert!(output.contains("state.set_local_number(0, 7.25_f64);"));
    assert!(!output.contains("scratch_add("));
    assert!(!output.contains("scratch_multiply("));
}

#[test]
fn lowers_dynamic_math_with_scratch_conversion_and_keeps_literal_types() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Sprite1",
                "isStage": false,
                "variables": {
                    "amount_id": ["amount", 5.5],
                    "same_id": ["same", false]
                },
                "blocks": {
                    "hat": {
                        "opcode": "event_whenflagclicked",
                        "next": "math_set",
                        "topLevel": true,
                        "inputs": {},
                        "fields": {}
                    },
                    "math_set": {
                        "opcode": "data_setvariableto",
                        "next": "compare_set",
                        "parent": "hat",
                        "inputs": {"VALUE": [1, "math"]},
                        "fields": {"VARIABLE": ["amount", "amount_id"]}
                    },
                    "math": {
                        "opcode": "operator_mathop",
                        "parent": "math_set",
                        "inputs": {"NUM": [3, "variable"]},
                        "fields": {"OPERATOR": ["floor", null]}
                    },
                    "variable": {
                        "opcode": "data_variable",
                        "parent": "math",
                        "inputs": {},
                        "fields": {"VARIABLE": ["amount", "amount_id"]}
                    },
                    "compare_set": {
                        "opcode": "data_setvariableto",
                        "parent": "math_set",
                        "inputs": {"VALUE": [1, "compare"]},
                        "fields": {"VARIABLE": ["same", "same_id"]}
                    },
                    "compare": {
                        "opcode": "operator_equals",
                        "parent": "compare_set",
                        "inputs": {
                            "OPERAND1": [1, [10, "5"]],
                            "OPERAND2": [1, [4, "5"]]
                        },
                        "fields": {}
                    }
                }
            }]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert!(output.contains("scratch_to_number(state.get(0)).floor()"));
    assert!(output.contains("scratch_equals(\"5\", 5.0_f64)"));
}

#[test]
fn numeric_loops_use_native_locals_and_commit_before_safeguard_yields() {
    // repeat-until avoids the separate simple-repeat fusion path in this test.
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1",
                    "isStage": false,
                    "variables": {"count_id": ["count", 0]},
                    "blocks": {
                        "hat": {"opcode": "event_whenflagclicked", "next": "repeat", "topLevel": true, "inputs": {}, "fields": {}},
                        "repeat": {"opcode": "control_repeat_until", "next": null, "inputs": {"CONDITION": [2, "equal"], "SUBSTACK": [2, "change"]}, "fields": {}},
                        "change": {"opcode": "data_changevariableby", "next": null, "inputs": {"VALUE": [1, [4, "1"]]}, "fields": {"VARIABLE": ["count", "count_id"]}},
                        "equal": {"opcode": "operator_equals", "next": null, "inputs": {"OPERAND1": [3, [12, "count", "count_id"], [4, "10"]], "OPERAND2": [1, [4, "10"]]}, "fields": {}}
                    }
                }]
            })
            .to_string(),
        )
        .unwrap();

    let output = generate(&project);
    assert!(output.contains("let mut scratch_local_0 = state.get_number(0);"));
    assert!(output.contains("scratch_local_0 += 1.0_f64;"));
    assert!(!output.contains("scratch_equals(\"same\", \"same\")"));
    let safeguard = output
        .find("if !scratch_warp_mode || script.warp_budget.expired(clock)")
        .unwrap();
    let commit = output[safeguard..]
        .find("state.set_local_number(0, scratch_local_0);")
        .unwrap()
        + safeguard;
    let yielded = output[commit..]
        .find("return ScriptStatus::Yield;")
        .unwrap()
        + commit;
    assert!(safeguard < commit && commit < yielded);
}

#[test]
fn monitored_loop_variables_do_not_emit_an_unreachable_deferred_write_path() {
    let project = parse_project(
        &json!({
            "targets": [{
                "name": "Stage",
                "isStage": true,
                "variables": {"count_id": ["count", 0]},
                "blocks": {
                    "hat": {"opcode":"event_whenflagclicked", "next":"repeat", "topLevel":true, "inputs":{}, "fields":{}},
                    "repeat": {"opcode":"control_repeat", "next":null, "parent":"hat", "inputs":{"TIMES":[1,[4,"100"]], "SUBSTACK":[2,"change"]}, "fields":{}},
                    "change": {"opcode":"data_changevariableby", "next":null, "parent":"repeat", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["count","count_id"]}}
                }
            }],
            "monitors": [{"id":"count_id", "mode":"default", "opcode":"data_variable", "params":{"VARIABLE":"count"}, "visible":true}]
        })
        .to_string(),
    )
    .unwrap();

    let output = generate(&project);
    assert!(output.contains("with_monitor_visible(true)"));
    assert!(!output.contains("state.can_defer_variable_writes"));
    assert!(output.contains("state.change_shared_number(0, scratch_to_number(1.0_f64));"));
}

#[test]
fn fast_math_only_changes_proven_numeric_loop_expressions() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/hard-math-10-seconds.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let compatible = generate(&project);
    let fast = super::generate_with_options(&project, true);

    assert!(compatible.contains("scratch_number_operand(scratch_local_0)"));
    assert!(compatible.contains("scratch_sin_number("));
    assert!(compatible.contains("repeat_counts[0] & (1usize << 0) != 0"));
    assert!(compatible.contains("repeat_counts[0] |= 1usize << 0"));
    assert!(compatible.contains(
        "let scratch_batch = script.warp_budget.safe_points_until_check();"
    ));
    assert!(compatible.contains(
        "script.warp_budget.expired_after_safe_points(clock, scratch_batch)"
    ));
    assert!(compatible.contains("let scratch_batch = 1024usize;"));
    assert!(!compatible.contains("fast_sin("));
    assert!(fast.contains("fast_sin("));
    assert!(fast.contains("fast_cos("));
    assert!(fast.contains("fast_asin_degrees("));
    assert!(!fast.contains("scratch_number_operand(scratch_local_0)"));

    let dynamic_input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/operators.sb3");
    let json = crate::extractor::read_project_json(dynamic_input).unwrap();
    let dynamic_project = parse_project(&json).unwrap();
    let dynamic_fast = super::generate_with_options(&dynamic_project, true);
    assert!(dynamic_fast.contains("scratch_add(state.get("));
    assert!(dynamic_fast.contains("scratch_to_number(state.get_shared("));
}

#[test]
fn numeric_loops_specialize_non_nan_comparisons_and_literal_increments() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1",
                    "isStage": false,
                    "variables": {
                        "count_id": ["count", 0],
                        "running_id": ["running", 1]
                    },
                    "blocks": {
                        "hat": {"opcode": "event_whenflagclicked", "next": "loop", "topLevel": true, "inputs": {}, "fields": {}},
                        "loop": {"opcode": "control_repeat_until", "next": null, "inputs": {"CONDITION": [2, "equal"], "SUBSTACK": [2, "change"]}, "fields": {}},
                        "equal": {"opcode": "operator_equals", "next": null, "inputs": {"OPERAND1": [3, [12, "running", "running_id"], [4, "0"]], "OPERAND2": [1, [4, "0"]]}, "fields": {}},
                        "change": {"opcode": "data_changevariableby", "next": null, "inputs": {"VALUE": [1, [4, "1"]]}, "fields": {"VARIABLE": ["count", "count_id"]}}
                    }
                }]
            })
            .to_string(),
        )
        .unwrap();

    let output = generate(&project);
    assert!(output.contains("if scratch_local_1 == 0.0_f64 { break; }"));
    assert!(output.contains("scratch_local_0 += 1.0_f64;"));
    assert!(!output.contains("scratch_equals(scratch_local_1, 0.0_f64)"));
    assert!(!output.contains("scratch_add(scratch_local_0, 1.0_f64)"));
}

#[test]
fn numeric_loops_keep_dynamic_scratch_semantics() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1",
                    "isStage": false,
                    "variables": {
                        "count_id": ["count", 0],
                        "change_id": ["change", 1]
                    },
                    "blocks": {
                        "hat": {"opcode": "event_whenflagclicked", "next": "set", "topLevel": true, "inputs": {}, "fields": {}},
                        "set": {"opcode": "data_setvariableto", "next": "loop", "inputs": {"VALUE": [3, "divide", [4, "0"]]}, "fields": {"VARIABLE": ["count", "count_id"]}},
                        "divide": {"opcode": "operator_divide", "next": null, "inputs": {"NUM1": [1, [4, "0"]], "NUM2": [1, [4, "0"]]}, "fields": {}},
                        "loop": {"opcode": "control_repeat_until", "next": null, "inputs": {"CONDITION": [2, "equal"], "SUBSTACK": [2, "change"]}, "fields": {}},
                        "equal": {"opcode": "operator_equals", "next": null, "inputs": {"OPERAND1": [3, [12, "count", "count_id"], [4, "0"]], "OPERAND2": [1, [4, "0"]]}, "fields": {}},
                        "change": {"opcode": "data_changevariableby", "next": null, "inputs": {"VALUE": [3, [12, "change", "change_id"], [4, "0"]]}, "fields": {"VARIABLE": ["count", "count_id"]}}
                    }
                }]
            })
            .to_string(),
        )
        .unwrap();

    let output = generate(&project);
    assert!(output.contains("scratch_equals(scratch_local_1, 0.0_f64)"));
    assert!(output.contains("scratch_add(scratch_local_1, scratch_local_0)"));
}

#[test]
fn loop_lowering_borrows_variables_and_converts_only_dynamic_conditions() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1",
                    "isStage": false,
                    "variables": {
                        "count_id": ["count", 3],
                        "text_id": ["text", "3"],
                        "flag_id": ["flag", false]
                    },
                    "blocks": {
                        "hat": {
                            "opcode": "event_whenflagclicked",
                            "next": "repeat",
                            "topLevel": true,
                            "inputs": {},
                            "fields": {}
                        },
                        "repeat": {
                            "opcode": "control_repeat",
                            "next": "while",
                            "parent": "hat",
                            "inputs": {"TIMES": [3, "text_reporter", [10, "3"]], "SUBSTACK": [2, "set_text"]},
                            "fields": {}
                        },
                        "while": {
                            "opcode": "control_while",
                            "next": null,
                            "parent": "repeat",
                            "inputs": {"CONDITION": [2, "compare"], "SUBSTACK": [2, "set_flag"]},
                            "fields": {}
                        },
                        "text_reporter": {
                            "opcode": "data_variable",
                            "parent": "repeat",
                            "inputs": {},
                            "fields": {"VARIABLE": ["text", "text_id"]}
                        },
                        "set_text": {
                            "opcode": "data_setvariableto",
                            "next": "set_flag",
                            "parent": "repeat",
                            "inputs": {"VALUE": [1, [10, "5"]]},
                            "fields": {"VARIABLE": ["count", "count_id"]}
                        },
                        "set_flag": {
                            "opcode": "data_setvariableto",
                            "next": null,
                            "parent": "set_text",
                            "inputs": {"VALUE": [3, "count_reporter", [10, ""]]},
                            "fields": {"VARIABLE": ["flag", "flag_id"]}
                        },
                        "compare": {
                            "opcode": "operator_equals",
                            "parent": "set_flag",
                            "inputs": {"OPERAND1": [3, "count_reporter", [10, ""]], "OPERAND2": [1, [4, "5"]]},
                            "fields": {}
                        },
                        "count_reporter": {
                            "opcode": "data_variable",
                            "parent": "compare",
                            "inputs": {},
                            "fields": {"VARIABLE": ["count", "count_id"]}
                        }
                    }
                }]
            })
            .to_string(),
        )
        .unwrap();

    let output = generate(&project);
    assert!(output.contains("scratch_round(state.get(2))"));
    assert!(output.contains("let left = state.get(0); let right = 5.0_f64;"));
    assert!(output.contains(
        "ScratchValue::Number(number) if !number.is_nan() && !right.is_nan() => *number == right"
    ));
    assert!(output.contains("_ => scratch_equals(left, right)"));
    assert!(output.contains("let scratch_new_value: ScratchValue = (state.get(0)).clone();"));
    assert!(!output.contains("scratch_bool(scratch_equals"));
    assert!(!output.contains("scratch_equals((state.get(0)).clone()"));
    assert!(!output.contains("scratch_round((state.get(2)).clone()"));
    assert_eq!(output.matches("scratch_round(state.get(2))").count(), 1);
    assert!(output.contains("return ScriptStatus::Yield;"));
}

#[test]
fn sensing_of_fixture_resolves_static_targets_including_sprites_without_scripts() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/sensing-of.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    for generated in [
        "state.sensing_of_target_slot(0, \"backdrop #\", None)",
        "state.sensing_of_target_slot(0, \"backdrop name\", None)",
        "state.sensing_of_target_slot(0, \"volume\", None)",
        "state.sensing_of_target_slot(0, \"another variable\", Some(",
        "state.sensing_of_target_slot(2, \"x position\", None)",
        "state.sensing_of_target_slot(2, \"y position\", None)",
        "state.sensing_of_target_slot(2, \"direction\", None)",
        "state.register_target(2, \"Sprite2\", false);",
    ] {
        assert!(
            output.contains(generated),
            "missing generated path: {generated}"
        );
    }
    assert_eq!(output.matches("state.sensing_of_target_slot(").count(), 7);
    assert!(!output.contains("state.sensing_of_by_name("));
    assert!(!output.contains("unsupported reporter: sensing_of"));
}

#[test]
fn sensing_of_mutations_fixture_reads_properties_after_script_updates() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/sensing-of-mutations.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert_eq!(output.matches("state.sensing_of_target_slot(").count(), 11);
    assert!(output.contains("state.sensing_of_target_slot(2, \"x position\", None)"));
    assert!(output.contains("state.sensing_of_target_slot(2, \"size\", None)"));
    assert!(output.contains("state.sensing_of_target_slot(0, \"backdrop #\", None)"));
    assert!(output.contains(
        "let scratch_size_input = scratch_to_number(80.0_f64); state.set_size(scratch_size_input);"
    ));
    assert!(output.contains("let scratch_size_input = scratch_to_number(-5.0_f64); state.change_size_by(scratch_size_input);"));
    assert!(!output.contains("state.sensing_of_by_name("));
}

#[test]
fn sensing_of_accepts_a_dynamic_object_reporter_and_handles_dangling_menu_ids() {
    let project = parse_project(
        &json!({
            "targets": [
                {
                    "name": "Stage",
                    "isStage": true,
                    "variables": {"target_id": ["target", "Sprite2"]},
                    "blocks": {
                        "hat": {"opcode": "event_whenflagclicked", "next": "say", "topLevel": true},
                        "say": {"opcode": "looks_say", "inputs": {"MESSAGE": [3, "sensor", [10, "_stage_"]]}},
                        "sensor": {"opcode": "sensing_of", "inputs": {"OBJECT": [3, "target_reporter", "menu"]}, "fields": {"PROPERTY": ["x position", null]}},
                        "target_reporter": {"opcode": "data_variable", "fields": {"VARIABLE": ["target", "target_id"]}},
                        "menu": {"opcode": "sensing_of_object_menu", "shadow": true, "fields": {"OBJECT": ["_stage_", null]}}
                    }
                },
                {"name": "Sprite2", "isStage": false}
            ]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&project);
    assert!(output.contains("state.sensing_of_by_name(state.get_shared(0), \"x position\")"));
    assert!(output.contains("state.register_target(1, \"Sprite2\", false);"));
    assert!(!output.contains("unsupported reporter: sensing_of"));

    let dangling = parse_project(
        &json!({
            "targets": [{
                "name": "Stage", "isStage": true,
                "blocks": {
                    "hat": {"opcode": "event_whenflagclicked", "next": "say", "topLevel": true},
                    "say": {"opcode": "looks_say", "inputs": {"MESSAGE": [2, "sensor"]}},
                    "sensor": {"opcode": "sensing_of", "inputs": {"OBJECT": [1, "missing-menu"]}, "fields": {"PROPERTY": ["x position", null]}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&dangling);
    assert!(output.contains("ScratchValue::Number(0.0_f64)"));

    let malformed = parse_project(
        &json!({
            "targets": [{
                "name": "Stage", "isStage": true,
                "blocks": {
                    "hat": {"opcode": "event_whenflagclicked", "next": "say", "topLevel": true},
                    "say": {"opcode": "looks_say", "inputs": {"MESSAGE": [2, "sensor"]}},
                    "sensor": {"opcode": "sensing_of", "inputs": {"OBJECT": [1, "menu"]}, "fields": {"PROPERTY": ["x position", null]}},
                    "menu": {"opcode": "sensing_of_object_menu", "shadow": true, "fields": {}}
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&malformed);
    assert!(!output.contains("state.sensing_of_by_name("));
    assert!(output.contains("ScratchValue::Number(0.0_f64)"));
}

#[test]
fn sensing_of_flows_through_assignments_conditions_loops_and_custom_arguments() {
    let mut blocks = serde_json::Map::new();
    blocks.insert(
        "hat".to_owned(),
        json!({"opcode":"event_whenflagclicked", "next":"set", "topLevel":true}),
    );
    blocks.insert(
        "set".to_owned(),
        json!({"opcode":"data_setvariableto", "next":"change", "inputs":{"VALUE":[3,"sum",[10,""]]}, "fields":{"VARIABLE":["value","value_id"]}}),
    );
    blocks.insert(
        "sum".to_owned(),
        json!({"opcode":"operator_add", "inputs":{"NUM1":[3,"sense1",[10,"0"]], "NUM2":[3,"sense2",[10,"0"]]}}),
    );
    blocks.insert(
        "change".to_owned(),
        json!({"opcode":"data_changevariableby", "next":"if", "inputs":{"VALUE":[3,"sense3",[10,"0"]]}, "fields":{"VARIABLE":["value","value_id"]}}),
    );
    blocks.insert(
        "if".to_owned(),
        json!({"opcode":"control_if", "next":"repeat", "inputs":{"CONDITION":[3,"sense4",[10,"0"]], "SUBSTACK":[2,"if_body"]}}),
    );
    blocks.insert(
        "if_body".to_owned(),
        json!({"opcode":"data_setvariableto", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["value","value_id"]}}),
    );
    blocks.insert(
        "repeat".to_owned(),
        json!({"opcode":"control_repeat_until", "next":"call", "inputs":{"CONDITION":[3,"sense5",[10,"0"]], "SUBSTACK":[2,"repeat_body"]}}),
    );
    blocks.insert(
        "repeat_body".to_owned(),
        json!({"opcode":"data_changevariableby", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["value","value_id"]}}),
    );
    blocks.insert(
        "call".to_owned(),
        json!({"opcode":"procedures_call", "inputs":{"argument_id":[3,"sense6",[10,"0"]]}, "mutation":{"proccode":"take %s", "argumentids":"[\"argument_id\"]", "warp":"false"}}),
    );
    blocks.insert(
        "definition".to_owned(),
        json!({"opcode":"procedures_definition", "next":"procedure_set", "topLevel":true, "inputs":{"custom_block":[1,"prototype"]}}),
    );
    blocks.insert(
        "prototype".to_owned(),
        json!({"opcode":"procedures_prototype", "parent":"definition", "inputs":{"argument_id":[1,"argument_default"]}, "mutation":{"proccode":"take %s", "argumentnames":"[\"value\"]", "argumentids":"[\"argument_id\"]", "argumentdefaults":"[\"\"]", "warp":"false"}}),
    );
    blocks.insert(
        "argument_default".to_owned(),
        json!({"opcode":"argument_reporter_string_number", "parent":"prototype", "shadow":true, "fields":{"VALUE":["value",null]}}),
    );
    blocks.insert(
        "procedure_set".to_owned(),
        json!({"opcode":"data_setvariableto", "parent":"definition", "inputs":{"VALUE":[3,"argument",[10,""]]}, "fields":{"VARIABLE":["value","value_id"]}}),
    );
    blocks.insert(
        "argument".to_owned(),
        json!({"opcode":"argument_reporter_string_number", "parent":"procedure_set", "fields":{"VALUE":["value",null]}}),
    );

    for (index, id) in (1..=6).map(|index| (index, format!("sense{index}"))) {
        let menu = format!("menu{index}");
        blocks.insert(
            id,
            json!({"opcode":"sensing_of", "inputs":{"OBJECT":[1,menu]}, "fields":{"PROPERTY":["size",null]}}),
        );
        blocks.insert(
            menu,
            json!({"opcode":"sensing_of_object_menu", "shadow":true, "fields":{"OBJECT":["Sprite2",null]}}),
        );
    }

    let project = parse_project(
        &json!({
            "targets": [
                {"name":"Stage", "isStage":true},
                {"name":"Sprite1", "isStage":false, "variables":{"value_id":["value",0]}, "blocks":blocks},
                {"name":"Sprite2", "isStage":false}
            ]
        })
        .to_string(),
    )
    .unwrap();
    let output = generate(&project);

    assert!(
        output
            .matches("state.sensing_of_target_slot(2, \"size\", None)")
            .count()
            >= 6
    );
    assert!(!output.contains("state.sensing_of_by_name("));
    assert!(!output.contains("unsupported reporter: sensing_of"));
}
