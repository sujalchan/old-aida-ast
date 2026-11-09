use super::*;

pub(in crate::rust) struct DirectProcedureOptions {
    pub(in crate::rust) target_index: usize,
    pub(in crate::rust) fast_math: bool,
    pub(in crate::rust) reachable_procedure_ids: HashSet<String>,
    pub(in crate::rust) numeric_call_procedure_ids: HashSet<String>,
}

pub(super) fn direct_eligible(
    target: &ScratchTarget,
    procedure: &ProcedureSignature,
    procedures: &[ProcedureSignature],
) -> bool {
    direct_eligible_inner(target, procedure, procedures, &mut HashSet::new(), true)
}

fn direct_call_eligible(
    target: &ScratchTarget,
    procedure: &ProcedureSignature,
    procedures: &[ProcedureSignature],
) -> bool {
    direct_eligible_inner(target, procedure, procedures, &mut HashSet::new(), false)
}

fn direct_eligible_inner(
    target: &ScratchTarget,
    procedure: &ProcedureSignature,
    procedures: &[ProcedureSignature],
    visiting: &mut HashSet<String>,
    allow_stop_this_script: bool,
) -> bool {
    if procedure.argument_names.len() != procedure.argument_ids.len()
        || procedure
            .argument_names
            .iter()
            .collect::<HashSet<_>>()
            .len()
            != procedure.argument_names.len()
    {
        return false;
    }
    if !visiting.insert(procedure.definition_id.clone()) {
        return false;
    }
    let Some(definition) = target.blocks.get(&procedure.definition_id) else {
        visiting.remove(&procedure.definition_id);
        return false;
    };
    let mut pending: Vec<&str> = definition.next.as_deref().into_iter().collect();
    let mut seen = HashSet::new();
    let mut eligible = true;
    while let Some(id) = pending.pop() {
        if !seen.insert(id) || seen.len() > 80 {
            eligible = false;
            break;
        }
        let Some(block) = target.blocks.get(id) else {
            eligible = false;
            break;
        };
        match block.opcode.as_str() {
            "control_if" | "control_if_else" => {
                for input in ["SUBSTACK", "SUBSTACK2"] {
                    if let Some(InputValue::BlockReference(branch)) =
                        block.inputs.get(input).and_then(parse_input)
                    {
                        pending.push(branch);
                    }
                }
            }
            "control_stop"
                if allow_stop_this_script
                    && field_value(block, "STOP_OPTION").and_then(Value::as_str)
                        == Some("this script") => {}
            "procedures_call" => {
                let Some(callee) = procedure_for_call(block, procedures) else {
                    eligible = false;
                    break;
                };
                if !direct_eligible_inner(target, callee, procedures, visiting, false) {
                    eligible = false;
                    break;
                }
            }
            "data_setvariableto"
            | "data_changevariableby"
            | "data_addtolist"
            | "data_deleteoflist"
            | "data_deletealloflist"
            | "data_insertatlist"
            | "data_replaceitemoflist"
            | "motion_gotoxy"
            | "motion_setx"
            | "motion_sety"
            | "pen_clear"
            | "pen_penDown"
            | "pen_penUp"
            | "pen_stamp"
            | "pen_setPenColorToColor"
            | "pen_changePenColorParamBy"
            | "pen_setPenColorParamTo"
            | "pen_setPenSizeTo"
            | "pen_changePenSizeBy" => {}
            _ => {
                eligible = false;
                break;
            }
        }
        if let Some(next) = block.next.as_deref() {
            pending.push(next);
        }
    }
    visiting.remove(&procedure.definition_id);
    eligible
}

pub(super) fn direct_warp_name(target_index: usize, frame_id: usize) -> String {
    format!("direct_warp_procedure_{target_index}_{frame_id}")
}

