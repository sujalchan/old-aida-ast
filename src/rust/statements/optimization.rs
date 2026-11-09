use super::*;

pub(super) struct FusedRepeatPlan {
    pub(super) body: Vec<String>,
    pub(super) blocks: Vec<String>,
    pub(super) local_random: bool,
}

// fuse synchronous repeat bodies to avoid one program-counter dispatch per block.
pub(super) fn fused_repeat_plan(
    target: &ScratchTarget,
    block: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
) -> Option<FusedRepeatPlan> {
    if block.opcode != "control_repeat" {
        return None;
    }
    let mut body = Vec::new();
    let mut blocks = Vec::new();
    let mut seen = HashSet::new();
    let mut current = block
        .inputs
        .get("SUBSTACK")
        .and_then(parse_input)
        .and_then(|input| match input {
            InputValue::BlockReference(id) => Some(id.to_owned()),
            _ => None,
        });
    while let Some(id) = current {
        if !seen.insert(id.clone()) {
            return None;
        }
        let statement = target.blocks.get(&id)?;
        match statement.opcode.as_str() {
            "data_setvariableto" | "data_changevariableby" | "data_replaceitemoflist" => {}
            "control_if" => {
                if !fused_repeat_branch(target, statement)? {
                    return None;
                }
                let mut branch = statement
                    .inputs
                    .get("SUBSTACK")
                    .and_then(parse_input)
                    .and_then(|input| match input {
                        InputValue::BlockReference(id) => Some(id.to_owned()),
                        _ => None,
                    });
                while let Some(branch_id) = branch {
                    if !seen.insert(branch_id.clone()) {
                        return None;
                    }
                    let branch_block = target.blocks.get(&branch_id)?;
                    blocks.push(branch_id);
                    branch = branch_block.next.clone();
                }
            }
            _ => return None,
        }
        blocks.push(id.clone());
        body.push(id.clone());
        current = statement.next.clone();
    }
    if body.is_empty() {
        return None;
    }
    let local_random = blocks.iter().any(|id| {
        target.blocks.get(id).is_some_and(|statement| {
            statement.inputs.values().any(|input| {
                reporter_contains_opcode(target, input, "operator_random", &mut HashSet::new())
            })
        })
    });
    if local_random
        && blocks.iter().any(|id| {
            target
                .blocks
                .get(id)
                .is_some_and(|statement| !fused_random_inputs_safe(target, statement, variables))
        })
    {
        return None;
    }
    if local_random && !fused_random_assignment_order_safe(target, &body, variables) {
        return None;
    }
    Some(FusedRepeatPlan {
        body,
        blocks,
        local_random,
    })
}

// ensure random-borrowed list operations see values written numerically earlier in the loop.
pub(super) fn fused_random_assignment_order_safe(
    target: &ScratchTarget,
    body: &[String],
    variables: &HashMap<String, VariableInfo>,
) -> bool {
    let mut numeric_slots = HashSet::new();
    for id in body {
        let Some(statement) = target.blocks.get(id) else {
            return false;
        };
        match statement.opcode.as_str() {
            "data_setvariableto" => {
                let Some(variable) = variable_field(statement, "VARIABLE", variables) else {
                    return false;
                };
                let Some(value) = statement.inputs.get("VALUE") else {
                    return false;
                };
                if variable.numeric
                    && reporter_contains_opcode(
                        target,
                        value,
                        "operator_random",
                        &mut HashSet::new(),
                    )
                {
                    numeric_slots.insert(variable.slot);
                }
            }
            "data_changevariableby" => {
                let Some(variable) = variable_field(statement, "VARIABLE", variables) else {
                    return false;
                };
                numeric_slots.insert(variable.slot);
            }
            "data_replaceitemoflist" => {
                for input in ["INDEX", "ITEM"] {
                    let Some(variable) =
                        variable_input_info(target, statement.inputs.get(input), variables)
                    else {
                        return false;
                    };
                    if !variable.numeric || !numeric_slots.contains(&variable.slot) {
                        return false;
                    }
                }
            }
            "control_if" => {}
            _ => return false,
        }
    }
    true
}

// locate reporter opcodes through connected inputs without changing their evaluation order.
pub(super) fn reporter_contains_opcode(
    target: &ScratchTarget,
    input: &Value,
    opcode: &str,
    active: &mut HashSet<String>,
) -> bool {
    let Some(InputValue::BlockReference(id)) = parse_input(input) else {
        return false;
    };
    if !active.insert(id.to_owned()) {
        return false;
    }
    let found = target.blocks.get(id).is_some_and(|block| {
        block.opcode == opcode
            || block
                .inputs
                .values()
                .any(|nested| reporter_contains_opcode(target, nested, opcode, active))
    });
    active.remove(id);
    found
}

// keep local random borrowing away from list selectors that can draw randomness internally.
pub(super) fn fused_random_inputs_safe(
    target: &ScratchTarget,
    statement: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
) -> bool {
    if statement.opcode == "data_replaceitemoflist"
        && !variable_input_info(target, statement.inputs.get("INDEX"), variables)
            .zip(variable_input_info(
                target,
                statement.inputs.get("ITEM"),
                variables,
            ))
            .is_some_and(|(index, item)| index.numeric && item.numeric)
    {
        return false;
    }
    for input in statement.inputs.values() {
        if !fused_random_input_safe(target, input, variables, &mut HashSet::new()) {
            return false;
        }
    }
    true
}

pub(super) fn fused_random_input_safe(
    target: &ScratchTarget,
    input: &Value,
    variables: &HashMap<String, VariableInfo>,
    active: &mut HashSet<String>,
) -> bool {
    let Some(InputValue::BlockReference(id)) = parse_input(input) else {
        return true;
    };
    if !active.insert(id.to_owned()) {
        return false;
    }
    let safe = target.blocks.get(id).is_some_and(|block| {
        let list_index_is_dynamic = block.opcode == "data_itemoflist"
            && !variable_input_info(target, block.inputs.get("INDEX"), variables)
                .is_some_and(|index| index.numeric);
        !list_index_is_dynamic
            && block
                .inputs
                .values()
                .all(|nested| fused_random_input_safe(target, nested, variables, active))
    });
    active.remove(id);
    safe
}

