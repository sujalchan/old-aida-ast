use super::*;
use serde_json::Value;

#[test]
fn fixture_without_event_scripts_generates_one_broadcast_dispatcher() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/svg-bitmap.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert_eq!(output.matches("fn new_broadcast_scripts(").count(), 1);
}

#[test]
fn wireframe_scripts_only_emit_reachable_procedure_bodies() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/wireframe-engine.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let (output, _) = generate_with_list_data(&project, false, "wireframe.rs.lists.json");
    assert!(output.contains("fn direct_procedure_1_"));
    assert!(output.contains("_numbers("));
    assert!(output.contains("cached_numeric_item_"));
    assert!(output.contains("script.warp_budget.expired(clock)"));
    assert!(output.contains("fn step_green_flag_script_engine_from_nowhere_"));
}

#[test]
fn numeric_direct_variants_are_omitted_when_no_call_site_uses_them() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/programs/double-custom-block.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(output.contains("fn direct_procedure_1_"));
    assert!(!output.contains("_numbers("));
}

#[test]
fn direct_numeric_definite_writes_drop_dirty_flags() {
    let project = long_direct_procedure_project(false, false);
    let output = generate(&project);
    let body = generated_numeric_direct_body(&output, "state.can_defer_variable_writes");

    assert!(body.contains("state.can_defer_variable_writes"));
    assert!(
        !body.contains("scratch_dirty_"),
        "unconditional writes should commit without dirty state"
    );
    assert!(body.contains("if scratch_defer_"));
    assert!(body.contains("state.set_local_number("));
}

#[test]
fn direct_numeric_conditional_writes_keep_only_required_dirty_state() {
    let project = long_direct_procedure_project(true, false);
    let output = generate(&project);
    let body = generated_numeric_direct_body(&output, "state.can_defer_variable_writes");

    assert!(body.contains("let mut scratch_dirty_0 = false;"));
    assert!(body.contains("scratch_dirty_0 = true;"));
    assert!(body.contains("scratch_defer_0 && scratch_dirty_0"));
    assert!(!body.contains("scratch_dirty_1"));
}

#[test]
fn direct_numeric_statically_nondeferrable_writes_emit_no_constant_branch() {
    let project = long_direct_procedure_project(false, true);
    let output = generate(&project);
    let body = generated_numeric_direct_body(&output, "let numeric_new = 3.0_f64;");

    assert!(!body.contains("scratch_defer_"));
    assert!(!body.contains("scratch_dirty_"));
    assert!(body.contains("state.set_local_number("), "{body}");
    assert!(body.contains("state.set_local_value("));
}

fn generated_numeric_direct_body<'a>(output: &'a str, marker_text: &str) -> &'a str {
    let marker = output
        .find(marker_text)
        .expect("the generated direct numeric procedure should defer writes");
    let start = output[..marker]
        .rfind("fn direct_procedure_")
        .expect("expected the numeric direct procedure declaration");
    let end = output[start..]
        .find("\n}\n")
        .map(|offset| start + offset + 2)
        .expect("expected the numeric direct procedure to close");
    &output[start..end]
}