pub(super) fn direct_warp_eligible(
    target: &ScratchTarget,
    procedure: &ProcedureSignature,
    procedures: &[ProcedureSignature],
    memo: &mut HashMap<String, bool>,
    visiting: &mut HashSet<String>,
) -> bool {
    if let Some(eligible) = memo.get(&procedure.definition_id) {
        return *eligible;
    }
    // Direct warp calls are synchronous: keep them to shallow control flow
    // that cannot spin indefinitely. Counted repeat loops are finite; while,
    // repeat-until, and forever loops stay on the resumable path.
    if visiting.len() >= 64 {
        return false;
    }
    if !visiting.insert(procedure.definition_id.clone()) {
        return false;
    }
    let mut pending = target
        .blocks
        .get(&procedure.definition_id)
        .and_then(|definition| definition.next.as_deref())
        .into_iter()
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    let mut eligible = true;
    while let Some(id) = pending.pop() {
        if !seen.insert(id.to_owned()) {
            continue;
        }
        if seen.len() > 512 {
            eligible = false;
            break;
        }
        let Some(block) = target.blocks.get(id) else {
            eligible = false;
            break;
        };
        match block.opcode.as_str() {
            "control_if" | "control_if_else" | "control_repeat" => {
                for input in ["SUBSTACK", "SUBSTACK2"] {
                    if let Some(InputValue::BlockReference(branch)) =
                        block.inputs.get(input).and_then(parse_input)
                    {
                        pending.push(branch);
                    }
                }
            }
            "procedures_call" => {
                let Some(callee) = procedure_for_call(block, procedures) else {
                    eligible = false;
                    break;
                };
                if !direct_warp_eligible(target, callee, procedures, memo, visiting) {
                    eligible = false;
                    break;
                }
            }
            "data_setvariableto"
            | "data_changevariableby"
            | "data_replaceitemoflist"
            | "motion_gotoxy"
            | "pen_penDown"
            | "pen_penUp" => {}
            "control_stop"
                if field_value(block, "STOP_OPTION").and_then(Value::as_str)
                    == Some("this script") => {}
            _ => {
                eligible = false;
                break;
            }
        }
        if let Some(next) = block.next.as_deref() {
            pending.push(next);
        }
    }
    visiting.remove(&procedure.definition_id);
    memo.insert(procedure.definition_id.clone(), eligible);
    eligible
}

pub(super) fn direct_warp_eligibility(
    target: &ScratchTarget,
    procedures: &[ProcedureSignature],
) -> HashMap<String, bool> {
    let mut memo = HashMap::new();
    for procedure in procedures {
        direct_warp_eligible(
            target,
            procedure,
            procedures,
            &mut memo,
            &mut HashSet::new(),
        );
    }
    memo
}

pub(super) fn direct_warp_contains_stop(
    target: &ScratchTarget,
    procedure: &ProcedureSignature,
) -> bool {
    let mut pending = target
        .blocks
        .get(&procedure.definition_id)
        .and_then(|definition| definition.next.as_deref())
        .into_iter()
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let Some(block) = target.blocks.get(id) else {
            continue;
        };
        if block.opcode == "control_stop"
            && field_value(block, "STOP_OPTION").and_then(Value::as_str) == Some("this script")
        {
            return true;
        }
        if matches!(
            block.opcode.as_str(),
            "control_if" | "control_if_else" | "control_repeat"
        ) {
            for input in ["SUBSTACK", "SUBSTACK2"] {
                if let Some(InputValue::BlockReference(branch)) =
                    block.inputs.get(input).and_then(parse_input)
                {
                    pending.push(branch);
                }
            }
        }
        if let Some(next) = block.next.as_deref() {
            pending.push(next);
        }
    }
    false
}

pub(super) fn direct_name(target_index: usize, frame_id: usize) -> String {
    format!("direct_procedure_{target_index}_{frame_id}")
}