// permit only non-suspending display and stop blocks in an inline conditional branch.
pub(super) fn fused_repeat_branch(target: &ScratchTarget, block: &ScratchBlock) -> Option<bool> {
    if !block.inputs.contains_key("CONDITION") {
        return Some(false);
    }
    let mut current = block
        .inputs
        .get("SUBSTACK")
        .and_then(parse_input)
        .and_then(|input| match input {
            InputValue::BlockReference(id) => Some(id),
            _ => None,
        });
    while let Some(id) = current {
        let nested = target.blocks.get(id)?;
        match nested.opcode.as_str() {
            "looks_say" | "looks_think" => {}
            "control_stop"
                if field_value(nested, "STOP_OPTION").and_then(Value::as_str) == Some("all") => {}
            _ => return Some(false),
        }
        current = nested.next.as_deref();
    }
    Some(true)
}

pub(super) struct FusedReplacementCompare {
    pub(super) expected: VariableInfo,
    pub(super) negated: bool,
}

// fuse an immediate list read/equality with its preceding replacement without caching list state.
pub(super) fn fused_replacement_compare(
    target: &ScratchTarget,
    replacement: &ScratchBlock,
    condition: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
) -> Option<FusedReplacementCompare> {
    let condition = condition
        .inputs
        .get("CONDITION")
        .and_then(parse_input)
        .and_then(|input| match input {
            InputValue::BlockReference(id) => target.blocks.get(id),
            _ => None,
        })?;
    let (comparison, negated) = if condition.opcode == "operator_not" {
        let comparison = condition
            .inputs
            .get("OPERAND")
            .and_then(parse_input)
            .and_then(|input| match input {
                InputValue::BlockReference(id) => target.blocks.get(id),
                _ => None,
            })?;
        (comparison, true)
    } else {
        (condition, false)
    };
    if comparison.opcode != "operator_equals" {
        return None;
    }
    let replaced_list = list_field(replacement, "LIST", lists)?;
    let replaced_index = variable_input_info(target, replacement.inputs.get("INDEX"), variables)?;
    for (item_input, expected_input) in [("OPERAND1", "OPERAND2"), ("OPERAND2", "OPERAND1")] {
        let InputValue::BlockReference(id) = parse_input(comparison.inputs.get(item_input)?)?
        else {
            continue;
        };
        let item_reporter = target.blocks.get(id)?;
        if item_reporter.opcode != "data_itemoflist"
            || list_field(item_reporter, "LIST", lists)?.slot != replaced_list.slot
        {
            continue;
        }
        let item_index = variable_input_info(target, item_reporter.inputs.get("INDEX"), variables)?;
        let expected =
            variable_input_info(target, comparison.inputs.get(expected_input), variables)?;
        if replaced_index.slot == item_index.slot
            && replaced_index.numeric
            && item_index.numeric
            && expected.numeric
        {
            return Some(FusedReplacementCompare { expected, negated });
        }
    }
    None
}

// Only batch a closed synchronous body: two constant-bound random writes,
// an immediate replacement/read comparison, a literal-only branch, and a
// constant variable increment. No operation in this body can inspect the
// temporarily borrowed list through ScratchState or suspend the script.
pub(super) struct NumericReplacementBatch {
    pub(super) list_slot: usize,
    pub(super) index_slot: usize,
    pub(super) item_slot: usize,
    pub(super) expected_slot: usize,
}

pub(super) fn numeric_replacement_batch(
    target: &ScratchTarget,
    plan: &FusedRepeatPlan,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
) -> Option<NumericReplacementBatch> {
    if !plan.local_random || plan.body.len() != 5 {
        return None;
    }
    let blocks: Vec<_> = plan.body.iter().map(|id| &target.blocks[id]).collect();
    if blocks[0].opcode != "data_setvariableto"
        || blocks[1].opcode != "data_setvariableto"
        || blocks[2].opcode != "data_replaceitemoflist"
        || blocks[3].opcode != "control_if"
        || blocks[4].opcode != "data_changevariableby"
    {
        return None;
    }
    let mut assigned = HashSet::new();
    for block in &blocks[..2] {
        let variable = variable_field(block, "VARIABLE", variables)?;
        let InputValue::BlockReference(id) = parse_input(block.inputs.get("VALUE")?)? else {
            return None;
        };
        if !variable.numeric
            || !assigned.insert(variable.slot)
            || constant_random_bounds(target, target.blocks.get(id)?).is_none()
        {
            return None;
        }
    }
    constant_numeric_input(target, blocks[4].inputs.get("VALUE"), &mut HashSet::new())?;
    variable_field(blocks[4], "VARIABLE", variables)?;
    let list = list_field(blocks[2], "LIST", lists)?;
    let index = variable_input_info(target, blocks[2].inputs.get("INDEX"), variables)?;
    let item = variable_input_info(target, blocks[2].inputs.get("ITEM"), variables)?;
    let compare = fused_replacement_compare(target, blocks[2], blocks[3], variables, lists)?;
    if !list.is_stage
        || !assigned.contains(&index.slot)
        || !assigned.contains(&item.slot)
        || !assigned.contains(&compare.expected.slot)
    {
        return None;
    }
    let mut branch = blocks[3]
        .inputs
        .get("SUBSTACK")
        .and_then(parse_input)
        .and_then(|input| match input {
            InputValue::BlockReference(id) => Some(id),
            _ => None,
        });
    while let Some(id) = branch {
        let nested = target.blocks.get(id)?;
        if matches!(nested.opcode.as_str(), "looks_say" | "looks_think")
            && !matches!(
                nested.inputs.get("MESSAGE").and_then(parse_input),
                Some(InputValue::String(_) | InputValue::Number(_))
            )
        {
            return None;
        }
        branch = nested.next.as_deref();
    }
    Some(NumericReplacementBatch {
        list_slot: list.slot,
        index_slot: index.slot,
        item_slot: item.slot,
        expected_slot: compare.expected.slot,
    })
}