fn long_direct_procedure_project(
    conditional: bool,
    mixed_representation: bool,
) -> crate::parser::ScratchProject {
    let mut blocks = json!({
        "hat": {"opcode":"event_whenflagclicked", "next":"call", "topLevel":true, "inputs":{}, "fields":{}},
        "call": {"opcode":"procedures_call", "parent":"hat", "inputs":{}, "fields":{}, "mutation":{"proccode":"long", "argumentids":"[]", "warp":"false"}},
        "definition": {"opcode":"procedures_definition", "next":if conditional {"condition"} else {"set0"}, "topLevel":true, "inputs":{"custom_block":[1,"prototype"]}, "fields":{}},
        "prototype": {"opcode":"procedures_prototype", "parent":"definition", "inputs":{}, "fields":{}, "mutation":{"proccode":"long", "argumentnames":"[]", "argumentids":"[]", "argumentdefaults":"[]", "warp":"false"}},
        "flag_reporter": {"opcode":"data_variable", "parent":"condition", "inputs":{}, "fields":{"VARIABLE":["flag","flag_id"]}},
        "condition": {"opcode":"control_if", "next":"set1", "parent":"definition", "inputs":{"CONDITION":[2,"flag_reporter"], "SUBSTACK":[2,"conditional_set"]}, "fields":{}},
        "conditional_set": {"opcode":"data_setvariableto", "parent":"condition", "inputs":{"VALUE":[1,[4,"1"]]}, "fields":{"VARIABLE":["conditional","conditional_id"]}}
    });

    let fixed_count = if conditional { 11 } else { 12 };
    for index in 0..fixed_count {
        let id = if conditional {
            format!("set{}", index + 1)
        } else {
            format!("set{index}")
        };
        let next = (index + 1 < fixed_count).then(|| {
            if conditional {
                format!("set{}", index + 2)
            } else {
                format!("set{}", index + 1)
            }
        });
        let value = if mixed_representation && index == 5 {
            json!([1, [10, "2"]])
        } else {
            json!([1, [4, "3"]])
        };
        blocks.as_object_mut().unwrap().insert(
            id.clone(),
            json!({
                "opcode":"data_setvariableto",
                "next":next,
                "parent":if conditional && index == 0 {"condition"} else if index == 0 {"definition"} else {"previous"},
                "inputs":{"VALUE":value},
                "fields":{"VARIABLE":if conditional { ["other", "other_id"] } else { ["value", "value_id"] }}
            }),
        );
        if index > 0 || !conditional {
            let previous = if conditional {
                if index == 0 {
                    "condition".to_owned()
                } else {
                    format!("set{index}")
                }
            } else if index == 0 {
                "definition".to_owned()
            } else {
                format!("set{}", index - 1)
            };
            if let Some(block) = blocks.as_object_mut().unwrap().get_mut(&id) {
                block["parent"] = json!(previous);
            }
        }
    }
    // The first conditional assignment is separate from the straight-line
    // writes so its dirty status remains conditional at the final flush.
    if conditional {
        blocks["conditional_set"]["next"] = Value::Null;
    }

    parse_project(
        &json!({
            "targets": [{
                "name":"Sprite1",
                "isStage":false,
                "variables": {
                    "conditional_id":["conditional", 0],
                    "other_id":["other", 0],
                    "value_id":["value", 0],
                    "flag_id":["flag", 1]
                },
                "blocks":blocks
            }]
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn engine_resumable_procedures_are_split_into_bounded_native_workers() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/programs/engine-from-nowhere.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    let lines = output.lines().collect::<Vec<_>>();
    let mut worker_sizes = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.starts_with("fn step_") || !line.contains("_partition_") {
            continue;
        }
        let end = (index + 1..lines.len())
            .find(|next| lines[*next].starts_with("fn "))
            .unwrap_or(lines.len());
        worker_sizes.push(
            lines[index..end]
                .iter()
                .map(|line| line.len() + 1)
                .sum::<usize>(),
        );
    }

    assert!(
        worker_sizes.len() > 34,
        "expected smaller partitions to increase the worker count"
    );
    assert!(
        worker_sizes.iter().all(|size| *size <= 40 * 1024),
        "worker source exceeded the single-arm allowance: {} bytes",
        worker_sizes.iter().copied().max().unwrap_or_default()
    );
    assert!(output.contains("#[inline(never)]\n#[allow(dead_code"));
}

#[test]
fn small_resumable_fixture_keeps_its_single_step_function() {
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/src/loops.sb3");
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(!output.contains("_partition_"));
}

#[test]
fn generated_program_exposes_tick_redraw_and_input_state_to_compiled_code() {
    let project = parse_project(
            &json!({
                "targets": [{
                    "name": "Sprite1", "isStage": false, "variables": {},
                    "blocks": {
                        "hat": {"opcode":"event_whenflagclicked", "next":"say_key", "topLevel":true, "inputs":{}, "fields":{}},
                        "say_key": {"opcode":"looks_say", "next":"say_x", "parent":"hat", "inputs":{"MESSAGE":[3,"key",[10,""]]}, "fields":{}},
                        "key": {"opcode":"sensing_keypressed", "next":null, "parent":"say_key", "inputs":{}, "fields":{"KEY_OPTION":["space",null]}},
                        "say_x": {"opcode":"looks_say", "next":null, "parent":"say_key", "inputs":{"MESSAGE":[3,"mouse_x",[10,""]]}, "fields":{}},
                        "mouse_x": {"opcode":"sensing_mousex", "next":null, "parent":"say_x", "inputs":{}, "fields":{}}
                    }
                }]
            }).to_string(),
        ).unwrap();
    let output = generate(&project);
    assert!(output.contains("pub fn begin_tick(&mut self)"));
    assert!(output.contains("pub fn redraw_requested(&self)"));
    assert!(output.contains("pub fn presentation_consumed(&mut self)"));
    assert!(output.contains("pub fn apply_input(&mut self, event: RuntimeInputEvent)"));
    assert!(output.contains("state.key_pressed(\"space\")"));
    assert!(output.contains("state.mouse_x()"));
}

#[test]
fn generated_helpers_and_imports_follow_project_usage() {
    let project_json = json!({
        "targets": [{
            "name": "Stage",
            "isStage": true,
            "variables": {},
            "lists": {},
            "blocks": {}
        }]
    });
    let project = parse_project(&project_json.to_string()).unwrap();
    let output = generate(&project);

    assert!(output.contains("use crate::runtime::{"));
    assert!(!output.contains("#[allow(unused_imports)]"));
    assert!(!output.contains("print_value"));
    assert!(!output.contains("WarpBudget"));
    assert!(!output.contains("warp_budget_expired"));
    assert!(!output.contains("yield_at_safe_point"));
    assert!(!output.contains("fn new_clone_scripts(target: usize, _instance: usize)"));
    assert!(!output.contains("take_pending_clones"));
    assert!(!output.contains("remove_target_scripts"));
    assert!(!output.contains("state.set_current_instance(*instance)"));
    assert!(output.contains("#[allow(dead_code)]\npub fn run()"));
}

#[test]
fn generated_warp_guards_only_include_used_helpers() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/src/clone-with-variables.sb3"
    );
    let json = crate::extractor::read_project_json(input).unwrap();
    let project = parse_project(&json).unwrap();
    let output = generate(&project);

    assert!(output.contains("fn warp_budget_expired(&mut self"));
    assert!(!output.contains("fn yield_at_safe_point(&mut self"));
    assert!(output.contains("WarpBudget"));
}