pub(in crate::rust) fn generate_direct_procedures(
    target: &ScratchTarget,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    procedures: &[ProcedureSignature],
    options: DirectProcedureOptions,
    output: &mut String,
) {
    let DirectProcedureOptions {
        target_index,
        fast_math,
        reachable_procedure_ids,
        numeric_call_procedure_ids,
    } = options;
    for procedure in procedures.iter().filter(|procedure| {
        reachable_procedure_ids.contains(&procedure.definition_id)
            && direct_eligible(target, procedure, procedures)
    }) {
        let arguments = procedure
            .argument_names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), format!("argument_{i}")))
            .collect::<HashMap<_, _>>();
        let params = (0..procedure.argument_ids.len())
            .map(|i| format!(", argument_{i}: ScratchValue"))
            .collect::<String>();
        let name = direct_name(target_index, procedure.frame_id);
        let numeric = super::super::direct_numeric::generate_numeric_direct(
            target, procedure, variables, lists, &name, false,
        );
        let body_name = if numeric.is_some() {
            format!("{name}_dynamic")
        } else {
            name
        };
        line(
            output,
            0,
            "#[allow(dead_code, unused_variables, unreachable_code)]",
        );
        line(
            output,
            0,
            &format!("fn {body_name}(state: &mut ScratchState, clock: &dyn Clock{params}) {{"),
        );
        let definition = &target.blocks[&procedure.definition_id];
        generate_direct_chain(
            target,
            target_index,
            definition.next.as_deref(),
            variables,
            lists,
            &arguments,
            procedures,
            output,
            1,
        );
        line(output, 0, "}\n");
        if let Some(numeric) = numeric {
            output.push_str(&numeric);
            if numeric_call_procedure_ids.contains(&procedure.definition_id) {
                output.push_str(
                    &super::super::direct_numeric::generate_numeric_direct(
                        target,
                        procedure,
                        variables,
                        lists,
                        &direct_name(target_index, procedure.frame_id),
                        true,
                    )
                    .unwrap(),
                );
            }
        }
    }
    let eligibility = direct_warp_eligibility(target, procedures);
    for procedure in procedures.iter().filter(|procedure| {
        reachable_procedure_ids.contains(&procedure.definition_id)
            && eligibility
                .get(&procedure.definition_id)
                .copied()
                .unwrap_or(false)
            && (!direct_eligible(target, procedure, procedures)
                || direct_warp_contains_stop(target, procedure))
    }) {
        let arguments = procedure
            .argument_names
            .iter()
            .enumerate()
            .map(|(index, name)| (name.clone(), format!("argument_{index}")))
            .collect::<HashMap<_, _>>();
        let params = (0..procedure.argument_ids.len())
            .map(|index| format!(", argument_{index}: ScratchValue"))
            .collect::<String>();
        let name = direct_warp_name(target_index, procedure.frame_id);
        line(
            output,
            0,
            "#[allow(dead_code, unused_variables, unreachable_code, unused_mut)]",
        );
        line(
            output,
            0,
            &format!("fn {name}(state: &mut ScratchState, clock: &dyn Clock{params}) -> bool {{"),
        );
        let definition = &target.blocks[&procedure.definition_id];
        generate_direct_warp_chain(
            target,
            target_index,
            definition.next.as_deref(),
            variables,
            lists,
            procedures,
            &eligibility,
            &arguments,
            output,
            1,
            fast_math,
        );
        line(output, 1, "true");
        line(output, 0, "}\n");
    }
}