pub(super) fn generate_fused_repeat_loop(
    context: StatementContext<'_>,
    id: &str,
    block: &ScratchBlock,
    graph: &ScriptGraph,
    plan: &FusedRepeatPlan,
    no_warp_timer: bool,
    output: &mut String,
) {
    let StatementContext {
        target,
        variables,
        lists,
        arguments,
    } = context;
    let pc = graph.block_pcs[id];
    let next_pc = block
        .next
        .as_ref()
        .and_then(|next| graph.block_pcs.get(next).copied())
        .unwrap_or(graph.fallthrough_pcs[id]);
    let repeat_slot = graph.repeat_slots[id];
    let times = expression(
        target,
        block.inputs.get("TIMES"),
        variables,
        lists,
        arguments,
        &mut HashSet::new(),
    );

    line(output, 4, &format!("{pc} => {{"));
    line(
        output,
        5,
        "let scratch_warp_mode = script.frames.last().is_some_and(|frame| frame.warp_mode);",
    );
    if !no_warp_timer {
        line(
            output,
            5,
            "if !scratch_warp_mode { script.warp_budget.reset(); }",
        );
    }
    line(
        output,
        5,
        &format!(
            "let mut repeat_remaining = script.frames.last().unwrap().repeat_counts[{repeat_slot}];"
        ),
    );
    line(output, 5, "if repeat_remaining == 0 {");
    line(
        output,
        6,
        &format!("repeat_remaining = scratch_round({times}).max(0.0) as usize;"),
    );
    line(
        output,
        6,
        &format!(
            "script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = repeat_remaining;"
        ),
    );
    line(output, 5, "}");
    line(output, 5, "if repeat_remaining == 0 {");
    line(
        output,
        6,
        &format!(
            "script.frames.last_mut().unwrap().pc = {};",
            pc_literal(next_pc)
        ),
    );
    line(output, 6, "continue;");
    line(output, 5, "}");
    let batch = numeric_replacement_batch(target, plan, variables, lists);
    if plan.local_random {
        if let Some(batch) = &batch {
            line(
                output,
                5,
                &format!(
                    "let scratch_fused_status = state.with_stage_numeric_list_batch({}, |state, scratch_batch| {{ with_scratch_random(|loop_random| {{",
                    batch.list_slot
                ),
            );
        } else {
            line(
                output,
                5,
                "let scratch_fused_status = with_scratch_random(|loop_random| {",
            );
        }
        line(output, 6, "loop {");
    } else {
        line(output, 5, "loop {");
    }
    let loop_indent = if plan.local_random { 6 } else { 5 };
    let body_indent = loop_indent + 1;
    for (step_index, statement_id) in plan.body.iter().enumerate() {
        let statement = &target.blocks[statement_id];
        let following_compare = statement
            .next
            .as_ref()
            .and_then(|next| target.blocks.get(next))
            .filter(|next| next.opcode == "control_if")
            .and_then(|next| fused_replacement_compare(target, statement, next, variables, lists));
        let previous_compare = step_index
            .checked_sub(1)
            .and_then(|previous| target.blocks.get(&plan.body[previous]))
            .filter(|previous| previous.opcode == "data_replaceitemoflist")
            .and_then(|previous| {
                fused_replacement_compare(target, previous, statement, variables, lists)
            });
        if let Some(batch) = &batch {
            if step_index < 2 {
                let variable = variable_field(statement, "VARIABLE", variables).unwrap();
                let value = fused_random_expression(
                    expression(
                        target,
                        statement.inputs.get("VALUE"),
                        variables,
                        lists,
                        arguments,
                        &mut HashSet::new(),
                    ),
                    true,
                );
                let setter = if variable.is_stage {
                    "set_shared_number"
                } else {
                    "set_existing_local_number"
                };
                line(
                    output,
                    body_indent,
                    &format!(
                        "let scratch_batch_value_{} = {value}; state.{setter}({}, scratch_batch_value_{});",
                        variable.slot, variable.slot, variable.slot
                    ),
                );
                continue;
            }
            if step_index == 2 {
                line(
                    output,
                    body_indent,
                    &format!(
                        "let (scratch_list_changed, scratch_list_replace_matches) = scratch_batch.replace_number_and_equals(scratch_batch_value_{}, scratch_batch_value_{}, scratch_batch_value_{}); state.list_changed_result({}, scratch_list_changed);",
                        batch.index_slot, batch.item_slot, batch.expected_slot, batch.list_slot
                    ),
                );
                continue;
            }
        }
        match statement.opcode.as_str() {
            "data_setvariableto" => generate_fused_set_variable(
                StatementContext {
                    target,
                    variables,
                    lists,
                    arguments,
                },
                statement,
                plan.local_random,
                output,
                body_indent,
            ),
            "data_changevariableby" => generate_fused_change_variable(
                StatementContext {
                    target,
                    variables,
                    lists,
                    arguments,
                },
                statement,
                plan.local_random,
                output,
                body_indent,
            ),
            "data_replaceitemoflist" => generate_fused_replace_item(
                StatementContext {
                    target,
                    variables,
                    lists,
                    arguments,
                },
                statement,
                plan.local_random,
                following_compare.as_ref(),
                output,
                body_indent,
            ),
            "control_if" => generate_fused_if(
                StatementContext {
                    target,
                    variables,
                    lists,
                    arguments,
                },
                statement,
                plan.local_random,
                previous_compare.as_ref(),
                output,
                body_indent,
            ),
            _ => unreachable!("fused repeat plan only contains validated statements"),
        }
    }
    line(output, body_indent, "repeat_remaining -= 1;");
    line(output, body_indent, "if repeat_remaining == 0 {");
    line(
        output,
        body_indent + 1,
        &format!("script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = 0;"),
    );
    line(
        output,
        body_indent + 1,
        &format!(
            "script.frames.last_mut().unwrap().pc = {};",
            pc_literal(next_pc)
        ),
    );
    if no_warp_timer {
        line(
            output,
            body_indent + 1,
            if plan.local_random {
                "if !scratch_warp_mode { return Some(ScriptStatus::Yield); }"
            } else {
                "if !scratch_warp_mode { return ScriptStatus::Yield; }"
            },
        );
    } else {
        line(
            output,
            body_indent + 1,
            if plan.local_random {
                "if script.yield_at_safe_point(clock) { return Some(ScriptStatus::Yield); }"
            } else {
                "if script.yield_at_safe_point(clock) { return ScriptStatus::Yield; }"
            },
        );
    }
    line(output, body_indent + 1, "break;");
    line(output, body_indent, "}");
    line(
        output,
        body_indent,
        if no_warp_timer {
            "if !scratch_warp_mode {"
        } else {
            "if script.yield_at_safe_point(clock) {"
        },
    );
    line(
        output,
        body_indent + 1,
        &format!(
            "script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = repeat_remaining;"
        ),
    );
    line(
        output,
        body_indent + 1,
        &format!("script.frames.last_mut().unwrap().pc = {pc};"),
    );
    line(
        output,
        body_indent + 1,
        if plan.local_random {
            "return Some(ScriptStatus::Yield);"
        } else {
            "return ScriptStatus::Yield;"
        },
    );
    line(output, body_indent, "}");
    line(output, loop_indent, "}");
    if plan.local_random {
        line(output, 6, "None");
        line(output, 5, if batch.is_some() { "}) });" } else { "});" });
        line(
            output,
            5,
            "if let Some(scratch_fused_status) = scratch_fused_status { return scratch_fused_status; }",
        );
    }
    line(output, 5, "continue;");
    line(output, 4, "},");
}

