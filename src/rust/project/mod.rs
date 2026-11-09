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

//! builds the generated Rust module and initializes its Scratch variables.
//!
//! allocates stable Rust identifiers for project variables, writes saved
//! variable values, then delegates each target's scripts to the script module.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::parser::{InputValue, ScratchProject, ScratchTarget, parse_input};

use super::{
    expressions::{constant_numeric_input, expression},
    literals::{line, literal, quote},
    scripts::{generate_green_flag_functions, generate_scripts},
};

const EXTERNAL_LIST_DATA_THRESHOLD: usize = 32 * 1024;

#[derive(Clone)]
pub(super) struct VariableInfo {
    // original Scratch ID; map keys use target-scoped identities instead.
    pub(super) id: String,
    pub(super) slot: usize,
    pub(super) name: String,
    pub(super) target: String,
    pub(super) is_stage: bool,
    pub(super) target_index: usize,
    pub(super) target_variable_order: usize,
    pub(super) numeric: bool,
    pub(super) canonical_numeric: bool,
    pub(super) non_nan: bool,
    pub(super) monitor_visible: bool,
    pub(super) index_read_override: Option<String>,
}

pub(super) fn variable_key(target_index: usize, id: &str) -> String {
    format!("{id}\u{1f}{target_index}")
}

pub(super) fn variable_for_target<'a>(
    target: &ScratchTarget,
    id: &str,
    variables: &'a HashMap<String, VariableInfo>,
) -> Option<&'a VariableInfo> {
    variable_for_target_index(target.target_index, id, variables)
}

pub(super) fn variable_for_target_index<'a>(
    target_index: usize,
    id: &str,
    variables: &'a HashMap<String, VariableInfo>,
) -> Option<&'a VariableInfo> {
    variables
        .get(&variable_key(target_index, id))
        .or_else(|| {
            variables
                .values()
                .find(|variable| variable.target_index == target_index && variable.id == id)
        })
        .or_else(|| {
            variables
                .values()
                .find(|variable| variable.is_stage && variable.id == id)
        })
}

pub(super) fn variable_key_for_target(target: &ScratchTarget, id: &str) -> String {
    if target.variables.contains_key(id) {
        return variable_key(target.target_index, id);
    }
    let stage_index = target
        .target_identities
        .iter()
        .find(|(_, _, is_stage)| *is_stage)
        .map(|(_, target_index, _)| *target_index)
        .unwrap_or(target.target_index);
    variable_key(stage_index, id)
}

impl VariableInfo {
    pub(super) fn read_expression(&self) -> String {
        if let Some(value) = &self.index_read_override {
            return value.clone();
        }
        let method = if self.is_stage { "get_shared" } else { "get" };
        format!("state.{method}({})", self.slot)
    }

    pub(super) fn write_statement(&self, value: &str) -> String {
        let method = if self.is_stage { "set_shared" } else { "set" };
        format!("state.{method}({}, {value});", self.slot)
    }
}

#[derive(Clone)]
pub(super) struct ListInfo {
    pub(super) slot: usize,
    pub(super) name: String,
    pub(super) target: String,
    pub(super) is_stage: bool,
    pub(super) target_index: usize,
    pub(super) cache_numeric_reads: bool,
    pub(super) monitor_possible: bool,
}

impl ListInfo {
    pub(super) fn read_expression(&self) -> String {
        let method = if self.is_stage { "stage_list" } else { "list" };
        format!("state.{method}({})", self.slot)
    }
}

/// generate the module header, saved variable declarations, and target scripts.
#[cfg(test)]
pub fn generate(project: &ScratchProject) -> String {
    generate_with_options(project, false)
}

#[cfg(test)]
pub fn generate_with_options(project: &ScratchProject, fast_math: bool) -> String {
    generate_internal(project, fast_math, None).0
}

/// Generate a runtime module and, when its initial lists are large, a companion
/// JSON payload that keeps those values out of Rust source literals.
pub fn generate_with_list_data(
    project: &ScratchProject,
    fast_math: bool,
    list_data_filename: &str,
) -> (String, Option<Vec<u8>>) {
    generate_internal(project, fast_math, Some(list_data_filename))
}

