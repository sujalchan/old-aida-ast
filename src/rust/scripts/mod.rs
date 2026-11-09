// Copyright (c) 2026 AIDA AST contributers (see AUTHORS.md)
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! emits resumable custom block and green flag script frames.

use std::collections::HashMap;
use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde_json::Value;

use crate::parser::{ScratchProject, ScratchTarget};

use super::{
    expressions::expression,
    literals::{line, quote},
    project::{ListInfo, VariableInfo, rust_identifier},
    statements::{
        DirectProcedureOptions, ResumableScriptOptions, generate_direct_procedures,
        generate_resumable_procedure_step, generate_resumable_script, prepare_resumable_procedures,
    },
};

mod procedures;

pub(super) use procedures::ProcedureSignature;
use procedures::procedure_signatures;

struct ResumableStepOutput {
    start_pc: usize,
    loop_count: usize,
    source: String,
}

// emit a script enum and one step function for each supported event hat.
pub(super) fn generate_green_flag_functions(
    project: &ScratchProject,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    fast_math: bool,
    no_warp_timer: bool,
    output: &mut String,
) {
    let scripts = event_scripts(project, variables, lists);
    if scripts.is_empty() {
        output.push_str("enum GeneratedScript {}\n\nimpl ResumableScript for GeneratedScript {\n    fn step(&mut self, _state: &mut ScratchState, _clock: &dyn Clock) -> ScriptStatus { match *self {} }\n}\n\n");
        return;
    }
    let uses_glide = project.targets.iter().any(|target| {
        target.blocks.values().any(|block| {
            matches!(
                block.opcode.as_str(),
                "motion_glideto" | "motion_glidesecstoxy"
            )
        })
    });
    let message_keys = broadcast_message_keys(&scripts);
    let message_key_arms = message_keys
        .iter()
        .map(|(message, key)| {
            format!(
                "{} => {key}",
                super::literals::quote(&message.to_uppercase())
            )
        })
        .collect::<Vec<_>>();
    let message_key_match = if message_key_arms.is_empty() {
        "_ => 0".to_owned()
    } else {
        format!("{}, _ => 0", message_key_arms.join(", "))
    };
    line(
        output,
        0,
        &format!(
            "#[allow(dead_code)]\nfn broadcast_message_key(message: String) -> usize {{ match message.to_uppercase().as_str() {{ {message_key_match} }} }}\n"
        ),
    );

    let glide_field = if uses_glide {
        ", glide: Option<GlideState>"
    } else {
        ""
    };
    let glide_initializer = if uses_glide { ", glide: None" } else { "" };
    output.push_str(&format!(
        "#[allow(dead_code)]\nstruct ProcedureFrame {{ function: usize, pc: usize, repeat_counts: Vec<usize>, numeric_loop_values: Vec<f64>, arguments: Vec<ScratchValue>, warp_mode: bool{glide_field} }}\nimpl ProcedureFrame {{ fn new(function: usize, pc: usize, repeat_counts: Vec<usize>, arguments: Vec<ScratchValue>, warp_mode: bool) -> Self {{ let numeric_loop_values = vec![0.0; repeat_counts.len()]; Self {{ function, pc, repeat_counts, numeric_loop_values, arguments, warp_mode{glide_initializer} }} }} }}\n"
    ));
    output.push_str(
        "#[allow(dead_code)]\n#[derive(Default)]\nstruct ProcedureFrameStorage { repeat_counts: Vec<usize>, numeric_loop_values: Vec<f64>, arguments: Vec<ScratchValue> }\n\n",
    );
    output.push_str("struct ScriptState { frames: Vec<ProcedureFrame>, #[allow(dead_code)] frame_pool: Vec<ProcedureFrameStorage>, __WARP_BUDGET_FIELD__ }\n\n");
    line(output, 0, "enum GeneratedScript {");
    for script in &scripts {
        line(
            output,
            1,
            &format!("{}(usize, ScriptState),", script.variant),
        );
    }
    line(output, 0, "}\n");

    output.push_str("impl ResumableScript for GeneratedScript {\n    fn step(&mut self, state: &mut ScratchState, clock: &dyn Clock) -> ScriptStatus {\n        match self {\n");
    for script in &scripts {
        line(
            output,
            3,
            &format!(
                "Self::{}(_, script) => step_{}(script, state, clock),",
                script.variant, script.function
            ),
        );
    }
    output.push_str("        }\n    }\n    fn target_instance(&self) -> usize { match self {\n");
    for script in &scripts {
        line(
            output,
            3,
            &format!("Self::{}(instance, _) => *instance,", script.variant),
        );
    }
    output.push_str("    } }\n    fn broadcast_receiver(&self) -> Option<(usize, usize, usize)> { match self {\n");
    for (index, script) in scripts.iter().enumerate() {
        if let Some(message) = &script.broadcast_message {
            let key = message_keys[message];
            line(
                output,
                3,
                &format!(
                    "Self::{}(instance, _) => Some(({key}, {index}, *instance)),",
                    script.variant
                ),
            );
        }
    }
    output.push_str("        _ => None,\n    } }\n    fn edge_hat_key(&self) -> Option<(usize, usize)> { match self {\n");
    for (index, script) in scripts.iter().enumerate() {
        if script.hat.opcode == "event_whengreaterthan" {
            line(
                output,
                3,
                &format!(
                    "Self::{}(instance, _) => Some(({index}, *instance)),",
                    script.variant
                ),
            );
        }
    }
    output.push_str("        _ => None,\n    } }\n    fn click_hat_key(&self) -> Option<(usize, usize)> { match self {\n");
    for (index, script) in scripts.iter().enumerate() {
        if matches!(
            script.hat.opcode.as_str(),
            "event_whenthisspriteclicked" | "event_whenstageclicked"
        ) {
            line(
                output,
                3,
                &format!(
                    "Self::{}(instance, _) => Some(({index}, *instance)),",
                    script.variant
                ),
            );
        }
    }
    // key hat identity includes both its compiled block index and target instance.
    output.push_str("        _ => None,\n    } }\n    fn key_hat_key(&self) -> Option<(usize, usize)> { match self {\n");
    for (index, script) in scripts.iter().enumerate() {
        if script.hat.opcode == "event_whenkeypressed" {
            line(
                output,
                3,
                &format!(
                    "Self::{}(instance, _) => Some(({index}, *instance)),",
                    script.variant
                ),
            );
        }
    }
    // omit a wildcard when every generated variant is a key hat to keep rust's match exhaustive.
    if scripts
        .iter()
        .any(|script| script.hat.opcode != "event_whenkeypressed")
    {
        output.push_str("        _ => None,\n");
    }
    output.push_str("    } }\n    fn restart_for_broadcast(&mut self) { match self {\n");
    for script in &scripts {
        if script.broadcast_message.is_some() {
            line(
                output,
                3,
                &format!(
                    "Self::{}(instance, _) => {{ let instance = *instance; *self = new_{}_for(instance); }},",
                    script.variant, script.function
                ),
            );
        }
    }
    output.push_str("        _ => {},\n    } }\n}\n\n");

    generate_edge_hat_poller(&scripts, output);

    let mut procedure_plans = HashMap::new();
    let mut procedures_by_target = HashMap::new();
    for script in &scripts {
        if procedure_plans.contains_key(&script.target_index) {
            continue;
        }
        let procedures = procedure_signatures(script.target);
        let script_starts = scripts
            .iter()
            .filter(|candidate| candidate.target_index == script.target_index)
            .map(|candidate| candidate.hat.next.as_deref())
            .collect::<Vec<_>>();
        let plan = prepare_resumable_procedures(
            script.target,
            &script_starts,
            variables,
            lists,
            &procedures,
            script.target_index,
        );
        procedures_by_target.insert(script.target_index, procedures);
        procedure_plans.insert(script.target_index, plan);
    }

    // Shared procedure dispatch saves substantial source only when procedure
    // bodies overlap across scripts. Compare actual generated bytes for both
    // layouts and keep the smaller one for each target.
    let mut resumable_outputs = HashMap::<String, ResumableStepOutput>::new();
    let mut shared_procedure_steps = HashMap::<usize, String>::new();
    let mut planned_targets = HashSet::new();
    for script in &scripts {
        if !planned_targets.insert(script.target_index) {
            continue;
        }
        let target_scripts = scripts
            .iter()
            .filter(|candidate| candidate.target_index == script.target_index)
            .collect::<Vec<_>>();
        let procedures = &procedures_by_target[&script.target_index];
        let shared_plan = &procedure_plans[&script.target_index];
        let mut shared_helper = String::new();
        generate_resumable_procedure_step(
            script.target,
            variables,
            lists,
            procedures,
            shared_plan,
            fast_math,
            no_warp_timer,
            true,
            &mut shared_helper,
        );
        let mut shared_outputs = HashMap::new();
        let mut shared_bytes = shared_helper.len();
        for candidate in &target_scripts {
            let mut source = String::new();
            let (start_pc, loop_count) = generate_resumable_script(
                candidate.target,
                candidate.hat.next.as_deref(),
                variables,
                lists,
                procedures,
                shared_plan,
                true,
                ResumableScriptOptions {
                    function_name: &candidate.function,
                    fast_math,
                    no_warp_timer,
                },
                &mut source,
            );
            shared_bytes += source.len();
            shared_outputs.insert(
                candidate.function.clone(),
                ResumableStepOutput {
                    start_pc,
                    loop_count,
                    source,
                },
            );
        }

        let mut legacy_outputs = HashMap::new();
        let mut legacy_bytes = 0;
        for candidate in &target_scripts {
            let legacy_plan = prepare_resumable_procedures(
                candidate.target,
                &[candidate.hat.next.as_deref()],
                variables,
                lists,
                procedures,
                candidate.target_index,
            );
            let mut source = String::new();
            let (start_pc, loop_count) = generate_resumable_script(
                candidate.target,
                candidate.hat.next.as_deref(),
                variables,
                lists,
                procedures,
                &legacy_plan,
                false,
                ResumableScriptOptions {
                    function_name: &candidate.function,
                    fast_math,
                    no_warp_timer,
                },
                &mut source,
            );
            legacy_bytes += source.len();
            legacy_outputs.insert(
                candidate.function.clone(),
                ResumableStepOutput {
                    start_pc,
                    loop_count,
                    source,
                },
            );
        }

        if shared_plan.has_resumable_procedures() && shared_bytes < legacy_bytes {
            shared_procedure_steps.insert(script.target_index, shared_helper);
            resumable_outputs.extend(shared_outputs);
        } else {
            resumable_outputs.extend(legacy_outputs);
        }
    }

    // Identical scripts on the same target can share one compiled step
    // function. A Rust item alias keeps call sites statically dispatched and
    // removes the duplicate body without adding a wrapper call.
    let mut canonical_steps = HashMap::<(usize, usize, usize, String), String>::new();
    for script in &scripts {
        let step = resumable_outputs
            .get_mut(&script.function)
            .expect("every event script has a generated step function");
        let canonical_source = step
            .source
            .replace(
                &format!("fn step_{}(", script.function),
                "fn __shared_script_step(",
            )
            .replace(
                &format!("step_{}_partition_", script.function),
                "step___shared_script_partition_",
            );
        let key = (
            script.target_index,
            step.start_pc,
            step.loop_count,
            canonical_source,
        );
        if let Some(canonical_name) = canonical_steps.get(&key) {
            step.source = format!(
                "use self::{canonical_name} as step_{};\n\n",
                script.function
            );
        } else {
            canonical_steps.insert(key, format!("step_{}", script.function));
        }
    }

    let mut emitted_targets = HashSet::new();
    for script in &scripts {
        let procedures = &procedures_by_target[&script.target_index];
        let procedure_plan = &procedure_plans[&script.target_index];
        if emitted_targets.insert(script.target_index) {
            generate_direct_procedures(
                script.target,
                variables,
                lists,
                &procedures,
                DirectProcedureOptions {
                    target_index: script.target_index,
                    fast_math,
                    reachable_procedure_ids: procedure_plan.reachable_ids().clone(),
                    numeric_call_procedure_ids: procedure_plan.numeric_call_procedure_ids().clone(),
                },
                output,
            );
            if let Some(shared_step) = shared_procedure_steps.get(&script.target_index) {
                output.push_str(shared_step);
            }
        }
        let step_output = &resumable_outputs[&script.function];
        output.push_str(&step_output.source);
        let start_pc = step_output.start_pc;
        let loop_count = step_output.loop_count;
        line(
            output,
            0,
            &format!(
                "fn new_{}_for(instance: usize) -> GeneratedScript {{ GeneratedScript::{}(instance, ScriptState {{ frames: vec![ProcedureFrame::new(0, {}, vec![0; {loop_count}], vec![], false)], frame_pool: Vec::new(), __WARP_BUDGET_INIT__ }}) }}\n",
                script.function,
                script.variant,
                if start_pc == usize::MAX {
                    "usize::MAX".to_owned()
                } else {
                    start_pc.to_string()
                }
            ),
        );
        if script.hat.opcode == "event_whenflagclicked" {
            line(
                output,
                0,
                &format!(
                    "fn new_{}() -> GeneratedScript {{ new_{}_for({}) }}\n",
                    script.function, script.function, script.target_index
                ),
            );
        } else if script.hat.opcode == "control_start_as_clone" {
            line(
                output,
                0,
                &format!(
                    "fn new_{}(instance: usize) -> GeneratedScript {{ new_{}_for(instance) }}\n",
                    script.function, script.function
                ),
            );
        }
    }
}

