use super::*;

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
    assert!(output.contains("#[inline]\n#[allow(dead_code"));
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