fn generate_internal(
    project: &ScratchProject,
    fast_math: bool,
    list_data_filename: Option<&str>,
) -> (String, Option<Vec<u8>>) {
    let variables = allocate_variables(project);
    let lists = allocate_lists(project);
    let uses_broadcasts = project.targets.iter().any(|target| {
        target.blocks.values().any(|block| {
            matches!(
                block.opcode.as_str(),
                "event_broadcast" | "event_broadcastandwait" | "event_whenbroadcastreceived"
            )
        })
    });
    let uses_clones = project.targets.iter().any(|target| {
        target.blocks.values().any(|block| {
            matches!(
                block.opcode.as_str(),
                "control_create_clone_of" | "control_delete_this_clone" | "control_start_as_clone"
            )
        })
    });
    // omit audio polling entirely from projects without executable sound blocks.
    let uses_sound = project.targets.iter().any(|target| {
        target.blocks.values().any(|block| {
            matches!(
                block.opcode.as_str(),
                "sound_play"
                    | "sound_playuntildone"
                    | "sound_stopallsounds"
                    | "sound_seteffectto"
                    | "sound_changeeffectby"
                    | "sound_cleareffects"
                    | "sound_changevolumeby"
                    | "sound_setvolumeto"
                    | "sound_volume"
            )
        })
    });
    // emit renderer input and edge polling only for projects that use those hats.
    let uses_click_hats = project.targets.iter().any(|target| {
        target.blocks.values().any(|block| {
            matches!(
                block.opcode.as_str(),
                "event_whenthisspriteclicked" | "event_whenstageclicked"
            )
        })
    });
    let uses_edge_hats = project.targets.iter().any(|target| {
        target
            .blocks
            .values()
            .any(|block| block.opcode == "event_whengreaterthan")
    });
    let uses_backdrop_switches = project.targets.iter().any(|target| {
        target.blocks.values().any(|block| {
            matches!(
                block.opcode.as_str(),
                "looks_switchbackdropto" | "looks_nextbackdrop"
            )
        })
    });
    let uses_timed_speech = project.targets.iter().any(|target| {
        target.blocks.values().any(|block| {
            matches!(
                block.opcode.as_str(),
                "looks_sayforsecs" | "looks_thinkforsecs"
            )
        })
    });
    // timer hats share the same project clock used by Scratch timer reporters.
    let uses_project_timer = project.targets.iter().any(|target| {
        target.blocks.values().any(|block| {
            matches!(
                block.opcode.as_str(),
                "sensing_timer" | "sensing_resettimer"
            ) || (block.opcode == "event_whengreaterthan"
                && block
                    .fields
                    .get("WHENGREATERTHANMENU")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|field| field.first())
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("TIMER")))
        })
    }) || project
        .monitors
        .iter()
        .any(|monitor| monitor.opcode == "sensing_timer");
    let has_visible_timer_monitor = project
        .monitors
        .iter()
        .any(|monitor| monitor.opcode == "sensing_timer" && monitor.visible);
    let has_live_sensing_monitors = project.monitors.iter().any(|monitor| {
        monitor.visible
            && matches!(
                monitor.opcode.as_str(),
                "sensing_current" | "sensing_dayssince2000"
            )
    });
    let has_sound_volume_monitor = project
        .monitors
        .iter()
        .any(|monitor| monitor.opcode == "sound_volume");
    let has_sound_volume_setter = project.targets.iter().any(|target| {
        target.blocks.values().any(|block| {
            matches!(
                block.opcode.as_str(),
                "sound_setvolumeto" | "sound_changevolumeby"
            )
        })
    });
    let expire_bubbles = if uses_timed_speech {
        "self.state.expire_speech_bubbles(self.scheduler.now_millis()); "
    } else {
        ""
    };
    let mut output = String::new();

    generate_green_flag_functions(project, &variables, &lists, fast_math, false, &mut output);
    generate_scripts(project, &variables, &lists, false, uses_clones, &mut output);

    // refresh volume monitor snapshots only for projects that can change volume.
    if has_sound_volume_setter {
        output.push_str(&format!(
            "const REFRESH_SOUND_VOLUME_MONITOR: bool = {has_sound_volume_monitor};\n"
        ));
    }

    let edge_state_field = if uses_edge_hats {
        ", edge_hat_previous: EdgeHatState"
    } else {
        ""
    };
    let edge_state_init = if uses_edge_hats {
        ", edge_hat_previous: EdgeHatState::default()"
    } else {
        ""
    };
    output.push_str(&format!("#[allow(dead_code)]\npub struct GeneratedProgram {{ state: ScratchState, scheduler: ScriptScheduler<GeneratedScript>, frame_order: Vec<u64>{edge_state_field} }}\n\n#[allow(dead_code)]\nimpl GeneratedProgram {{\n    pub fn new() -> Self {{ Self::build(None) }}\n    pub fn new_with_clock(clock: Box<dyn Clock>) -> Self {{ Self::build(Some(clock)) }}\n    pub fn uses_sound() -> bool {{ {uses_sound} }}\n    pub fn set_sound_library(&mut self, library: SoundLibrary) {{ self.state.set_sound_library(library); }}\n    pub fn enable_realtime_audio(&mut self) -> Result<(), String> {{ self.state.set_realtime_audio(RealtimeAudio::open()?); Ok(()) }}\n    fn build(clock: Option<Box<dyn Clock>>) -> Self {{\n"));

    let mut initial_values = vec![String::new(); variables.len()];
    let mut variable_metadata = vec![String::new(); variables.len()];
    for info in variables.values() {
        let variable = project
            .targets
            .get(info.target_index)
            .and_then(|target| target.variables.get(&info.id))
            .expect("every allocated variable has an initializer");
        initial_values[info.slot] = format!("({}).into()", literal(&variable.value));
        variable_metadata[info.slot] = format!(
            "ScratchVariableMetadata::for_target({}, {}, {}, {}, {}).with_monitor_visible({})",
            quote(&info.id),
            quote(&info.name),
            quote(&info.target),
            info.target_index,
            info.is_stage,
            info.monitor_visible
        );
    }
    let mut serialized_lists = vec![String::new(); lists.len()];
    let mut list_metadata = vec![String::new(); lists.len()];
    for (id, info) in &lists {
        let list = project
            .targets
            .get(info.target_index)
            .and_then(|target| target.lists.get(id))
            .expect("every allocated list has saved contents");
        serialized_lists[info.slot] = serde_json::to_string(&list.items)
            .expect("Scratch list values can be serialized as JSON");
        list_metadata[info.slot] = format!(
            "ScratchListMetadata::for_target({}, {}, {}, {}, {})",
            quote(id),
            quote(&info.name),
            quote(&info.target),
            info.target_index,
            info.is_stage
        );
    }

    let total_list_data_bytes = serialized_lists.iter().map(String::len).sum::<usize>();
    let use_external_list_data =
        list_data_filename.is_some() && total_list_data_bytes >= EXTERNAL_LIST_DATA_THRESHOLD;
    let mut external_list_data = use_external_list_data.then(Vec::new);
    let mut initial_lists = vec![String::new(); lists.len()];
    if use_external_list_data {
        let filename = list_data_filename.expect("external list data requires a filename");
        line(
            &mut output,
            1,
            &format!(
                "let initial_list_data: &[u8] = include_bytes!({});",
                quote(filename)
            ),
        );
        for (slot, serialized_items) in serialized_lists.iter().enumerate() {
            let bytes = external_list_data
                .as_mut()
                .expect("external list data was allocated");
            let start = bytes.len();
            bytes.extend_from_slice(serialized_items.as_bytes());
            let end = bytes.len();
            initial_lists[slot] =
                format!("ScratchList::from_json_bytes(&initial_list_data[{start}..{end}])");
        }
    } else {
        for (slot, serialized_items) in serialized_lists.iter().enumerate() {
            initial_lists[slot] = format!("ScratchList::from_json({})", quote(serialized_items));
        }
    }
    line(
        &mut output,
        1,
        &format!(
            "let mut state = ScratchState::with_project_data(vec![{}], vec![{}], vec![{}], vec![{}], vec![{}]);",
            initial_values.join(", "),
            variable_metadata.join(", "),
            initial_lists.join(", "),
            list_metadata.join(", "),
            lists
                .keys()
                .map(|id| project
                    .monitors
                    .iter()
                    .any(|monitor| monitor.visible && monitor.id == *id)
                    .to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    if has_visible_timer_monitor {
        line(
            &mut output,
            1,
            "state.set_project_timer_monitor_visible(true);",
        );
    }

    if let Some(stage) = project.targets.iter().find(|target| target.is_stage) {
        let names = stage
            .costumes
            .iter()
            .map(|costume| format!("{}.to_owned()", quote(&costume.name)))
            .collect::<Vec<_>>()
            .join(", ");
        line(
            &mut output,
            1,
            &format!(
                "state.set_backdrops(vec![{names}], {});",
                stage.current_costume
            ),
        );
    }

    for (target_index, target) in project.targets.iter().enumerate() {
        line(
            &mut output,
            1,
            &format!(
                "state.register_target({target_index}, {}, {});",
                quote(&target.name),
                target.is_stage
            ),
        );
        line(
            &mut output,
            1,
            &format!(
                "state.set_original_volume({target_index}, {}_f64);",
                target.volume
            ),
        );
        line(
            &mut output,
            1,
            &format!(
                "state.set_original_xy({target_index}, {}_f64, {}_f64);",
                target.x, target.y
            ),
        );
        line(
            &mut output,
            1,
            &format!(
                "state.set_original_visual({target_index}, {}_f64, {}_f64, {});",
                target.direction, target.size, target.visible
            ),
        );
        // seed the runtime drawable with the serialized scratch rotation style.
        line(
            &mut output,
            1,
            &format!(
                "state.set_original_rotation_style({target_index}, {});",
                quote(&target.rotation_style)
            ),
        );
        let costume_metadata = target
            .costumes
            .iter()
            .map(|costume| {
                let size = costume.logical_size.map_or_else(
                    || "None".to_owned(),
                    |[width, height]| format!("Some([{width:?}_f64, {height:?}_f64])"),
                );
                format!("({}.to_owned(), {size})", quote(&costume.name))
            })
            .collect::<Vec<_>>()
            .join(", ");
        line(
            &mut output,
            1,
            &format!(
                "state.set_target_costumes_with_sizes({target_index}, vec![{costume_metadata}], {});",
                target.current_costume
            ),
        );
    }

    output.push_str("let scripts = new_scripts();\nlet scheduler = match clock { Some(clock) => ScriptScheduler::with_clock(scripts, clock), None => ScriptScheduler::new(scripts) };\n");
    if uses_project_timer {
        output.push_str("state.reset_project_timer(scheduler.now_millis());\n");
    }
    output.push_str(&format!(
        "    Self {{ state, scheduler, frame_order: Vec::new(){edge_state_init} }}\n    }}\n\n"
    ));
    output.push_str(
        "    pub fn step(&mut self) -> bool { self.advance() == SchedulerStatus::Progressed }\n",
    );
    if uses_edge_hats {
        output.push_str("    fn update_edge_hats(&mut self) { let starters = poll_edge_hats(&mut self.state, self.scheduler.now_millis(), &mut self.edge_hat_previous); self.scheduler.start_edge_hats(starters); }\n");
    }
    output.push_str("    fn process_after_step(&mut self) {");
    if uses_broadcasts {
        output.push_str(" if let Some((sender, message, wait)) = self.scheduler.take_broadcast() { let receivers = new_broadcast_scripts(message, &self.state); self.scheduler.broadcast(sender, receivers, wait); }");
    }
    if uses_clones {
        output.push_str(" let deleted = self.state.current_instance(); if !self.state.instance_is_alive(deleted) { self.scheduler.remove_target_scripts(deleted); } for instance in self.state.take_pending_clones() { if let Some(target) = self.state.instance_target(instance) { self.scheduler.add_scripts(new_clone_scripts(target, instance)); } }");
    }
    if uses_backdrop_switches {
        output.push_str(" for backdrop in self.state.take_pending_backdrop_events() { let receivers = new_backdrop_scripts(&backdrop, &self.state); self.scheduler.add_scripts(receivers); }");
    }
    output.push_str(" }\n");
    let edge_poll = if uses_edge_hats {
        "self.update_edge_hats(); "
    } else {
        ""
    };
    let sound_poll = if uses_sound {
        "self.state.advance_sounds(self.scheduler.now_millis()); for sound_wait in self.state.drain_finished_sounds() { self.scheduler.signal(sound_wait); } "
    } else {
        ""
    };
    let sound_complete = if uses_sound {
        "if status == SchedulerStatus::Complete && self.state.has_active_sounds() { SchedulerStatus::WaitingForSignal } else { status }"
    } else {
        "status"
    };
    output.push_str("    pub fn advance(&mut self) -> SchedulerStatus { ");
    output.push_str(sound_poll);
    output.push_str(edge_poll);
    output.push_str(
        "let status = self.scheduler.advance(&mut self.state); self.process_after_step(); ",
    );
    output.push_str(sound_complete);
    output.push_str(" }\n");
    if uses_broadcasts || uses_clones || uses_backdrop_switches {
        output.push_str(&format!("    pub fn advance_frame(&mut self) -> SchedulerStatus {{ {sound_poll}{expire_bubbles}{edge_poll}self.scheduler.script_ids_in_frame_order_into(&mut self.frame_order); let mut cursor = 0; let mut progressed = false; while cursor < self.frame_order.len() {{ let id = self.frame_order[cursor]; cursor += 1; if let Some(status) = self.scheduler.advance_script_id(id, &mut self.state) {{ if status == SchedulerStatus::Complete {{ return if self.state.has_active_sounds() {{ SchedulerStatus::WaitingForSignal }} else {{ SchedulerStatus::Complete }}; }} progressed = true; self.process_after_step(); }} }} let status = if progressed {{ SchedulerStatus::Progressed }} else {{ self.scheduler.idle_status() }}; {sound_complete} }}\n"));
    } else {
        output.push_str(&format!("    pub fn advance_frame(&mut self) -> SchedulerStatus {{ {sound_poll}{expire_bubbles}{edge_poll}self.scheduler.script_ids_in_frame_order_into(&mut self.frame_order); let mut cursor = 0; let mut progressed = false; while cursor < self.frame_order.len() {{ let id = self.frame_order[cursor]; cursor += 1; if let Some(status) = self.scheduler.advance_script_id(id, &mut self.state) {{ if status == SchedulerStatus::Complete {{ return if self.state.has_active_sounds() {{ SchedulerStatus::WaitingForSignal }} else {{ SchedulerStatus::Complete }}; }} progressed = true; }} }} let status = if progressed {{ SchedulerStatus::Progressed }} else {{ self.scheduler.idle_status() }}; {sound_complete} }}\n"));
    }
    output.push_str("    pub fn duration_until(&self, deadline: u64) -> std::time::Duration { self.scheduler.duration_until(deadline) }\n");
    if uses_project_timer {
        let clear_edges = if uses_edge_hats {
            "self.edge_hat_previous.clear(); "
        } else {
            ""
        };
        let restart_sounds = if uses_sound {
            "self.state.restart_sounds(self.scheduler.now_millis()); "
        } else {
            ""
        };
        output.push_str(&format!("    pub fn restart(&mut self) {{ {clear_edges}{restart_sounds}self.state.remove_clones(); self.scheduler.restart(new_scripts()); self.state.reset_project_timer(self.scheduler.now_millis()); }}\n"));
    } else {
        let clear_edges = if uses_edge_hats {
            "self.edge_hat_previous.clear(); "
        } else {
            ""
        };
        let restart_sounds = if uses_sound {
            "self.state.restart_sounds(self.scheduler.now_millis()); "
        } else {
            ""
        };
        output.push_str(&format!("    pub fn restart(&mut self) {{ {clear_edges}{restart_sounds}self.state.remove_clones(); self.scheduler.restart(new_scripts()); }}\n"));
    }
    // stopping scripts leaves edge predicates intact so true hats cannot fire again immediately.
    output
        .push_str("    pub fn stop_all(&mut self) { self.scheduler.stop_all(&mut self.state); }\n");
    if uses_sound {
        output.push_str("    pub fn is_complete(&self) -> bool { self.scheduler.active_scripts() == 0 && !self.state.has_active_sounds() }\n");
    } else {
        output.push_str(
            "    pub fn is_complete(&self) -> bool { self.scheduler.active_scripts() == 0 }\n",
        );
    }
    output.push_str(
        "    pub fn active_script_count(&self) -> usize { self.scheduler.active_scripts() }\n",
    );
    output.push_str("    pub fn state(&self) -> &ScratchState { &self.state }\n");
    output.push_str("    pub fn set_username(&mut self, username: String) { self.state.set_username(username); }\n");
    output.push_str(&format!(
        "    pub fn has_live_sensing_monitors(&self) -> bool {{ {has_live_sensing_monitors} }}\n"
    ));
    output.push_str("    pub fn set_target_fence_geometry(&mut self, target: usize, geometry: SpriteFenceGeometry) { self.state.set_target_fence_geometry(target, geometry); }\n");
    output.push_str("    pub fn begin_tick(&mut self) { self.state.begin_tick(); }\n");
    output
        .push_str("    pub fn redraw_requested(&self) -> bool { self.state.redraw_requested() }\n");
    output.push_str("    pub fn presentation_requested(&self) -> bool { self.state.presentation_requested() }\n");
    output.push_str(
        "    pub fn presentation_consumed(&mut self) { self.state.presentation_consumed(); }\n",
    );
    // pass key events through the scheduler's non-restarting per-instance hat gate.
    if uses_click_hats {
        output.push_str("    pub fn apply_input(&mut self, event: RuntimeInputEvent) { match event { RuntimeInputEvent::PointerButton { pressed, inside_stage, target_instance, draggable, was_dragged, monitor_interaction } => { if pointer_click_activates(pressed, inside_stage, draggable, was_dragged, monitor_interaction) { self.scheduler.start_click_hats(new_click_scripts(target_instance, &self.state)); } }, event => if let Some(key) = self.state.apply_input(event) { let scripts = new_keypress_scripts(&key, &self.state); self.scheduler.start_key_hats(scripts); } } }\n");
    } else {
        output.push_str("    pub fn apply_input(&mut self, event: RuntimeInputEvent) { if let Some(key) = self.state.apply_input(event) { let scripts = new_keypress_scripts(&key, &self.state); self.scheduler.start_key_hats(scripts); } }\n");
    }
    output.push_str(
        "    pub fn set_loudness(&mut self, value: f64) { self.state.set_loudness(value); }\n",
    );
    let monitor_reporters = reporter_monitor_values(project, &variables, &lists);
    let snapshot_base = if uses_project_timer {
        "self.state.observable_snapshot_filtered_with_timer(tick, self.scheduler.now_millis(), include_hidden_items)"
    } else {
        "self.state.observable_snapshot_filtered(tick, include_hidden_items)"
    };
    let snapshot_binding = if monitor_reporters.is_empty() {
        "let snapshot"
    } else {
        "let mut snapshot"
    };
    // reporter monitors with connected inputs temporarily use their declared target context.
    output.push_str("    pub fn observable_snapshot(&mut self, tick: usize) -> RuntimeSnapshot { self.snapshot(tick, true) }\n");
    output.push_str("    pub fn renderer_snapshot(&mut self, tick: usize) -> RuntimeSnapshot { self.snapshot(tick, false) }\n");
    output.push_str(&format!("    fn snapshot(&mut self, tick: usize, include_hidden_items: bool) -> RuntimeSnapshot {{ {snapshot_binding} = {snapshot_base};\n"));
    if !monitor_reporters.is_empty() {
        output.push_str(&format!(
            "        snapshot.reporters = Vec::with_capacity({});\n",
            monitor_reporters.len()
        ));
        for (key, expression, target_context) in monitor_reporters {
            if let Some(target_index) = target_context {
                line(
                    &mut output,
                    2,
                    &format!(
                        "{{ let previous_instance = self.state.current_instance(); let value: ScratchValue = if let Some(instance) = self.state.original_instance({target_index}) {{ self.state.set_current_instance(instance); let value = {{ let state = &self.state; let _ = state; ({expression}).into() }}; self.state.set_current_instance(previous_instance); value }} else {{ let state = &self.state; let _ = state; ({expression}).into() }}; snapshot.reporters.push(ReporterSnapshot {{ key: {}.to_owned(), value }}); }}",
                        quote(&key)
                    ),
                );
            } else {
                line(
                    &mut output,
                    2,
                    &format!(
                        "{{ let state = &self.state; let _ = state; let value: ScratchValue = ({}).into(); snapshot.reporters.push(ReporterSnapshot {{ key: {}.to_owned(), value }}); }}",
                        expression,
                        quote(&key),
                    ),
                );
            }
        }
    }
    output.push_str("        snapshot }\n");
    output.push_str("    pub fn take_pending_pen_commands(&mut self) -> Vec<PenCommand> { self.state.take_pending_pen_commands() }\n");
    output.push_str("    pub fn set_observable_event_sink(&mut self, sink: Option<std::sync::mpsc::Sender<ObservableEvent>>) { self.state.set_event_sink(sink); }\n");
    output.push_str("    pub fn run_until_complete(&mut self) { loop { match self.advance_frame() { SchedulerStatus::Progressed => {}, SchedulerStatus::WaitingUntil(deadline) => std::thread::sleep(self.duration_until(deadline)), SchedulerStatus::WaitingForSignal => std::thread::sleep(std::time::Duration::from_millis(8)), SchedulerStatus::Complete => break } } }\n}\n\n");
    output.push_str("#[allow(dead_code)]\npub fn run() { let mut program = GeneratedProgram::new(); program.run_until_complete(); }\n");
    let (output, uses_pc_setter, uses_pc_transition, uses_frame_accessors) =
        compact_resumable_state_source(&output);
    let identifiers = rust_identifiers(&output);
    let imports = runtime_imports(&identifiers);
    let print_value_macro = if identifiers.contains("print_value") {
        "macro_rules! print_value {\n    ($state:expr, $name:expr, $value:expr) => {{\n        let value = $value;\n        $state.emit_observable_output($name, &value);\n    }};\n}\n\n"
    } else {
        ""
    };
    let state_machine_macros = match (uses_pc_setter, uses_pc_transition) {
        (false, false) => "",
        (true, false) => {
            "macro_rules! __aida_set_pc { ($script:expr, $pc:expr) => { ($script).frame_mut().pc = $pc; }; }\n\n"
        }
        (false, true) => {
            "macro_rules! __aida_continue_at { ($script:expr, $pc:expr) => {{ ($script).frame_mut().pc = $pc; continue; }}; }\n\n"
        }
        (true, true) => {
            "macro_rules! __aida_set_pc { ($script:expr, $pc:expr) => { ($script).frame_mut().pc = $pc; }; }\nmacro_rules! __aida_continue_at { ($script:expr, $pc:expr) => {{ ($script).frame_mut().pc = $pc; continue; }}; }\n\n"
        }
    };
    let frame_accessors = if uses_frame_accessors {
        "\nimpl ScriptState {\n    #[inline(always)]\n    fn frame(&self) -> &ProcedureFrame { self.frames.last().unwrap() }\n    #[inline(always)]\n    fn frame_mut(&mut self) -> &mut ProcedureFrame { self.frames.last_mut().unwrap() }\n    #[inline(always)]\n    fn argument(&self, index: usize) -> &ScratchValue { let frame = self.frame(); debug_assert!(!frame.numeric_only_arguments && index < frame.argument_count); &self.arguments[frame.argument_start + index] }\n    #[inline(always)]\n    fn numeric_argument(&self, index: usize) -> f64 { let frame = self.frame(); debug_assert!(frame.numeric_only_arguments && index < frame.numeric_argument_count); self.numeric_arguments[frame.numeric_argument_start + index] }\n    #[inline(always)]\n    fn push_frame(&mut self, function: usize, pc: usize, repeat_counts: smallvec::SmallVec<[usize; 4]>, arguments: smallvec::SmallVec<[ScratchValue; 16]>, warp_mode: bool) { let argument_start = self.arguments.len(); let argument_count = arguments.len(); self.arguments.extend(arguments); self.frames.push(ProcedureFrame::new(function, pc, repeat_counts, argument_start, argument_count, 0, 0, false, warp_mode)); }\n    #[inline(always)]\n    fn push_numeric_frame(&mut self, function: usize, pc: usize, repeat_counts: smallvec::SmallVec<[usize; 4]>, arguments: smallvec::SmallVec<[f64; 16]>, warp_mode: bool) { let numeric_argument_start = self.numeric_arguments.len(); let numeric_argument_count = arguments.len(); self.numeric_arguments.extend(arguments); self.frames.push(ProcedureFrame::new(function, pc, repeat_counts, 0, 0, numeric_argument_start, numeric_argument_count, true, warp_mode)); }\n    #[inline(always)]\n    fn pop_frame(&mut self) -> Option<ProcedureFrame> { let frame = self.frames.pop()?; if frame.numeric_only_arguments { self.numeric_arguments.truncate(frame.numeric_argument_start); } else { self.arguments.truncate(frame.argument_start); } Some(frame) }\n}\n"
    } else {
        ""
    };
    (
        format!("{imports}{print_value_macro}{state_machine_macros}{output}{frame_accessors}"),
        external_list_data,
    )
}

/// Factor the repeated generated frame lookups and program-counter transitions
/// into always-inlined accessors and compile-time macros. The macros expand at
/// each call site, so they add no runtime dispatch or function-call overhead.
fn compact_resumable_state_source(source: &str) -> (String, bool, bool, bool) {
    let source = source
        .replace("script.frames.last().unwrap()", "script.frame()")
        .replace("script.frames.last_mut().unwrap()", "script.frame_mut()");
    let mut output = String::with_capacity(source.len());
    let mut uses_pc_setter = false;
    let mut uses_pc_transition = false;
    let mut uses_frame_accessors = source.contains("script.frame()")
        || source.contains("script.frame_mut()")
        || source.contains("script.push_frame(")
        || source.contains("script.push_numeric_frame(")
        || source.contains("script.numeric_argument(")
        || source.contains("script.pop_frame()");
    let lines = source.split_inclusive('\n').collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let without_newline = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = without_newline.trim_start();
        let indent_len = without_newline.len() - trimmed.len();
        if let Some(assignment) = trimmed.strip_prefix("script.frame_mut().pc = ") {
            if let Some(pc) = assignment.strip_suffix(';') {
                if lines
                    .get(index + 1)
                    .is_some_and(|next| next.trim() == "continue;")
                {
                    output.push_str(&format!(
                        "{}__aida_continue_at!(script, {pc});\n",
                        &without_newline[..indent_len]
                    ));
                    uses_pc_transition = true;
                    uses_frame_accessors = true;
                    index += 2;
                    continue;
                }
                output.push_str(&format!(
                    "{}__aida_set_pc!(script, {pc});\n",
                    &without_newline[..indent_len]
                ));
                uses_pc_setter = true;
                uses_frame_accessors = true;
                index += 1;
                continue;
            }
        }
        output.push_str(line);
        index += 1;
    }
    (
        output,
        uses_pc_setter,
        uses_pc_transition,
        uses_frame_accessors,
    )
}

mod analysis;
mod imports;
mod monitors;

use analysis::*;
use imports::*;
use monitors::*;

pub(super) use analysis::rust_identifier;