pub(super) fn generate_fused_set_variable(
    context: StatementContext<'_>,
    block: &ScratchBlock,
    local_random: bool,
    output: &mut String,
    indent: usize,
) {
    let StatementContext {
        target,
        variables,
        lists,
        arguments,
    } = context;
    if let Some(variable) = variable_field(block, "VARIABLE", variables) {
        let input = block.inputs.get("VALUE");
        let value = fused_random_expression(
            expression(
                target,
                input,
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            ),
            local_random,
        );
        if variable.numeric {
            let setter = if variable.is_stage {
                "set_shared_number"
            } else {
                "set_local_number"
            };
            if let Some(source) =
                variable_input_info(target, input, variables).filter(|source| source.numeric)
            {
                let source = source.read_expression();
                line(
                    output,
                    indent,
                    &format!(
                        "if let ScratchValue::Number(scratch_numeric_value) = {source} {{ state.{setter}({}, *scratch_numeric_value); }} else {{ let scratch_new_value: ScratchValue = ({source}).clone(); {} }}",
                        variable.slot,
                        variable.write_statement("scratch_new_value")
                    ),
                );
            } else {
                line(
                    output,
                    indent,
                    &format!("state.{setter}({}, {value});", variable.slot),
                );
            }
        } else if let Some(source) = variable_input(target, input, variables, lists, arguments) {
            line(
                output,
                indent,
                &format!("let scratch_new_value: ScratchValue = ({source}).clone();"),
            );
            line(
                output,
                indent,
                &variable.write_statement("scratch_new_value"),
            );
        } else {
            line(
                output,
                indent,
                &format!("let scratch_new_value: ScratchValue = ({value}).into();"),
            );
            line(
                output,
                indent,
                &variable.write_statement("scratch_new_value"),
            );
        }
    }
}

