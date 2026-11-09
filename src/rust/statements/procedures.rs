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
    pub(super) call_pc: usize,
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
        call_pc,
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
    let numeric_frame_values = entry
        .numeric_arguments
        .as_deref()
        .filter(|numeric| {
            !numeric.is_empty()
                && numeric.len() == procedure.argument_ids.len()
                && numeric.iter().all(|is_numeric| *is_numeric)
        })
        .and_then(|_| {
            procedure
                .argument_ids
                .iter()
                .enumerate()
                .map(|(index, argument_id)| {
                    Some(
                        super::direct::numeric_direct_argument_expression(
                            target,
                            block.inputs.get(argument_id),
                            variables,
                            lists,
                            arguments,
                        )
                        .unwrap_or_else(|| format!("scratch_to_number({})", raw_values[index])),
                    )
                })
                .collect::<Option<Vec<_>>>()
        });
    let frame_values = values.join(", ");
    if let Some(direct_warp_name) = &entry.direct_warp_name {
        debug_assert!(emit_continue);
        let direct_warp_is_immediate = procedure.warp && (entry.bounded_warp || no_warp_timer);
        if !direct_warp_is_immediate {
            line(
                output,
                indent,
                "let inherited_warp = script.frames.last().is_some_and(|frame| frame.warp_mode);",
            );
        }
        let direct_args = if let Some(numeric_arguments) = &entry.numeric_warp_arguments {
            procedure
                .argument_ids
                .iter()
                .enumerate()
                .map(|(index, argument_id)| {
                    if numeric_arguments.get(index).copied().unwrap_or(false) {
                        let value = super::direct::numeric_direct_argument_expression(
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
                            &format!("let direct_warp_arg_{index}: f64 = {value};"),
                        );
                    } else {
                        line(
                            output,
                            indent,
                            &format!(
                                "let direct_warp_arg_{index}: ScratchValue = {};",
                                values[index]
                            ),
                        );
                    }
                    format!("direct_warp_arg_{index}")
                })
                .collect::<Vec<_>>()
        } else {
            values
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
                .collect::<Vec<_>>()
        };
        let direct_args = if direct_args.is_empty() {
            String::new()
        } else {
            format!(", {}", direct_args.join(", "))
        };
        let direct_frame_values = procedure
            .argument_ids
            .iter()
            .enumerate()
            .map(|(index, _)| format!("direct_warp_arg_{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let direct_numeric_frame_values = numeric_frame_values.as_ref().map(|_| {
            procedure
                .argument_ids
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    if entry
                        .numeric_warp_arguments
                        .as_ref()
                        .and_then(|arguments| arguments.get(index))
                        .copied()
                        .unwrap_or(false)
                    {
                        format!("direct_warp_arg_{index}")
                    } else {
                        format!("scratch_to_number(direct_warp_arg_{index})")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        });
        if procedure.warp && (entry.bounded_warp || no_warp_timer) {
            if entry.bounded_warp && !no_warp_timer {
                line(output, indent, "if script.warp_budget.expired(clock) {");
                line(
                    output,
                    indent + 1,
                    &format!(
                        "script.frames.last_mut().unwrap().pc = {};",
                        pc_literal(call_pc)
                    ),
                );
                line(output, indent + 1, "return ScriptStatus::Yield;");
                line(output, indent, "}");
            }
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
                    "{}",
                    direct_warp_call(
                        direct_warp_name,
                        &direct_args,
                        entry.direct_warp_returns_status
                    )
                ),
            );
            if entry.bounded_warp && !no_warp_timer {
                line(
                    output,
                    indent,
                    "if script.warp_budget.expired(clock) { return ScriptStatus::Yield; }",
                );
            }
            if emit_continue {
                line(output, indent, "continue;");
            }
            return;
        }

        line(output, indent, "if inherited_warp {");
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
                &procedure_frame_push(
                    entry,
                    &direct_frame_values,
                    direct_numeric_frame_values.as_deref(),
                    "inherited_warp || false",
                ),
            );
            line(output, indent + 2, "return ScriptStatus::Yield;");
            line(output, indent + 1, "}");
        }
        line(
            output,
            indent + 1,
            &format!(
                "{}",
                direct_warp_call(
                    direct_warp_name,
                    &direct_args,
                    entry.direct_warp_returns_status
                )
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
        // The numeric direct variant can resume only from a numeric frame.
        // Procedures with mixed ScratchValue arguments keep the generic direct
        // path so a warp-budget yield retains their original argument types.
        let proven_numeric_call = entry.numeric_direct
            && entry
                .numeric_call_arguments
                .as_deref()
                .is_some_and(|arguments| {
                    arguments.len() == procedure.argument_ids.len()
                        && arguments.iter().all(|is_numeric| *is_numeric)
                });
        let numeric_candidate =
            (numeric_list_candidate || proven_numeric_call) && numeric_frame_values.is_some();
        if numeric_candidate && proven_numeric_call {
            let numeric_vars = (0..values.len())
                .map(|index| format!("numeric_value_{index}"))
                .collect::<Vec<_>>();
            for (index, value) in numeric_frame_values.as_ref().unwrap().iter().enumerate() {
                line(
                    output,
                    indent,
                    &format!("let numeric_value_{index}: f64 = ({value});"),
                );
            }
            if !no_warp_timer {
                line(
                    output,
                    indent,
                    &format!(
                        "if (inherited_warp || {}) && script.warp_budget.expired(clock) {{",
                        procedure.warp
                    ),
                );
                line(
                    output,
                    indent + 1,
                    &procedure_frame_push(
                        entry,
                        &frame_values,
                        Some(&numeric_vars.join(", ")),
                        &format!("inherited_warp || {}", procedure.warp),
                    ),
                );
                line(output, indent + 1, "return ScriptStatus::Yield;");
                line(output, indent, "}");
            }
            line(
                output,
                indent,
                &format!(
                    "{direct_name}_numbers(state, clock, {});",
                    numeric_vars.join(", ")
                ),
            );
            if emit_continue {
                line(output, indent, "continue;");
            }
            return;
        }
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
                line(
                    output,
                    indent + 3,
                    &procedure_frame_push(
                        entry,
                        &frame_values,
                        Some(&numeric_vars.join(", ")),
                        &format!("inherited_warp || {}", procedure.warp),
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
        let direct_frame_values = args.join(", ");
        let direct_numeric_frame_values = numeric_frame_values.as_ref().map(|_| {
            args.iter()
                .map(|argument| format!("scratch_to_number({argument})"))
                .collect::<Vec<_>>()
                .join(", ")
        });
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
                &procedure_frame_push(
                    entry,
                    &direct_frame_values,
                    direct_numeric_frame_values.as_deref(),
                    &format!("inherited_warp || {}", procedure.warp),
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
    if procedure.warp && return_pc == usize::MAX {
        let (smallvec_type, push_method, tail_values) =
            if let Some(numeric_values) = &numeric_frame_values {
                ("f64", "push_numeric_frame", numeric_values.join(", "))
            } else {
                ("ScratchValue", "push_frame", frame_values.clone())
            };
        line(
            output,
            indent,
            &format!(
                "let scratch_tail_call_arguments: smallvec::SmallVec<[{smallvec_type}; 16]> = smallvec::smallvec![{tail_values}];"
            ),
        );
        line(output, indent, "script.pop_frame();");
        line(
            output,
            indent,
            &format!(
                "script.{push_method}({}, {}, smallvec::smallvec![0; {}], scratch_tail_call_arguments, true);",
                entry.function_id,
                pc_literal(entry.start_pc),
                entry.repeat_count
            ),
        );
        if !no_warp_timer {
            line(
                output,
                indent,
                "if script.warp_budget.expired(clock) { return ScriptStatus::Yield; }",
            );
        }
        line(output, indent, "continue;");
        return;
    }
    if !procedure.warp {
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
    let warp_mode = if procedure.warp {
        "true"
    } else {
        "inherited_warp"
    };
    line(
        output,
        indent,
        &procedure_frame_push(
            entry,
            &frame_values,
            numeric_frame_values
                .as_ref()
                .map(|values| values.join(", "))
                .as_deref(),
            warp_mode,
        ),
    );
    if !procedure.warp {
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
    }
    if !no_warp_timer && procedure.warp {
        line(
            output,
            indent,
            "if script.warp_budget.expired(clock) { return ScriptStatus::Yield; }",
        );
    } else if !no_warp_timer {
        line(
            output,
            indent,
            "if script.frames.last().unwrap().warp_mode && script.warp_budget_expired(clock) { return ScriptStatus::Yield; }",
        );
    }
    line(output, indent, "continue;");
}

fn direct_warp_call(name: &str, args: &str, returns_status: bool) -> String {
    if returns_status {
        format!(
            "if matches!({name}(state, clock{args}), DirectWarpStatus::StopAll) {{ return ScriptStatus::StopAll; }}"
        )
    } else {
        format!("{name}(state, clock{args});")
    }
}

fn procedure_frame_push(
    entry: &ProcedureEntry,
    dynamic_arguments: &str,
    numeric_arguments: Option<&str>,
    warp_mode: &str,
) -> String {
    let (method, arguments) = match numeric_arguments {
        Some(arguments) => ("push_numeric_frame", arguments),
        None => ("push_frame", dynamic_arguments),
    };
    format!(
        "script.{method}({}, {}, smallvec::smallvec![0; {}], smallvec::smallvec![{}], {});",
        entry.function_id,
        pc_literal(entry.start_pc),
        entry.repeat_count,
        arguments,
        warp_mode
    )
}

pub(super) fn pc_literal(pc: usize) -> String {
    if pc == usize::MAX {
        "usize::MAX".to_owned()
    } else {
        pc.to_string()
    }
}
