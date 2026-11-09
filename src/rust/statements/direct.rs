use super::*;

pub(in crate::rust) struct DirectProcedureOptions {
    pub(in crate::rust) target_index: usize,
    pub(in crate::rust) fast_math: bool,
    pub(in crate::rust) no_warp_timer: bool,
    pub(in crate::rust) reachable_procedure_ids: HashSet<String>,
    pub(in crate::rust) numeric_call_procedure_ids: HashSet<String>,
    pub(in crate::rust) numeric_arguments: HashMap<String, Vec<bool>>,
    pub(in crate::rust) numeric_warp_procedure_ids: HashSet<String>,
}

pub(super) fn direct_eligible(target: &ScratchTarget, procedure: &ProcedureSignature) -> bool {
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
    let Some(definition) = target.blocks.get(&procedure.definition_id) else {
        return false;
    };
    let mut pending: Vec<&str> = definition.next.as_deref().into_iter().collect();
    let mut seen = HashSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) || seen.len() > 80 {
            return false;
        }
        let Some(block) = target.blocks.get(id) else {
            return false;
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
                if field_value(block, "STOP_OPTION").and_then(Value::as_str)
                    == Some("this script") => {}
            "data_setvariableto"
            | "data_changevariableby"
            | "data_replaceitemoflist"
            | "motion_gotoxy"
            | "pen_penDown"
            | "pen_penUp" => {}
            _ => return false,
        }
        if let Some(next) = block.next.as_deref() {
            pending.push(next);
        }
    }
    true
}

pub(super) fn direct_warp_name(target_index: usize, frame_id: usize) -> String {
    format!("direct_warp_procedure_{target_index}_{frame_id}")
}