pub(super) fn generate_fused_change_variable(
    context: StatementContext<'_>,
    block: &ScratchBlock,
    local_random: bool,
    output: &mut String,
    indent: usize,
) {
    let StatementContext {
        target,
        variables,
        lists,
        arguments,
    } = context;
    if let Some(variable) = variable_field(block, "VARIABLE", variables) {
        let value = fused_random_expression(
            expression(
                target,
                block.inputs.get("VALUE"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            ),
            local_random,
        );
        let method = if variable.is_stage {
            "change_shared_number"
        } else {
            "change_local_number"
        };
        line(
            output,
            indent,
            &format!(
                "state.{method}({}, scratch_to_number({value}));",
                variable.slot
            ),
        );
    }
}

pub(super) fn generate_fused_replace_item(
    context: StatementContext<'_>,
    block: &ScratchBlock,
    local_random: bool,
    compare: Option<&FusedReplacementCompare>,
    output: &mut String,
    indent: usize,
) {
    let StatementContext {
        target,
        variables,
        lists,
        arguments,
    } = context;
    if let Some(list) = list_field(block, "LIST", lists) {
        let index_input = block.inputs.get("INDEX");
        let item_input = block.inputs.get("ITEM");
        if let Some(compare) = compare {
            let index = variable_input_info(target, index_input, variables)
                .expect("fused replacement index was validated");
            let item = variable_input_info(target, item_input, variables)
                .expect("fused replacement value was validated");
            line(
                output,
                indent,
                &format!(
                    "let scratch_list_replace_inputs = match (state.get({}), state.get({}), state.get({})) {{ (ScratchValue::Number(scratch_index), ScratchValue::Number(scratch_item), ScratchValue::Number(scratch_expected)) => Some((*scratch_index, *scratch_item, *scratch_expected)), _ => None }}; let scratch_list_replace_result = scratch_list_replace_inputs.map(|(scratch_index, scratch_item, scratch_expected)| state.list_mut({}).replace_number_and_equals(scratch_index, scratch_item, scratch_expected));",
                    index.slot, item.slot, compare.expected.slot, list.slot
                ),
            );
            line(
                output,
                indent,
                &format!(
                    "let (scratch_list_changed, scratch_list_replace_matches) = scratch_list_replace_result.unwrap_or_else(|| {{ let scratch_list_index: ScratchValue = ({}).clone(); let scratch_list_item: ScratchValue = ({}).clone(); let scratch_expected: ScratchValue = ({}).clone(); let scratch_list_changed = state.list_mut({}).replace(scratch_list_index.clone(), scratch_list_item); let scratch_list_replace_matches = scratch_equals({}.get(&scratch_list_index), &scratch_expected); (scratch_list_changed, scratch_list_replace_matches) }});",
                    index.read_expression(),
                    item.read_expression(),
                    compare.expected.read_expression(),
                    list.slot,
                    list.read_expression()
                ),
            );
            line(
                output,
                indent,
                &format!(
                    "state.list_changed_result({}, scratch_list_changed);",
                    list.slot
                ),
            );
            return;
        }
        if let Some((index, item)) = variable_input_info(target, index_input, variables)
            .zip(variable_input_info(target, item_input, variables))
            .filter(|(index, item)| index.numeric && item.numeric)
        {
            line(
                output,
                indent,
                &format!(
                    "let scratch_list_numeric_pair = match ({}, {}) {{ (ScratchValue::Number(scratch_list_index), ScratchValue::Number(scratch_list_item)) => Some((*scratch_list_index, *scratch_list_item)), _ => None }};",
                    index.read_expression(),
                    item.read_expression()
                ),
            );
            line(
                output,
                indent,
                &format!(
                    "let scratch_list_changed = if let Some((scratch_list_index, scratch_list_item)) = scratch_list_numeric_pair {{ state.list_mut({}).replace_number(scratch_list_index, scratch_list_item) }} else {{ let scratch_list_index: ScratchValue = ({}).clone(); let scratch_list_item: ScratchValue = ({}).clone(); state.list_mut({}).replace(scratch_list_index, scratch_list_item) }};",
                    list.slot,
                    index.read_expression(),
                    item.read_expression(),
                    list.slot
                ),
            );
        } else {
            let item = fused_random_expression(
                owned_input(target, item_input, variables, lists, arguments),
                local_random,
            );
            let index = fused_random_expression(
                owned_input(target, index_input, variables, lists, arguments),
                local_random,
            );
            line(
                output,
                indent,
                &format!("let scratch_list_index: ScratchValue = {index};"),
            );
            line(
                output,
                indent,
                &format!("let scratch_list_item: ScratchValue = {item};"),
            );
            line(
                output,
                indent,
                &format!(
                    "let scratch_list_changed = state.list_mut({}).replace(scratch_list_index, scratch_list_item);",
                    list.slot
                ),
            );
        }
        line(
            output,
            indent,
            &format!(
                "state.list_changed_result({}, scratch_list_changed);",
                list.slot
            ),
        );
    }
}

pub(super) fn generate_fused_if(
    context: StatementContext<'_>,
    block: &ScratchBlock,
    local_random: bool,
    compare: Option<&FusedReplacementCompare>,
    output: &mut String,
    indent: usize,
) {
    let StatementContext {
        target,
        variables,
        lists,
        arguments,
    } = context;
    let condition = compare.map_or_else(
        || {
            fused_random_expression(
                expression(
                    target,
                    block.inputs.get("CONDITION"),
                    variables,
                    lists,
                    arguments,
                    &mut HashSet::new(),
                ),
                local_random,
            )
        },
        |compare| {
            if compare.negated {
                "!scratch_list_replace_matches".to_owned()
            } else {
                "scratch_list_replace_matches".to_owned()
            }
        },
    );
    line(output, indent, &format!("if scratch_bool({condition}) {{"));
    let mut current = block
        .inputs
        .get("SUBSTACK")
        .and_then(parse_input)
        .and_then(|input| match input {
            InputValue::BlockReference(id) => Some(id),
            _ => None,
        });
    while let Some(id) = current {
        let nested = &target.blocks[id];
        match nested.opcode.as_str() {
            "looks_say" | "looks_think" => {
                let value = fused_random_expression(
                    expression(
                        target,
                        nested.inputs.get("MESSAGE"),
                        variables,
                        lists,
                        arguments,
                        &mut HashSet::new(),
                    ),
                    local_random,
                );
                line(
                    output,
                    indent + 1,
                    &format!("let scratch_bubble_text = scratch_string({value});"),
                );
                if nested.opcode == "looks_say" {
                    let name = say_variable_name(target, nested.inputs.get("MESSAGE"))
                        .unwrap_or_else(|| "say".to_owned());
                    line(
                        output,
                        indent + 1,
                        &format!(
                            "print_value!(state, {}, scratch_bubble_text.clone());",
                            quote(&name)
                        ),
                    );
                }
                line(
                    output,
                    indent + 1,
                    &format!(
                        "state.set_speech_bubble(BubbleKind::{}, scratch_bubble_text, None);",
                        if nested.opcode == "looks_say" {
                            "Say"
                        } else {
                            "Think"
                        }
                    ),
                );
            }
            "control_stop" => match field_value(nested, "STOP_OPTION").and_then(Value::as_str) {
                Some("all") => line(
                    output,
                    indent + 1,
                    if local_random {
                        "return Some(ScriptStatus::StopAll);"
                    } else {
                        "return ScriptStatus::StopAll;"
                    },
                ),
                _ => unreachable!("fused branch validated the stop option"),
            },
            _ => unreachable!("fused branch validated the statement opcode"),
        }
        current = nested.next.as_deref();
    }
    line(output, indent, "}");
}

// bind Scratch random reporters to the loop's existing PRNG borrow when safe.
pub(super) fn fused_random_expression(expression: String, local_random: bool) -> String {
    if local_random {
        expression.replace(
            "scratch_random_constant(",
            "loop_random.between_with_integer_bounds(",
        )
    } else {
        expression
    }
}

pub(super) struct NumericLoopPlan {
    pub(super) body: Vec<String>,
    pub(super) local_slots: Vec<usize>,
    pub(super) written_slots: Vec<usize>,
}

pub(super) fn numeric_loop_plan(
    target: &ScratchTarget,
    block: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
) -> Option<NumericLoopPlan> {
    if !matches!(
        block.opcode.as_str(),
        "control_repeat" | "control_repeat_until" | "control_while" | "control_forever"
    ) {
        return None;
    }
    let mut body = Vec::new();
    let mut current = block
        .inputs
        .get("SUBSTACK")
        .and_then(parse_input)
        .and_then(|input| match input {
            InputValue::BlockReference(id) => Some(id),
            _ => None,
        });
    let mut seen = HashSet::new();
    let mut local_slots = HashSet::new();
    let mut written_slots = BTreeSet::new();
    while let Some(id) = current {
        if !seen.insert(id.to_owned()) {
            return None;
        }
        let statement = target.blocks.get(id)?;
        if !matches!(
            statement.opcode.as_str(),
            "data_setvariableto" | "data_changevariableby"
        ) {
            return None;
        }
        let variable = variable_field(statement, "VARIABLE", variables)?;
        if !variable.numeric {
            return None;
        }
        written_slots.insert(variable.slot);
        local_slots.insert(variable.slot);
        if !collect_numeric_local_slots(
            target,
            statement.inputs.get("VALUE"),
            variables,
            &mut local_slots,
            &mut HashSet::new(),
        ) {
            return None;
        }
        body.push(id.to_owned());
        current = statement.next.as_deref();
    }
    if body.is_empty() {
        return None;
    }

    let loop_input = match block.opcode.as_str() {
        "control_repeat" => block.inputs.get("TIMES"),
        "control_repeat_until" | "control_while" => block.inputs.get("CONDITION"),
        "control_forever" => None,
        _ => unreachable!(),
    };
    if loop_input.is_some()
        && !collect_numeric_local_slots(
            target,
            loop_input,
            variables,
            &mut local_slots,
            &mut HashSet::new(),
        )
    {
        return None;
    }

    let mut local_slots = local_slots.into_iter().collect::<Vec<_>>();
    local_slots.sort_unstable();
    Some(NumericLoopPlan {
        body,
        local_slots,
        written_slots: written_slots.into_iter().collect(),
    })
}

pub(super) fn generate_native_numeric_loop(
    context: NumericLoopContext<'_>,
    id: &str,
    block: &ScratchBlock,
    plan: &NumericLoopPlan,
    output: &mut String,
) {
    let NumericLoopContext {
        target,
        graph,
        variables,
        fast_math,
        no_warp_timer,
    } = context;
    let pc = graph.block_pcs[id];
    let next_pc = block
        .next
        .as_ref()
        .and_then(|next| graph.block_pcs.get(next).copied())
        .unwrap_or(graph.fallthrough_pcs[id]);
    let locals = plan
        .local_slots
        .iter()
        .map(|slot| (*slot, format!("scratch_local_{slot}")))
        .collect::<HashMap<_, _>>();

    line(output, 4, &format!("{pc} => {{"));
    for slot in &plan.local_slots {
        line(
            output,
            5,
            &format!(
                "let mut scratch_local_{slot} = scratch_to_number({});",
                variable_read_for_slot(variables, *slot)
            ),
        );
    }
    line(
        output,
        5,
        "let scratch_warp_mode = script.frames.last().is_some_and(|frame| frame.warp_mode);",
    );
    if !no_warp_timer {
        line(
            output,
            5,
            "if !scratch_warp_mode { script.warp_budget.reset(); }",
        );
    }

    if block.opcode == "control_repeat" {
        generate_native_repeat_loop(context, id, block, plan, &locals, next_pc, output);
    } else {
        line(output, 5, "loop {");
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
            let exit_condition = if block.opcode == "control_repeat_until" {
                condition
            } else {
                format!("!({condition})")
            };
            line(output, 6, &format!("if {exit_condition} {{ break; }}"));
        }
        generate_native_numeric_body(target, variables, plan, &locals, fast_math, output, 6);
        line(
            output,
            6,
            if no_warp_timer {
                "if !scratch_warp_mode {"
            } else {
                "if !scratch_warp_mode || script.warp_budget.expired(clock) {"
            },
        );
        line(
            output,
            7,
            &format!("script.frames.last_mut().unwrap().pc = {pc};"),
        );
        generate_local_commits(plan, variables, output, 7);
        line(output, 7, "return ScriptStatus::Yield;");
        line(output, 6, "}");
        line(output, 5, "}");
        generate_local_commits(plan, variables, output, 5);
        line(
            output,
            5,
            &format!(
                "script.frames.last_mut().unwrap().pc = {};",
                pc_literal(next_pc)
            ),
        );
        line(output, 5, "continue;");
    }
    line(output, 4, "},");
}

