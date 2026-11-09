use super::*;

#[derive(Clone, Copy)]
pub(super) struct ProcedureCallContext<'a> {
    pub(super) target: &'a ScratchTarget,
    pub(super) variables: &'a HashMap<String, VariableInfo>,
    pub(super) lists: &'a HashMap<String, ListInfo>,
    pub(super) arguments: &'a HashMap<String, String>,
    pub(super) procedures: &'a [ProcedureSignature],
    pub(super) entries: &'a HashMap<String, ProcedureEntry>,
    pub(super) no_warp_timer: bool,
}

pub(super) struct ProcedureCallOutput<'output, 'cache> {
    pub(super) output: &'output mut String,
    pub(super) indent: usize,
    pub(super) emit_continue: bool,
    pub(super) reusable_indexes: Option<&'cache HashSet<usize>>,
    pub(super) reusable_items: Option<&'cache mut HashMap<String, CachedNumericItem>>,
}

pub(super) fn generate_resumable_procedure_call(
    context: ProcedureCallContext<'_>,
    block: &ScratchBlock,
    return_pc: usize,
    emission: ProcedureCallOutput<'_, '_>,
) {
    let ProcedureCallContext {
        target,
        variables,
        lists,
        arguments,
        procedures,
        entries,
        no_warp_timer,
    } = context;
    let ProcedureCallOutput {
        output,
        indent,
        emit_continue,
        reusable_indexes,
        reusable_items,
    } = emission;
    let mut reusable_items = reusable_items;
    let Some(procedure) = procedure_for_call(block, procedures) else {
        line(
            output,
            indent,
            "todo!(\"unsupported or missing custom procedure\");",
        );
        return;
    };
    let Some(entry) = entries.get(&procedure.definition_id) else {
        line(output, indent, "todo!(\"missing custom procedure body\");");
        return;
    };

    let (argument_caches, cached_slots) =
        cached_procedure_indexes(target, block, procedure, entry, variables);
    let raw_values = procedure
        .argument_ids
        .iter()
        .enumerate()
        .map(|(index, argument_id)| {
            let input = block.inputs.get(argument_id);
            if input.is_none() {
                return format!(
                    "todo!({})",
                    quote(&format!("missing procedure argument {index}"))
                );
            }
            let mut cached_variables;
            let expression_variables = if let Some(slots) = &argument_caches[index] {
                cached_variables = variables.clone();
                for variable in cached_variables.values_mut() {
                    if slots.contains(&variable.slot) && cached_slots.contains(&variable.slot) {
                        variable.index_read_override =
                            Some(format!("cached_index_{}", variable.slot));
                    }
                }
                &cached_variables
            } else {
                variables
            };
            expression(
                target,
                input,
                expression_variables,
                lists,
                arguments,
                &mut HashSet::new(),
            )
        })
        .collect::<Vec<_>>();
    let values = raw_values
        .iter()
        .map(|value| format!("({value}).to_owned().into()"))
        .collect::<Vec<_>>();
    let mut frame_values = values.join(", ");
    if let Some(direct_warp_name) = &entry.direct_warp_name
        && emit_continue
    {
        line(
            output,
            indent,
            "let inherited_warp = script.frames.last().is_some_and(|frame| frame.warp_mode);",
        );
        for slot in &cached_slots {
            if reusable_indexes.is_some_and(|cached| cached.contains(slot)) {
                continue;
            }
            if let Some(variable) = variables.values().find(|variable| variable.slot == *slot) {
                line(
                    output,
                    indent,
                    &format!(
                        "let cached_index_{slot} = CachedListIndex::new({});",
                        variable.read_expression()
                    ),
                );
            }
        }
        let direct_args = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                line(
                    output,
                    indent,
                    &format!("let direct_warp_arg_{index}: ScratchValue = {value};"),
                );
                format!("direct_warp_arg_{index}")
            })
            .collect::<Vec<_>>();
        let direct_args = if direct_args.is_empty() {
            String::new()
        } else {
            format!(", {}", direct_args.join(", "))
        };
        let direct_frame_args = if direct_args.is_empty() {
            "[]".to_owned()
        } else {
            format!("[{}]", direct_args.trim_start_matches(", "))
        };
        line(
            output,
            indent,
            &format!("if inherited_warp || {} {{", procedure.warp),
        );
        line(
            output,
            indent + 1,
            &format!(
                "script.frames.last_mut().unwrap().pc = {};",
                pc_literal(return_pc)
            ),
        );
        if !no_warp_timer {
            line(output, indent + 1, "if script.warp_budget.expired(clock) {");
            line(
                output,
                indent + 2,
                &format!(
                    "script.push_frame({}, {}, {}, {}, true);",
                    entry.function_id,
                    pc_literal(entry.start_pc),
                    entry.repeat_count,
                    direct_frame_args
                ),
            );
            line(output, indent + 2, "return ScriptStatus::Yield;");
            line(output, indent + 1, "}");
        }
        line(
            output,
            indent + 1,
            &format!(
                "if !{direct_warp_name}(state, clock{direct_args}) {{ return ScriptStatus::Complete; }}"
            ),
        );
        if !no_warp_timer {
            line(
                output,
                indent + 1,
                "if script.warp_budget.expired(clock) { return ScriptStatus::Yield; }",
            );
        }
        if emit_continue {
            line(output, indent + 1, "continue;");
        }
        line(output, indent, "}");
        frame_values = values
            .iter()
            .enumerate()
            .map(|(index, _)| format!("direct_warp_arg_{index}"))
            .collect::<Vec<_>>()
            .join(", ");
    }
    if let Some(direct_name) = &entry.direct_name {
        if !no_warp_timer {
            line(
                output,
                indent,
                "let inherited_warp = script.frames.last().is_some_and(|frame| frame.warp_mode);",
            );
        }
        line(
            output,
            indent,
            &format!(
                "script.frames.last_mut().unwrap().pc = {};",
                pc_literal(return_pc)
            ),
        );
        for slot in &cached_slots {
            if reusable_indexes.is_some_and(|cached| cached.contains(slot)) {
                continue;
            }
            if let Some(variable) = variables.values().find(|variable| variable.slot == *slot) {
                line(
                    output,
                    indent,
                    &format!(
                        "let cached_index_{slot} = CachedListIndex::new({});",
                        variable.read_expression()
                    ),
                );
            }
        }
        let numeric_list_candidate =
            numeric_list_call_candidate(target, block, procedure, entry, &argument_caches);
        let numeric_candidate = numeric_list_candidate;
        if numeric_candidate {
            let guards = cached_slots
                .iter()
                .map(|slot| format!("cached_index_{slot}.is_numeric()"))
                .collect::<Vec<_>>();
            let guard = if guards.is_empty() {
                "true".to_owned()
            } else {
                guards.join(" && ")
            };
            let mut item_names = Vec::new();
            for (index, value) in raw_values.iter().enumerate() {
                if let Some(items) = reusable_items
                    .as_deref_mut()
                    .filter(|_| numeric_list_candidate)
                {
                    if !items.contains_key(value) {
                        let InputValue::BlockReference(id) = block
                            .inputs
                            .get(&procedure.argument_ids[index])
                            .and_then(parse_input)
                            .unwrap()
                        else {
                            unreachable!()
                        };
                        let reporter = &target.blocks[id];
                        let list = list_field(reporter, "LIST", lists).unwrap();
                        let item = CachedNumericItem {
                            name: format!("cached_numeric_item_{}", output.len()),
                            list_slot: list.slot,
                            index_slots: argument_caches[index].clone().unwrap_or_default(),
                        };
                        line(
                            output,
                            indent,
                            &format!("let mut {} = f64::NAN;", item.name),
                        );
                        items.insert(value.clone(), item);
                    }
                    item_names.push(Some(items[value].name.clone()));
                } else {
                    item_names.push(None);
                }
            }
            line(output, indent, "let mut numeric_call_completed = false;");
            line(output, indent, &format!("if {guard} {{"));
            for (index, value) in raw_values.iter().enumerate() {
                let expression = match &item_names[index] {
                    Some(name) => format!(
                        "{{ if {name}.is_nan() {{ {name} = ({value}).number_or_nan(); }} {name} }}"
                    ),
                    None => format!("({value}).number_or_nan()"),
                };
                line(
                    output,
                    indent + 1,
                    &format!("let numeric_value_{index} = {expression};"),
                );
            }
            let numeric_vars = (0..values.len())
                .map(|index| format!("numeric_value_{index}"))
                .collect::<Vec<_>>();
            let valid = numeric_vars
                .iter()
                .map(|name| format!("!{name}.is_nan()"))
                .collect::<Vec<_>>()
                .join(" && ");
            line(output, indent + 1, &format!("if {valid} {{"));
            if !no_warp_timer {
                line(
                    output,
                    indent + 2,
                    &format!(
                        "if (inherited_warp || {}) && script.warp_budget.expired(clock) {{",
                        procedure.warp
                    ),
                );
                let frame_values = numeric_vars
                    .iter()
                    .map(|name| format!("ScratchValue::Number({name})"))
                    .collect::<Vec<_>>()
                    .join(", ");
                line(
                    output,
                    indent + 3,
                    &format!(
                        "script.push_frame({}, {}, {}, [{frame_values}], inherited_warp || {});",
                        entry.function_id,
                        pc_literal(entry.start_pc),
                        entry.repeat_count,
                        procedure.warp
                    ),
                );
                line(output, indent + 3, "return ScriptStatus::Yield;");
                line(output, indent + 2, "}");
            }
            line(
                output,
                indent + 2,
                &format!(
                    "{direct_name}_numbers(state, clock, {});",
                    numeric_vars.join(", ")
                ),
            );
            line(output, indent + 2, "numeric_call_completed = true;");
            line(output, indent + 1, "}");
            line(output, indent, "}");
            line(output, indent, "if !numeric_call_completed {");
        }
        let call_indent = indent + usize::from(numeric_candidate);
        for (index, value) in values.iter().enumerate() {
            line(
                output,
                call_indent,
                &format!("let direct_arg_{index}: ScratchValue = {value};"),
            );
        }
        let args = (0..values.len())
            .map(|index| format!("direct_arg_{index}"))
            .collect::<Vec<_>>();
        if !no_warp_timer {
            line(
                output,
                call_indent,
                &format!(
                    "if (inherited_warp || {}) && script.warp_budget.expired(clock) {{",
                    procedure.warp
                ),
            );
            line(
                output,
                call_indent + 1,
                &format!(
                    "script.push_frame({}, {}, {}, [{}], inherited_warp || {});",
                    entry.function_id,
                    pc_literal(entry.start_pc),
                    entry.repeat_count,
                    args.join(", "),
                    procedure.warp
                ),
            );
            line(output, call_indent + 1, "return ScriptStatus::Yield;");
            line(output, call_indent, "}");
        }
        let arguments = if args.is_empty() {
            String::new()
        } else {
            format!(", {}", args.join(", "))
        };
        line(
            output,
            call_indent,
            &format!("{direct_name}(state, clock{arguments});"),
        );
        if numeric_candidate {
            line(output, indent, "}");
        }
        if emit_continue {
            line(output, indent, "continue;");
        }
        return;
    }
    let inherited_warp = procedure.warp;
    line(
        output,
        indent,
        "let inherited_warp = script.frames.last().is_some_and(|frame| frame.warp_mode);",
    );
    line(
        output,
        indent,
        &format!(
            "script.frames.last_mut().unwrap().pc = {};",
            pc_literal(return_pc)
        ),
    );
    line(
        output,
        indent,
        &format!(
            "script.push_frame({}, {}, {}, [{}], inherited_warp || {});",
            entry.function_id,
            pc_literal(entry.start_pc),
            entry.repeat_count,
            frame_values,
            inherited_warp
        ),
    );
    line(
        output,
        indent,
        "let recursive_call = script.frames.iter().filter(|frame| frame.function == script.frames.last().unwrap().function).count() > 1;",
    );
    line(
        output,
        indent,
        "if recursive_call && !script.frames.last().unwrap().warp_mode { return ScriptStatus::Yield; }",
    );
    if !no_warp_timer {
        line(
            output,
            indent,
            "if script.frames.last().unwrap().warp_mode && script.warp_budget_expired(clock) { return ScriptStatus::Yield; }",
        );
    }
    line(output, indent, "continue;");
}

pub(super) fn pc_literal(pc: usize) -> String {
    if pc == usize::MAX {
        "usize::MAX".to_owned()
    } else {
        pc.to_string()
    }
}