// collect green flag hats across targets in stable target and block order.
pub(super) fn generate_scripts(
    project: &ScratchProject,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    no_warp_timer: bool,
    uses_clones: bool,
    output: &mut String,
) {
    let glide_initializer = if project.targets.iter().any(|target| {
        target.blocks.values().any(|block| {
            matches!(
                block.opcode.as_str(),
                "motion_glideto" | "motion_glidesecstoxy"
            )
        })
    }) {
        ", glide: None"
    } else {
        ""
    };
    let scripts = event_scripts(project, variables, lists);
    let values = scripts
        .iter()
        .filter(|script| script.hat.opcode == "event_whenflagclicked")
        .map(|script| format!("new_{}()", script.function))
        .collect::<Vec<_>>();
    line(
        output,
        0,
        &format!(
            "fn new_scripts() -> Vec<GeneratedScript> {{ vec![{}] }}\n",
            values.join(", ")
        ),
    );
    let mut receiver_groups = BTreeMap::<String, Vec<&GreenFlagScript<'_>>>::new();
    for script in &scripts {
        let Some(message) = &script.broadcast_message else {
            continue;
        };
        receiver_groups
            .entry(message.clone())
            .or_default()
            .push(script);
    }
    let message_keys = broadcast_message_keys(&scripts);
    let receiver_arms = receiver_groups.into_iter().map(|(message, receivers)| {
        let starters = receivers.into_iter().map(|script| format!(
            "for instance in state.instances_for_target({}) {{ scripts.push(new_{}_for(instance)); }}",
            script.target_index, script.function
        )).collect::<Vec<_>>().join(" ");
        format!("{} => {{ {} }}", message_keys[&message], starters)
    }).collect::<Vec<_>>();
    let receiver_match = if receiver_arms.is_empty() {
        "_ => {}".to_owned()
    } else {
        format!("{}, _ => {{}}", receiver_arms.join(", "))
    };
    line(
        output,
        0,
        &format!(
            "#[allow(dead_code, unused_variables, unused_mut)]\nfn new_broadcast_scripts(message: usize, state: &ScratchState) -> Vec<GeneratedScript> {{ let mut scripts = Vec::new(); match message {{ {receiver_match} }} scripts }}\n",
        ),
    );
    let mut backdrop_groups = BTreeMap::<String, Vec<&GreenFlagScript<'_>>>::new();
    for script in &scripts {
        if let Some(name) = &script.backdrop_name {
            backdrop_groups
                .entry(name.clone())
                .or_default()
                .push(script);
        }
    }
    let backdrop_arms = backdrop_groups
        .into_iter()
        .map(|(name, receivers)| {
            let starters = receivers
                .into_iter()
                .map(|script| {
                    format!(
                        "for instance in state.instances_for_target({}) {{ scripts.push(new_{}_for(instance)); }}",
                        script.target_index, script.function
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("{} => {{ {} }}", quote(&name), starters)
        })
        .collect::<Vec<_>>();
    let backdrop_match = if backdrop_arms.is_empty() {
        "_ => {}".to_owned()
    } else {
        format!("{}, _ => {{}}", backdrop_arms.join(", "))
    };
    line(
        output,
        0,
        &format!(
            "#[allow(dead_code, unused_variables, unused_mut)]\nfn new_backdrop_scripts(backdrop: &str, state: &ScratchState) -> Vec<GeneratedScript> {{ let mut scripts = Vec::new(); match backdrop {{ {backdrop_match} }} scripts }}\n"
        ),
    );
    let mut key_groups = BTreeMap::<String, Vec<&GreenFlagScript<'_>>>::new();
    for script in &scripts {
        if script.hat.opcode != "event_whenkeypressed" {
            continue;
        }
        let key = script
            .hat
            .fields
            .get("KEY_OPTION")
            .and_then(Value::as_array)
            .and_then(|field| field.first())
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        key_groups.entry(key).or_default().push(script);
    }
    let any_key_scripts = key_groups.remove("any").unwrap_or_default();
    let any_key_starters = any_key_scripts
        .iter()
        .map(|script| {
            format!(
                "for instance in state.instances_for_target({}) {{ scripts.push(new_{}_for(instance)); }}",
                script.target_index, script.function
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    let key_arms = key_groups
        .into_iter()
        .map(|(key, receivers)| {
            let starters = receivers
                .iter()
                .map(|script| {
                    format!(
                        "for instance in state.instances_for_target({}) {{ scripts.push(new_{}_for(instance)); }}",
                        script.target_index, script.function
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("{} => {{ {} }}", quote(&key), starters)
        })
        .collect::<Vec<_>>();
    let key_match = if key_arms.is_empty() {
        "_ => {}".to_owned()
    } else {
        format!("{}, _ => {{}}", key_arms.join(", "))
    };
    line(
        output,
        0,
        &format!(
            "#[allow(dead_code, unused_variables, unused_mut)]\nfn new_keypress_scripts(key: &str, state: &ScratchState) -> Vec<GeneratedScript> {{ let mut scripts = Vec::new(); {any_key_starters} match key {{ {key_match} }} scripts }}\n"
        ),
    );
    generate_click_hat_starter(&scripts, output);
    if uses_clones {
        let mut clone_targets = Vec::new();
        for target_index in 0..project.targets.len() {
            let values = scripts
                .iter()
                .filter(|script| {
                    script.target_index == target_index
                        && script.hat.opcode == "control_start_as_clone"
                })
                .map(|script| format!("new_{}(_instance)", script.function))
                .collect::<Vec<_>>();
            if !values.is_empty() {
                clone_targets.push(format!("{target_index} => vec![{}]", values.join(", ")));
            }
        }
        let clone_arms = if clone_targets.is_empty() {
            "_ => Vec::new()".to_owned()
        } else {
            format!("{}, _ => Vec::new()", clone_targets.join(", "))
        };
        line(
            output,
            0,
            &format!(
                "fn new_clone_scripts(target: usize, _instance: usize) -> Vec<GeneratedScript> {{ match target {{ {clone_arms} }} }}\n"
            ),
        );
    }

    let uses_warp_budget_expired = output.contains("script.warp_budget_expired(");
    let uses_yield_at_safe_point = output.contains("script.yield_at_safe_point(");
    let uses_warp_budget = output.contains("script.warp_budget")
        || uses_warp_budget_expired
        || (uses_yield_at_safe_point && !no_warp_timer);
    let budget_field = if uses_warp_budget {
        "warp_budget: WarpBudget,"
    } else {
        ""
    };
    let budget_init = if uses_warp_budget {
        "warp_budget: WarpBudget::new(500),"
    } else {
        ""
    };
    *output = output
        .replace("__WARP_BUDGET_FIELD__", budget_field)
        .replace("__WARP_BUDGET_INIT__", budget_init);

    let mut helpers = Vec::new();
    if output.contains("script.push_frame(") {
        helpers.push(
            "    fn push_frame(&mut self, function: usize, pc: usize, repeat_count: usize, arguments: impl IntoIterator<Item = ScratchValue>, warp_mode: bool) {\n        let mut storage = self.frame_pool.pop().unwrap_or_default();\n        storage.repeat_counts.resize(repeat_count, 0);\n        storage.repeat_counts.fill(0);\n        storage.numeric_loop_values.resize(repeat_count, 0.0);\n        storage.numeric_loop_values.fill(0.0);\n        storage.arguments.clear();\n        storage.arguments.extend(arguments);\n        self.frames.push(ProcedureFrame { function, pc, repeat_counts: storage.repeat_counts, numeric_loop_values: storage.numeric_loop_values, arguments: storage.arguments, warp_mode__GLIDE_FIELD__ });\n    }"
                .replace("__GLIDE_FIELD__", glide_initializer),
        );
    }
    if output.contains("script.frames.pop();") || output.contains("script.pop_frame();") {
        helpers.push(
            "    fn pop_frame(&mut self) {\n        if let Some(frame) = self.frames.pop() {\n            self.frame_pool.push(ProcedureFrameStorage { repeat_counts: frame.repeat_counts, numeric_loop_values: frame.numeric_loop_values, arguments: frame.arguments });\n        }\n    }"
                .to_owned(),
        );
    }
    if uses_warp_budget_expired {
        helpers.push("    fn warp_budget_expired(&mut self, clock: &dyn Clock) -> bool {\n        if !self.frames.last().is_some_and(|frame| frame.warp_mode) { self.warp_budget.reset(); return false; }\n        self.warp_budget.expired(clock)\n    }".to_owned());
    }
    if uses_yield_at_safe_point {
        if no_warp_timer {
            helpers.push(
                "    fn yield_at_safe_point(&mut self, _clock: &dyn Clock) -> bool { true }"
                    .to_owned(),
            );
        } else {
            helpers.push("    fn yield_at_safe_point(&mut self, clock: &dyn Clock) -> bool {\n        if !self.frames.last().is_some_and(|frame| frame.warp_mode) { self.warp_budget.reset(); return true; }\n        self.warp_budget.expired(clock)\n    }".to_owned());
        }
    }
    if !helpers.is_empty() {
        output.push_str("impl ScriptState {\n");
        output.push_str(&helpers.join("\n"));
        output.push_str("\n}\n\n");
    }
}

// generate event dispatch for clicks after the graphical thread has picked a target.
fn generate_click_hat_starter(scripts: &[GreenFlagScript<'_>], output: &mut String) {
    let sprite_groups = scripts
        .iter()
        .filter(|script| {
            script.hat.opcode == "event_whenthisspriteclicked" && !script.target.is_stage
        })
        .fold(
            BTreeMap::<usize, Vec<&GreenFlagScript<'_>>>::new(),
            |mut groups, script| {
                groups.entry(script.target_index).or_default().push(script);
                groups
            },
        );
    let sprite_arms = sprite_groups
        .into_iter()
        .map(|(target, scripts)| {
            let starters = scripts
                .into_iter()
                .map(|script| format!("scripts.push(new_{}_for(instance));", script.function))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{target} => {{ {starters} }}")
        })
        .collect::<Vec<_>>();
    let sprite_match = if sprite_arms.is_empty() {
        "_ => {}".to_owned()
    } else {
        format!("{}, _ => {{}}", sprite_arms.join(", "))
    };
    let stage_starters = scripts
        .iter()
        .filter(|script| script.hat.opcode == "event_whenstageclicked" && script.target.is_stage)
        .map(|script| {
            format!(
                "if let Some(instance) = state.original_instance({}) {{ scripts.push(new_{}_for(instance)); }}",
                script.target_index, script.function
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    line(
        output,
        0,
        &format!(
            "#[allow(dead_code, unused_variables, unused_mut)]\nfn new_click_scripts(target_instance: Option<usize>, state: &ScratchState) -> Vec<GeneratedScript> {{ let mut scripts = Vec::new(); match target_instance {{ Some(instance) => if let Some(target) = state.instance_target(instance) {{ match target {{ {sprite_match} }} }}, None => {{ {stage_starters} }} }} scripts }}\n"
        ),
    );
}

// poll compiled edge-hat predicates before stepping scripts, like Scratch VM's runtime step.
fn generate_edge_hat_poller(scripts: &[GreenFlagScript<'_>], output: &mut String) {
    let edges = scripts
        .iter()
        .enumerate()
        .filter(|(_, script)| script.hat.opcode == "event_whengreaterthan")
        .collect::<Vec<_>>();
    if edges.is_empty() {
        return;
    }
    output.push_str("fn poll_edge_hats(state: &mut ScratchState, now_millis: u64, previous: &mut EdgeHatState) -> Vec<GeneratedScript> { let mut starters = Vec::new();\n");
    for (index, script) in edges {
        let value = script.greater_than_value.as_deref().unwrap_or("10.0_f64");
        let predicate = match script.greater_than_option.as_deref() {
            Some("loudness") => format!("state.loudness() > scratch_to_number({value})"),
            Some("timer") => {
                format!("state.project_timer_seconds(now_millis) > scratch_to_number({value})")
            }
            _ => "false".to_owned(),
        };
        line(
            output,
            1,
            &format!(
                "for instance in state.instances_for_target({}) {{ let previous_instance = state.current_instance(); state.set_current_instance(instance); let active = {predicate}; state.set_current_instance(previous_instance); if previous.rising_edge({index}, instance, active) {{ starters.push(new_{}_for(instance)); }} }}",
                script.target_index, script.function
            ),
        );
    }
    output.push_str("    previous.retain_live_instances(|instance| state.instance_is_alive(instance)); starters\n}\n\n");
}

struct GreenFlagScript<'a> {
    target: &'a ScratchTarget,
    hat: &'a crate::parser::ScratchBlock,
    variant: String,
    function: String,
    target_index: usize,
    broadcast_message: Option<String>,
    backdrop_name: Option<String>,
    greater_than_option: Option<String>,
    greater_than_value: Option<String>,
}

fn event_scripts<'a>(
    project: &'a ScratchProject,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
) -> Vec<GreenFlagScript<'a>> {
    let mut scripts = Vec::new();
    for (target_index, target) in project.targets.iter().enumerate() {
        let target_name = rust_identifier(&target.name).to_ascii_lowercase();
        for (hat_id, hat) in event_hats(target).into_iter().filter(|(_, hat)| {
            matches!(
                hat.opcode.as_str(),
                "event_whenflagclicked"
                    | "control_start_as_clone"
                    | "event_whenbroadcastreceived"
                    | "event_whenkeypressed"
                    | "event_whenbackdropswitchesto"
                    | "event_whenthisspriteclicked"
                    | "event_whenstageclicked"
                    | "event_whengreaterthan"
            )
        }) {
            let function_type = match hat.opcode.as_str() {
                "event_whenflagclicked" => "green_flag_script",
                "control_start_as_clone" => "clone_start_script",
                "event_whenkeypressed" => "key_pressed_script",
                "event_whenbackdropswitchesto" => "backdrop_receiver_script",
                "event_whenthisspriteclicked" => "sprite_clicked_script",
                "event_whenstageclicked" => "stage_clicked_script",
                "event_whengreaterthan" => "greater_than_script",
                _ => "broadcast_receiver_script",
            };
            let stable_id = stable_script_id(target_index, hat_id);
            let function = format!("{function_type}_{target_name}_{stable_id:016x}");
            scripts.push(GreenFlagScript {
                target,
                hat,
                variant: format!("Script{stable_id:016x}"),
                function,
                target_index,
                broadcast_message: if hat.opcode == "event_whenbroadcastreceived" {
                    broadcast_message(project, hat)
                } else {
                    None
                },
                backdrop_name: if hat.opcode == "event_whenbackdropswitchesto" {
                    hat.fields
                        .get("BACKDROP")
                        .and_then(Value::as_array)
                        .and_then(|field| field.first())
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                } else {
                    None
                },
                greater_than_option: if hat.opcode == "event_whengreaterthan" {
                    Some(
                        hat.fields
                            .get("WHENGREATERTHANMENU")
                            .and_then(Value::as_array)
                            .and_then(|field| field.first())
                            .and_then(Value::as_str)
                            .unwrap_or("LOUDNESS")
                            .to_ascii_lowercase(),
                    )
                } else {
                    None
                },
                greater_than_value: if hat.opcode == "event_whengreaterthan" {
                    // compile threshold reporters through the shared expression path.
                    if hat.inputs.contains_key("VALUE") {
                        let mut active = HashSet::new();
                        Some(expression(
                            target,
                            hat.inputs.get("VALUE"),
                            variables,
                            lists,
                            &HashMap::new(),
                            &mut active,
                        ))
                    } else {
                        Some("10.0_f64".to_owned())
                    }
                } else {
                    None
                },
            });
        }
    }
    scripts
}

fn stable_script_id(target_index: usize, hat_id: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in (target_index as u64)
        .to_le_bytes()
        .into_iter()
        .chain(hat_id.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn broadcast_message_keys(scripts: &[GreenFlagScript<'_>]) -> BTreeMap<String, usize> {
    scripts
        .iter()
        .filter_map(|script| script.broadcast_message.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(index, message)| (message, index + 1))
        .collect()
}

fn broadcast_message(
    project: &ScratchProject,
    hat: &crate::parser::ScratchBlock,
) -> Option<String> {
    let field = hat.fields.get("BROADCAST_OPTION")?.as_array()?;
    let name = field.first()?.as_str()?;
    let id = field.get(1).and_then(serde_json::Value::as_str);
    let stage = project.targets.iter().find(|target| target.is_stage)?;
    id.and_then(|id| stage.broadcasts.get(id))
        .cloned()
        .or_else(|| Some(name.to_owned()))
}

fn event_hats(target: &ScratchTarget) -> Vec<(&str, &crate::parser::ScratchBlock)> {
    let mut hats: Vec<_> = target
        .blocks
        .iter()
        .filter(|(_, block)| {
            block.top_level
                && matches!(
                    block.opcode.as_str(),
                    "event_whenflagclicked"
                        | "event_whenthisspriteclicked"
                        | "event_whenstageclicked"
                        | "event_whenkeypressed"
                        | "event_whenbroadcastreceived"
                        | "event_whenbackdropswitchesto"
                        | "event_whengreaterthan"
                        | "control_start_as_clone"
                )
        })
        .map(|(id, block)| (id.as_str(), block))
        .collect();
    hats.sort_by(|(left_id, _), (right_id, _)| left_id.cmp(right_id));
    hats
}