pub(super) fn generate_native_repeat_loop(
    context: NumericLoopContext<'_>,
    id: &str,
    block: &ScratchBlock,
    plan: &NumericLoopPlan,
    locals: &HashMap<usize, String>,
    next_pc: usize,
    output: &mut String,
) {
    let NumericLoopContext {
        target,
        graph,
        variables,
        fast_math,
        no_warp_timer,
    } = context;
    let pc = graph.block_pcs[id];
    let slot = graph.repeat_slots[id];
    let times = local_expression(
        target,
        block.inputs.get("TIMES"),
        variables,
        locals,
        fast_math,
        &mut HashSet::new(),
    )
    .expect("numeric repeat count was validated");
    let times = local_to_number(times);
    line(
        output,
        5,
        &format!("let mut repeat_remaining = script.frames.last().unwrap().repeat_counts[{slot}];"),
    );
    line(output, 5, "if repeat_remaining == 0 {");
    line(
        output,
        6,
        &format!("repeat_remaining = scratch_round({times}).max(0.0) as usize;"),
    );
    line(
        output,
        6,
        &format!("script.frames.last_mut().unwrap().repeat_counts[{slot}] = repeat_remaining;"),
    );
    line(output, 5, "}");
    line(output, 5, "if repeat_remaining == 0 {");
    line(
        output,
        6,
        &format!(
            "script.frames.last_mut().unwrap().pc = {};",
            pc_literal(next_pc)
        ),
    );
    line(output, 6, "continue;");
    line(output, 5, "}");
    line(output, 5, "loop {");
    generate_native_numeric_body(target, variables, plan, locals, fast_math, output, 6);
    line(output, 6, "repeat_remaining -= 1;");
    line(
        output,
        6,
        &format!("script.frames.last_mut().unwrap().repeat_counts[{slot}] = repeat_remaining;"),
    );
    line(
        output,
        6,
        if no_warp_timer {
            "if !scratch_warp_mode {"
        } else {
            "if !scratch_warp_mode || script.warp_budget.expired(clock) {"
        },
    );
    line(
        output,
        7,
        &format!(
            "script.frames.last_mut().unwrap().pc = if repeat_remaining > 0 {{ {pc} }} else {{ {} }};",
            pc_literal(next_pc)
        ),
    );
    generate_local_commits(plan, variables, output, 7);
    line(output, 7, "return ScriptStatus::Yield;");
    line(output, 6, "}");
    line(output, 6, "if repeat_remaining == 0 { break; }");
    line(output, 5, "}");
    generate_local_commits(plan, variables, output, 5);
    line(
        output,
        5,
        &format!(
            "script.frames.last_mut().unwrap().pc = {};",
            pc_literal(next_pc)
        ),
    );
    line(output, 5, "continue;");
}