pub(super) fn numeric_direct_argument_expression(
    target: &ScratchTarget,
    input: Option<&serde_json::Value>,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
) -> Option<String> {
    match input.and_then(parse_input)? {
        InputValue::Number(_) => Some(expression(
            target,
            input,
            variables,
            lists,
            arguments,
            &mut HashSet::new(),
        )),
        InputValue::VariableReference(reference) => {
            let variable =
                super::super::project::variable_for_target(target, reference.id, variables)?;
            Some(format!("{}.number_or_nan()", variable.read_expression()))
        }
        InputValue::BlockReference(id) => {
            let block = target.blocks.get(id)?;
            match block.opcode.as_str() {
                "argument_reporter_string_number" | "argument_reporter_boolean" => {
                    let name = field_value(block, "VALUE")?.as_str()?;
                    let value = arguments.get(name)?;
                    if value.starts_with("script.numeric_argument(") {
                        Some(value.clone())
                    } else {
                        Some(format!("({value}).number_or_nan()"))
                    }
                }
                "data_variable" => {
                    let variable = variable_field(block, "VARIABLE", variables)?;
                    Some(format!("{}.number_or_nan()", variable.read_expression()))
                }
                "operator_add"
                | "operator_subtract"
                | "operator_multiply"
                | "operator_divide"
                | "operator_mod"
                | "operator_round"
                | "operator_random"
                | "operator_mathop"
                | "data_lengthoflist"
                | "data_itemnumoflist"
                | "math_number"
                | "math_integer"
                | "math_whole_number"
                | "math_positive_number"
                | "math_angle" => Some(expression(
                    target,
                    input,
                    variables,
                    lists,
                    arguments,
                    &mut HashSet::new(),
                )),
                _ => None,
            }
        }
        _ => None,
    }
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
        let Some(block) = target.blocks.get(id) else {
            eligible = false;
            break;
        };
        match block.opcode.as_str() {
            "control_if"
            | "control_if_else"
            | "control_repeat"
            | "control_repeat_until"
            | "control_while"
            | "control_forever" => {
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
            "control_stop"
                if field_value(block, "STOP_OPTION")
                    .and_then(Value::as_str)
                    .is_some_and(|option| matches!(option, "this script" | "all")) => {}
            "data_setvariableto"
            | "data_changevariableby"
            | "data_replaceitemoflist"
            | "motion_gotoxy"
            | "pen_penDown"
            | "pen_penUp" => {}
            // Stop is kept on the resumable path so it can propagate to the script scheduler.
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

fn direct_warp_bounded_eligible(
    target: &ScratchTarget,
    procedure: &ProcedureSignature,
    procedures: &[ProcedureSignature],
    memo: &mut HashMap<String, bool>,
    visiting: &mut HashSet<String>,
) -> bool {
    if let Some(eligible) = memo.get(&procedure.definition_id) {
        return *eligible;
    }
    if procedure.argument_names.len() != procedure.argument_ids.len()
        || procedure
            .argument_names
            .iter()
            .collect::<HashSet<_>>()
            .len()
            != procedure.argument_names.len()
        || !visiting.insert(procedure.definition_id.clone())
    {
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
            "procedures_call" => {
                let Some(callee) = procedure_for_call(block, procedures) else {
                    eligible = false;
                    break;
                };
                if !direct_warp_bounded_eligible(target, callee, procedures, memo, visiting) {
                    eligible = false;
                    break;
                }
            }
            "control_stop"
                if field_value(block, "STOP_OPTION")
                    .and_then(Value::as_str)
                    .is_some_and(|option| matches!(option, "this script" | "all")) => {}
            "data_setvariableto"
            | "data_changevariableby"
            | "data_replaceitemoflist"
            | "motion_gotoxy"
            | "pen_penDown"
            | "pen_penUp" => {}
            // Loops and suspension operations need resumable checkpoints.
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

pub(super) fn direct_warp_bounded_eligibility(
    target: &ScratchTarget,
    procedures: &[ProcedureSignature],
) -> HashMap<String, bool> {
    let mut memo = HashMap::new();
    for procedure in procedures {
        direct_warp_bounded_eligible(
            target,
            procedure,
            procedures,
            &mut memo,
            &mut HashSet::new(),
        );
    }
    memo
}

pub(super) fn direct_warp_status_procedures(
    target: &ScratchTarget,
    procedures: &[ProcedureSignature],
) -> HashSet<String> {
    let mut memo = HashMap::new();
    for procedure in procedures {
        direct_warp_returns_status(
            target,
            procedure,
            procedures,
            &mut memo,
            &mut HashSet::new(),
        );
    }
    memo.into_iter()
        .filter_map(|(definition_id, returns_status)| returns_status.then_some(definition_id))
        .collect()
}

fn direct_warp_returns_status(
    target: &ScratchTarget,
    procedure: &ProcedureSignature,
    procedures: &[ProcedureSignature],
    memo: &mut HashMap<String, bool>,
    visiting: &mut HashSet<String>,
) -> bool {
    if let Some(returns_status) = memo.get(&procedure.definition_id) {
        return *returns_status;
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
    let mut returns_status = false;
    while let Some(id) = pending.pop() {
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let Some(block) = target.blocks.get(id) else {
            continue;
        };
        match block.opcode.as_str() {
            "control_stop"
                if field_value(block, "STOP_OPTION")
                    .and_then(Value::as_str)
                    .is_some_and(|option| matches!(option, "this script" | "all")) =>
            {
                returns_status = true;
            }
            "procedures_call" => {
                if procedure_for_call(block, procedures).is_some_and(|callee| {
                    direct_warp_returns_status(target, callee, procedures, memo, visiting)
                }) {
                    returns_status = true;
                }
            }
            _ => {}
        }
        for input in ["SUBSTACK", "SUBSTACK2"] {
            if let Some(InputValue::BlockReference(branch)) =
                block.inputs.get(input).and_then(parse_input)
            {
                pending.push(branch);
            }
        }
        if let Some(next) = block.next.as_deref() {
            pending.push(next);
        }
    }

    visiting.remove(&procedure.definition_id);
    memo.insert(procedure.definition_id.clone(), returns_status);
    returns_status
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
        no_warp_timer,
        reachable_procedure_ids,
        numeric_call_procedure_ids,
        numeric_arguments,
        numeric_warp_procedure_ids,
    } = options;
    for procedure in procedures.iter().filter(|procedure| {
        reachable_procedure_ids.contains(&procedure.definition_id)
            && direct_eligible(target, procedure)
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
            target, procedure, variables, lists, procedures, &name, false, None,
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
            definition.next.as_deref(),
            variables,
            lists,
            &arguments,
            output,
            1,
        );
        line(output, 0, "}\n");
        if let Some(numeric) = numeric {
            output.push_str(&numeric);
        }
        if numeric_call_procedure_ids.contains(&procedure.definition_id) {
            output.push_str(
                &super::super::direct_numeric::generate_numeric_direct(
                    target,
                    procedure,
                    variables,
                    lists,
                    procedures,
                    &direct_name(target_index, procedure.frame_id),
                    true,
                    None,
                )
                .expect("numeric direct call analysis must yield a numeric procedure body"),
            );
        }
    }
    {
        let eligibility = direct_warp_eligibility(target, procedures);
        let bounded_eligibility = direct_warp_bounded_eligibility(target, procedures);
        let status_procedures = direct_warp_status_procedures(target, procedures);
        let emits_status_helper = procedures.iter().any(|procedure| {
            reachable_procedure_ids.contains(&procedure.definition_id)
                && status_procedures.contains(&procedure.definition_id)
                && !direct_eligible(target, procedure)
                && (bounded_eligibility
                    .get(&procedure.definition_id)
                    .copied()
                    .unwrap_or(false)
                    || (no_warp_timer
                        && eligibility
                            .get(&procedure.definition_id)
                            .copied()
                            .unwrap_or(false)))
        });
        if emits_status_helper && !output.contains("enum DirectWarpStatus") {
            line(
                output,
                0,
                "#[allow(dead_code)] #[derive(Clone, Copy, PartialEq, Eq)] enum DirectWarpStatus { Continue, ProcedureReturn, StopAll }",
            );
        }
        for procedure in procedures.iter().filter(|procedure| {
            let full_eligibility = no_warp_timer
                && eligibility
                    .get(&procedure.definition_id)
                    .copied()
                    .unwrap_or(false);
            let bounded = bounded_eligibility
                .get(&procedure.definition_id)
                .copied()
                .unwrap_or(false);
            reachable_procedure_ids.contains(&procedure.definition_id)
                && (full_eligibility || bounded)
                && !direct_eligible(target, procedure)
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
            let returns_status = status_procedures.contains(&procedure.definition_id);
            let proven_arguments = numeric_warp_procedure_ids
                .contains(&procedure.definition_id)
                .then(|| numeric_arguments.get(&procedure.definition_id))
                .flatten()
                .map(Vec::as_slice);
            let numeric = super::super::direct_numeric::generate_numeric_direct(
                target,
                procedure,
                variables,
                lists,
                procedures,
                &name,
                false,
                proven_arguments,
            );
            let needs_dynamic = numeric
                .as_ref()
                .is_none_or(|source| source.contains(&format!("{name}_dynamic")));
            if needs_dynamic {
                let body_name = if numeric.is_some() {
                    format!("{name}_dynamic")
                } else {
                    name.clone()
                };
                line(
                    output,
                    0,
                    "#[allow(dead_code, unused_variables, unreachable_code, unused_mut)]",
                );
                line(
                    output,
                    0,
                    &format!(
                        "fn {body_name}(state: &mut ScratchState, clock: &dyn Clock{params}){} {{",
                        if returns_status {
                            " -> DirectWarpStatus"
                        } else {
                            ""
                        }
                    ),
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
                    &numeric_warp_procedure_ids,
                    &numeric_arguments,
                    &status_procedures,
                    &arguments,
                    output,
                    1,
                    fast_math,
                );
                if returns_status {
                    line(output, 1, "DirectWarpStatus::Continue");
                }
                line(output, 0, "}\n");
            }
            if let Some(numeric) = numeric {
                output.push_str(&numeric);
            }
        }
    }
}

pub(super) fn generate_direct_chain<'a>(
    target: &'a ScratchTarget,
    mut id: Option<&'a str>,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
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
                    branch("SUBSTACK"),
                    variables,
                    lists,
                    arguments,
                    output,
                    indent + 1,
                );
                if block.opcode == "control_if_else" {
                    line(output, indent, "} else {");
                    generate_direct_chain(
                        target,
                        branch("SUBSTACK2"),
                        variables,
                        lists,
                        arguments,
                        output,
                        indent + 1,
                    );
                }
                line(output, indent, "}");
            }
            "control_stop" => line(output, indent, "return;"),
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
    numeric_warp_procedure_ids: &HashSet<String>,
    numeric_arguments: &HashMap<String, Vec<bool>>,
    status_procedures: &HashSet<String>,
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
                    numeric_warp_procedure_ids,
                    numeric_arguments,
                    status_procedures,
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
                        numeric_warp_procedure_ids,
                        numeric_arguments,
                        status_procedures,
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
                                numeric_warp_procedure_ids,
                                numeric_arguments,
                                status_procedures,
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
                                numeric_warp_procedure_ids,
                                numeric_arguments,
                                status_procedures,
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
                                numeric_warp_procedure_ids,
                                numeric_arguments,
                                status_procedures,
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
                let proven_arguments = numeric_warp_procedure_ids
                    .contains(&callee.definition_id)
                    .then(|| numeric_arguments.get(&callee.definition_id))
                    .flatten();
                let values = callee
                    .argument_ids
                    .iter()
                    .enumerate()
                    .map(|(index, argument_id)| {
                        if proven_arguments
                            .and_then(|mask| mask.get(index))
                            .copied()
                            .unwrap_or(false)
                        {
                            let value = numeric_direct_argument_expression(
                                target,
                                block.inputs.get(argument_id),
                                variables,
                                lists,
                                arguments,
                            )
                            .expect("the direct numeric call was proven numeric");
                            line(
                                output,
                                indent,
                                &format!("let direct_warp_arg_{index} = {value};"),
                            );
                            format!("direct_warp_arg_{index}")
                        } else {
                            let value = expression(
                                target,
                                block.inputs.get(argument_id),
                                variables,
                                lists,
                                arguments,
                                &mut HashSet::new(),
                            );
                            format!("({value}).to_owned().into()")
                        }
                    })
                    .collect::<Vec<_>>();
                let args = if values.is_empty() {
                    String::new()
                } else {
                    format!(", {}", values.join(", "))
                };
                let callee_name = if direct_eligible(target, callee) {
                    direct_name(target_index, callee.frame_id)
                } else {
                    direct_warp_name(target_index, callee.frame_id)
                };
                if !direct_eligible(target, callee)
                    && status_procedures.contains(&callee.definition_id)
                {
                    line(
                        output,
                        indent,
                        &format!("match {callee_name}(state, clock{args}) {{"),
                    );
                    line(
                        output,
                        indent + 1,
                        "DirectWarpStatus::Continue | DirectWarpStatus::ProcedureReturn => {},",
                    );
                    line(
                        output,
                        indent + 1,
                        "DirectWarpStatus::StopAll => return DirectWarpStatus::StopAll,",
                    );
                    line(output, indent, "}");
                } else {
                    line(
                        output,
                        indent,
                        &format!("{callee_name}(state, clock{args});"),
                    );
                }
            }
            "control_stop" => match field_value(block, "STOP_OPTION").and_then(Value::as_str) {
                Some("this script") => {
                    line(output, indent, "return DirectWarpStatus::ProcedureReturn;")
                }
                Some("all") => line(output, indent, "return DirectWarpStatus::StopAll;"),
                _ => line(
                    output,
                    indent,
                    "unreachable!(\"unsupported stop option in bounded direct warp\");",
                ),
            },
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
                "let mut scratch_local_{slot} = scratch_to_number({});",
                variable_read_for_slot(variables, *slot)
            ),
        );
    }
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