pub(super) fn generate_direct_chain<'a>(
    target: &'a ScratchTarget,
    target_index: usize,
    mut id: Option<&'a str>,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
    procedures: &[ProcedureSignature],
    output: &mut String,
    indent: usize,
) {
    while let Some(current) = id {
        let block = &target.blocks[current];
        match block.opcode.as_str() {
            "control_if" | "control_if_else" => {
                let condition = boolean_input(
                    target,
                    block,
                    "CONDITION",
                    variables,
                    lists,
                    arguments,
                    &mut HashSet::new(),
                );
                line(output, indent, &format!("if {condition} {{"));
                let branch = |name| {
                    block
                        .inputs
                        .get(name)
                        .and_then(parse_input)
                        .and_then(|value| match value {
                            InputValue::BlockReference(id) => Some(id),
                            _ => None,
                        })
                };
                generate_direct_chain(
                    target,
                    target_index,
                    branch("SUBSTACK"),
                    variables,
                    lists,
                    arguments,
                    procedures,
                    output,
                    indent + 1,
                );
                if block.opcode == "control_if_else" {
                    line(output, indent, "} else {");
                    generate_direct_chain(
                        target,
                        target_index,
                        branch("SUBSTACK2"),
                        variables,
                        lists,
                        arguments,
                        procedures,
                        output,
                        indent + 1,
                    );
                }
                line(output, indent, "}");
            }
            "control_stop" => line(output, indent, "return;"),
            "procedures_call" => {
                let Some(callee) = procedure_for_call(block, procedures) else {
                    line(
                        output,
                        indent,
                        "unreachable!(\"direct procedure call was not resolved\");",
                    );
                    id = block.next.as_deref();
                    continue;
                };
                debug_assert!(direct_call_eligible(target, callee, procedures));
                let values = callee
                    .argument_ids
                    .iter()
                    .map(|argument_id| {
                        expression(
                            target,
                            block.inputs.get(argument_id),
                            variables,
                            lists,
                            arguments,
                            &mut HashSet::new(),
                        )
                    })
                    .map(|value| format!("({value}).to_owned().into()"))
                    .collect::<Vec<_>>();
                let args = if values.is_empty() {
                    String::new()
                } else {
                    format!(", {}", values.join(", "))
                };
                line(
                    output,
                    indent,
                    &format!(
                        "{}(state, clock{args});",
                        direct_name(target_index, callee.frame_id)
                    ),
                );
            }
            "data_setvariableto"
                if variable_field(block, "VARIABLE", variables)
                    .is_some_and(|variable| !variable.is_stage && !variable.numeric) =>
            {
                let variable = variable_field(block, "VARIABLE", variables).unwrap();
                let input = block.inputs.get("VALUE");
                let value = if let Some(source) =
                    variable_input(target, input, variables, lists, arguments)
                {
                    format!("({source}).clone()")
                } else {
                    format!(
                        "({}).into()",
                        expression(
                            target,
                            input,
                            variables,
                            lists,
                            arguments,
                            &mut HashSet::new()
                        )
                    )
                };
                line(
                    output,
                    indent,
                    &format!("let scratch_new_value: ScratchValue = {value};"),
                );
                line(
                    output,
                    indent,
                    &format!(
                        "state.set_local_value({}, scratch_new_value);",
                        variable.slot
                    ),
                );
            }
            _ => generate_resumable_statement(
                StatementContext {
                    target,
                    variables,
                    lists,
                    arguments,
                },
                block,
                usize::MAX,
                output,
                indent,
            ),
        }
        id = block.next.as_deref();
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn generate_direct_warp_chain<'a>(
    target: &'a ScratchTarget,
    target_index: usize,
    mut id: Option<&'a str>,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    procedures: &[ProcedureSignature],
    eligibility: &HashMap<String, bool>,
    arguments: &HashMap<String, String>,
    output: &mut String,
    indent: usize,
    fast_math: bool,
) {
    while let Some(current) = id {
        let Some(block) = target.blocks.get(current) else {
            return;
        };
        match block.opcode.as_str() {
            "control_if" | "control_if_else" => {
                let condition = boolean_input(
                    target,
                    block,
                    "CONDITION",
                    variables,
                    lists,
                    arguments,
                    &mut HashSet::new(),
                );
                line(output, indent, &format!("if {condition} {{"));
                let branch = |name| {
                    block
                        .inputs
                        .get(name)
                        .and_then(parse_input)
                        .and_then(|value| match value {
                            InputValue::BlockReference(id) => Some(id),
                            _ => None,
                        })
                };
                generate_direct_warp_chain(
                    target,
                    target_index,
                    branch("SUBSTACK"),
                    variables,
                    lists,
                    procedures,
                    eligibility,
                    arguments,
                    output,
                    indent + 1,
                    fast_math,
                );
                if block.opcode == "control_if_else" {
                    line(output, indent, "} else {");
                    generate_direct_warp_chain(
                        target,
                        target_index,
                        branch("SUBSTACK2"),
                        variables,
                        lists,
                        procedures,
                        eligibility,
                        arguments,
                        output,
                        indent + 1,
                        fast_math,
                    );
                }
                line(output, indent, "}");
            }
            "control_repeat" | "control_repeat_until" | "control_while" | "control_forever" => {
                let branch =
                    block.inputs.get("SUBSTACK").and_then(parse_input).and_then(
                        |value| match value {
                            InputValue::BlockReference(id) => Some(id),
                            _ => None,
                        },
                    );
                if let Some(plan) = numeric_loop_plan(target, block, variables) {
                    generate_direct_numeric_loop(
                        target, block, variables, &plan, fast_math, output, indent,
                    );
                } else {
                    match block.opcode.as_str() {
                        "control_repeat" => {
                            let times = expression(
                                target,
                                block.inputs.get("TIMES"),
                                variables,
                                lists,
                                arguments,
                                &mut HashSet::new(),
                            );
                            line(
                                output,
                                indent,
                                &format!(
                                    "let direct_repeat_count = scratch_round({times}).max(0.0) as usize;"
                                ),
                            );
                            line(output, indent, "for _ in 0..direct_repeat_count {");
                            generate_direct_warp_chain(
                                target,
                                target_index,
                                branch,
                                variables,
                                lists,
                                procedures,
                                eligibility,
                                arguments,
                                output,
                                indent + 1,
                                fast_math,
                            );
                            line(output, indent, "}");
                        }
                        "control_repeat_until" | "control_while" => {
                            let condition = boolean_input(
                                target,
                                block,
                                "CONDITION",
                                variables,
                                lists,
                                arguments,
                                &mut HashSet::new(),
                            );
                            let condition = if block.opcode == "control_repeat_until" {
                                format!("!({condition})")
                            } else {
                                condition
                            };
                            line(output, indent, &format!("while {condition} {{"));
                            generate_direct_warp_chain(
                                target,
                                target_index,
                                branch,
                                variables,
                                lists,
                                procedures,
                                eligibility,
                                arguments,
                                output,
                                indent + 1,
                                fast_math,
                            );
                            line(output, indent, "}");
                        }
                        "control_forever" => {
                            line(output, indent, "loop {");
                            generate_direct_warp_chain(
                                target,
                                target_index,
                                branch,
                                variables,
                                lists,
                                procedures,
                                eligibility,
                                arguments,
                                output,
                                indent + 1,
                                fast_math,
                            );
                            line(output, indent, "}");
                        }
                        _ => unreachable!(),
                    }
                }
            }
            "control_stop" => line(output, indent, "return false;"),
            "procedures_call" => {
                let Some(callee) = procedure_for_call(block, procedures) else {
                    line(
                        output,
                        indent,
                        "todo!(\"missing custom procedure in direct warp block\");",
                    );
                    id = block.next.as_deref();
                    continue;
                };
                debug_assert!(
                    eligibility
                        .get(&callee.definition_id)
                        .copied()
                        .unwrap_or(false)
                );
                let values = callee
                    .argument_ids
                    .iter()
                    .map(|argument_id| {
                        expression(
                            target,
                            block.inputs.get(argument_id),
                            variables,
                            lists,
                            arguments,
                            &mut HashSet::new(),
                        )
                    })
                    .map(|value| format!("({value}).to_owned().into()"))
                    .collect::<Vec<_>>();
                let args = if values.is_empty() {
                    String::new()
                } else {
                    format!(", {}", values.join(", "))
                };
                let callee_stops = direct_warp_contains_stop(target, callee);
                let callee_name = if direct_eligible(target, callee, procedures) && !callee_stops {
                    direct_name(target_index, callee.frame_id)
                } else {
                    direct_warp_name(target_index, callee.frame_id)
                };
                if callee_name.starts_with("direct_warp_procedure_") {
                    line(
                        output,
                        indent,
                        &format!("if !{callee_name}(state, clock{args}) {{ return false; }}"),
                    );
                } else {
                    line(
                        output,
                        indent,
                        &format!("{callee_name}(state, clock{args});"),
                    );
                }
            }
            _ => generate_resumable_statement(
                StatementContext {
                    target,
                    variables,
                    lists,
                    arguments,
                },
                block,
                usize::MAX,
                output,
                indent,
            ),
        }
        id = block.next.as_deref();
    }
}

pub(super) fn generate_direct_numeric_loop(
    target: &ScratchTarget,
    block: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
    plan: &NumericLoopPlan,
    fast_math: bool,
    output: &mut String,
    indent: usize,
) {
    let locals = plan
        .local_slots
        .iter()
        .map(|slot| (*slot, format!("scratch_local_{slot}")))
        .collect::<HashMap<_, _>>();
    for slot in &plan.local_slots {
        line(
            output,
            indent,
            &format!(
                "let mut scratch_local_{slot} = {};",
                variable_read_for_slot(variables, *slot)
            ),
        );
    }
    emit_numeric_stability_flags(plan, output, indent);
    match block.opcode.as_str() {
        "control_repeat" => {
            let times = local_expression(
                target,
                block.inputs.get("TIMES"),
                variables,
                &locals,
                fast_math,
                &mut HashSet::new(),
            )
            .expect("numeric repeat count was validated");
            line(
                output,
                indent,
                &format!(
                    "let mut scratch_direct_remaining = scratch_round({}).max(0.0) as usize;",
                    local_to_number(times)
                ),
            );
            line(output, indent, "while scratch_direct_remaining > 0 {");
            generate_native_numeric_body(
                target,
                variables,
                plan,
                &locals,
                fast_math,
                None,
                output,
                indent + 1,
            );
            line(output, indent + 1, "scratch_direct_remaining -= 1;");
            line(output, indent, "}");
        }
        "control_repeat_until" | "control_while" | "control_forever" => {
            line(output, indent, "loop {");
            if block.opcode != "control_forever" {
                let condition = local_expression(
                    target,
                    block.inputs.get("CONDITION"),
                    variables,
                    &locals,
                    fast_math,
                    &mut HashSet::new(),
                )
                .expect("numeric loop condition was validated");
                let condition = local_to_bool(condition);
                let stop = if block.opcode == "control_repeat_until" {
                    condition
                } else {
                    format!("!({condition})")
                };
                line(output, indent + 1, &format!("if {stop} {{ break; }}"));
            }
            generate_native_numeric_body(
                target,
                variables,
                plan,
                &locals,
                fast_math,
                None,
                output,
                indent + 1,
            );
            line(output, indent, "}");
        }
        _ => unreachable!("numeric local plan requires a Scratch loop"),
    }
    generate_local_commits(plan, variables, output, indent);
}

// emit a resumable step function for one green flag script.