pub(super) fn generate_native_numeric_body(
    target: &ScratchTarget,
    variables: &HashMap<String, VariableInfo>,
    plan: &NumericLoopPlan,
    locals: &HashMap<usize, String>,
    fast_math: bool,
    output: &mut String,
    indent: usize,
) {
    for id in &plan.body {
        let block = &target.blocks[id];
        let variable = variable_field(block, "VARIABLE", variables)
            .expect("numeric loop target was validated");
        let value = local_expression(
            target,
            block.inputs.get("VALUE"),
            variables,
            locals,
            fast_math,
            &mut HashSet::new(),
        )
        .expect("numeric loop expression was validated");
        let local = &locals[&variable.slot];
        if block.opcode == "data_setvariableto" {
            assert_eq!(value.kind, LocalValueKind::Number);
            line(
                output,
                indent,
                &format!("{local} = {};", expression_argument(&value.code)),
            );
        } else if let Some(constant) = variable
            .non_nan
            .then(|| {
                constant_numeric_input(target, block.inputs.get("VALUE"), &mut HashSet::new())
                    .filter(|value| value.is_finite())
            })
            .flatten()
        {
            line(output, indent, &format!("{local} += {constant:?}_f64;"));
        } else if fast_math && value.kind == LocalValueKind::Number {
            line(
                output,
                indent,
                &format!("{local} += {};", expression_argument(&value.code)),
            );
        } else {
            line(
                output,
                indent,
                &format!(
                    "{local} = scratch_add({local}, {});",
                    expression_argument(&value.code)
                ),
            );
        }
    }
}

pub(super) fn generate_local_commits(
    plan: &NumericLoopPlan,
    variables: &HashMap<String, VariableInfo>,
    output: &mut String,
    indent: usize,
) {
    for slot in &plan.written_slots {
        line(
            output,
            indent,
            &variable_write_for_slot(variables, *slot, &format!("scratch_local_{slot}.into()")),
        );
    }
}

pub(super) fn variable_read_for_slot(
    variables: &HashMap<String, VariableInfo>,
    slot: usize,
) -> String {
    variables
        .values()
        .find(|variable| variable.slot == slot)
        .map(VariableInfo::read_expression)
        .unwrap_or_else(|| format!("state.get({slot})"))
}

pub(super) fn variable_write_for_slot(
    variables: &HashMap<String, VariableInfo>,
    slot: usize,
    value: &str,
) -> String {
    variables
        .values()
        .find(|variable| variable.slot == slot)
        .map(|variable| variable.write_statement(value))
        .unwrap_or_else(|| format!("state.set({slot}, {value});"))
}

pub(super) fn is_straight_line_block(block: &ScratchBlock) -> bool {
    !block.opcode.starts_with("control_")
        && !matches!(
            block.opcode.as_str(),
            "motion_glideto"
                | "motion_glidesecstoxy"
                | "looks_sayforsecs"
                | "looks_thinkforsecs"
                | "looks_switchbackdropto"
                | "looks_nextbackdrop"
                | "sound_playuntildone"
                | "sound_seteffectto"
                | "sound_changeeffectby"
                | "sound_setvolumeto"
                | "sound_changevolumeby"
        )
        && block.opcode != "procedures_call"
        && block.opcode != "event_broadcast"
        && block.opcode != "event_broadcastandwait"
}

pub(super) fn generate_loop_boundary(
    target: &ScratchTarget,
    id: &str,
    graph: &ScriptGraph,
    no_warp_timer: bool,
    output: &mut String,
) {
    let pc = graph.boundary_pcs[id];
    line(output, 4, &format!("{pc} => {{"));
    let block = &target.blocks[id];
    if block.opcode == "control_repeat" {
        let slot = graph.repeat_slots[id];
        let next_pc = block
            .next
            .as_ref()
            .and_then(|next| graph.block_pcs.get(next).copied())
            .unwrap_or(graph.fallthrough_pcs[id]);
        let branch_pc = block
            .inputs
            .get("SUBSTACK")
            .and_then(parse_input)
            .and_then(|input| match input {
                InputValue::BlockReference(id) => graph.block_pcs.get(id).copied(),
                _ => None,
            })
            .unwrap_or(pc);
        line(
            output,
            5,
            &format!(
                "let repeat_remaining = script.frames.last().unwrap().repeat_counts[{slot}].saturating_sub(1);"
            ),
        );
        line(
            output,
            5,
            &format!("script.frames.last_mut().unwrap().repeat_counts[{slot}] = repeat_remaining;"),
        );
        line(
            output,
            5,
            &format!(
                "let next_pc = if repeat_remaining > 0 {{ {branch_pc} }} else {{ {} }};",
                pc_literal(next_pc)
            ),
        );
        line(output, 5, "script.frames.last_mut().unwrap().pc = next_pc;");
    } else {
        let loop_pc = graph.block_pcs[id];
        line(
            output,
            5,
            &format!("script.frames.last_mut().unwrap().pc = {loop_pc};"),
        );
    }
    if no_warp_timer {
        line(
            output,
            5,
            "if !script.frames.last().unwrap().warp_mode { return ScriptStatus::Yield; }",
        );
    } else {
        line(
            output,
            5,
            "if script.yield_at_safe_point(clock) { return ScriptStatus::Yield; }",
        );
    }
    line(output, 5, "continue;");
    line(output, 4, "},");
}

// Only cache indexes whose expression is composed of pure arithmetic,
// literals and variable reads. Preserve dynamic selector values in the cache.
pub(super) fn collect_list_index_variables(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    active: &mut HashSet<String>,
    found: &mut HashSet<usize>,
) -> bool {
    match input.and_then(parse_input) {
        Some(InputValue::Number(_) | InputValue::String(_)) => true,
        Some(InputValue::VariableReference(reference)) => {
            super::super::project::variable_for_target(target, reference.id, variables).is_some_and(
                |variable| {
                    found.insert(variable.slot);
                    true
                },
            )
        }
        Some(InputValue::BlockReference(id)) => {
            if !active.insert(id.to_owned()) {
                return false;
            }
            let result = target
                .blocks
                .get(id)
                .is_some_and(|block| match block.opcode.as_str() {
                    "data_variable" => {
                        variable_field(block, "VARIABLE", variables).is_some_and(|variable| {
                            found.insert(variable.slot);
                            true
                        })
                    }
                    "operator_add" | "operator_subtract" | "operator_multiply"
                    | "operator_divide" => {
                        collect_list_index_variables(
                            target,
                            block.inputs.get("NUM1"),
                            variables,
                            active,
                            found,
                        ) && collect_list_index_variables(
                            target,
                            block.inputs.get("NUM2"),
                            variables,
                            active,
                            found,
                        )
                    }
                    "math_number"
                    | "math_integer"
                    | "math_whole_number"
                    | "math_positive_number"
                    | "math_angle"
                    | "text" => true,
                    _ => false,
                });
            active.remove(id);
            result
        }
        _ => false,
    }
}

pub(super) fn cacheable_list_argument(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
) -> Option<HashSet<usize>> {
    let InputValue::BlockReference(id) = input.and_then(parse_input)? else {
        return None;
    };
    let block = target.blocks.get(id)?;
    if block.opcode != "data_itemoflist" {
        return None;
    }
    let mut found = HashSet::new();
    collect_list_index_variables(
        target,
        block.inputs.get("INDEX"),
        variables,
        &mut HashSet::new(),
        &mut found,
    )
    .then_some(found)
}

// The speculative numeric list read may be retried only when the index
// expression cannot itself invoke Scratch's random list selector.
pub(super) fn index_has_random_selector(
    target: &ScratchTarget,
    input: Option<&Value>,
    active: &mut HashSet<String>,
) -> bool {
    match input.and_then(parse_input) {
        Some(InputValue::String(value)) => value
            .as_str()
            .is_some_and(|text| matches!(text, "random" | "any")),
        Some(InputValue::BlockReference(id)) => {
            if !active.insert(id.to_owned()) {
                return true;
            }
            let found = target.blocks.get(id).is_none_or(|block| {
                (block.opcode == "text"
                    && field_value(block, "TEXT")
                        .and_then(Value::as_str)
                        .is_some_and(|text| matches!(text, "random" | "any")))
                    || block
                        .inputs
                        .values()
                        .any(|nested| index_has_random_selector(target, Some(nested), active))
            });
            active.remove(id);
            found
        }
        _ => false,
    }
}

pub(super) fn numeric_list_call_candidate(
    target: &ScratchTarget,
    block: &ScratchBlock,
    procedure: &ProcedureSignature,
    entry: &ProcedureEntry,
    caches: &[Option<HashSet<usize>>],
) -> bool {
    entry.numeric_direct
        && !procedure.argument_ids.is_empty()
        && caches.iter().all(Option::is_some)
        && procedure.argument_ids.iter().all(|id| {
            let Some(InputValue::BlockReference(reporter)) =
                block.inputs.get(id).and_then(parse_input)
            else {
                return false;
            };
            target.blocks.get(reporter).is_some_and(|reporter| {
                !index_has_random_selector(
                    target,
                    reporter.inputs.get("INDEX"),
                    &mut HashSet::new(),
                )
            })
        })
}

pub(super) fn cached_procedure_indexes(
    target: &ScratchTarget,
    block: &ScratchBlock,
    procedure: &ProcedureSignature,
    entry: &ProcedureEntry,
    variables: &HashMap<String, VariableInfo>,
) -> (Vec<Option<HashSet<usize>>>, Vec<usize>) {
    let caches = procedure
        .argument_ids
        .iter()
        .map(|id| {
            if entry.direct_name.is_some() {
                cacheable_list_argument(target, block.inputs.get(id), variables)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let mut counts = HashMap::<usize, usize>::new();
    for slots in caches.iter().flatten() {
        for slot in slots {
            *counts.entry(*slot).or_default() += 1;
        }
    }
    let numeric_candidate = numeric_list_call_candidate(target, block, procedure, entry, &caches);
    let mut result = counts
        .into_iter()
        .filter_map(|(slot, count)| (count >= 2 || numeric_candidate).then_some(slot))
        .collect::<Vec<_>>();
    result.sort_unstable();
    (caches, result)
}
