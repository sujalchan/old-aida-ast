use super::*;

fn is_borrowed_scratch_value_input(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
) -> bool {
    let borrowed_variable = |variable: &VariableInfo| {
        variable
            .index_read_override
            .as_deref()
            .is_none_or(|expression| expression.starts_with("&scratch_dynamic_"))
    };
    match input.and_then(parse_input) {
        Some(InputValue::VariableReference(reference)) => {
            super::super::project::variable_for_target(target, reference.id, variables)
                .is_some_and(borrowed_variable)
        }
        Some(InputValue::BlockReference(id)) => target.blocks.get(id).is_some_and(|block| {
            match block.opcode.as_str() {
                "data_itemoflist" => true,
                "data_variable" => variable_field(block, "VARIABLE", variables)
                    .is_some_and(|variable| borrowed_variable(&variable)),
                "argument_reporter_string_number" | "argument_reporter_boolean" => true,
                _ => false,
            }
        }),
        _ => false,
    }
}

pub(super) struct StructuredBody {
    pub(super) blocks: HashSet<String>,
    pub(super) has_procedure_calls: bool,
}

pub(super) fn structured_loop_body(
    target: &ScratchTarget,
    loop_id: &str,
    first: &str,
    procedures: &[ProcedureSignature],
    entries: &HashMap<String, ProcedureEntry>,
) -> Option<StructuredBody> {
    let mut pending = vec![first];
    let mut seen = HashSet::<String>::new();
    let mut has_procedure_calls = false;
    while let Some(id) = pending.pop() {
        if !seen.insert(id.to_owned()) || seen.len() > 80 {
            return None;
        }
        let Some(block) = target.blocks.get(id) else {
            return None;
        };
        match block.opcode.as_str() {
            "control_if" | "control_if_else" => {
                for name in ["SUBSTACK", "SUBSTACK2"] {
                    if let Some(InputValue::BlockReference(branch)) =
                        block.inputs.get(name).and_then(parse_input)
                    {
                        pending.push(branch);
                    }
                }
            }
            "procedures_call" => {
                has_procedure_calls = true;
                let Some(procedure) = procedure_for_call(block, procedures) else {
                    return None;
                };
                if entries
                    .get(&procedure.definition_id)
                    .is_none_or(|entry| entry.direct_name.is_none())
                {
                    return None;
                }
            }
            "data_setvariableto"
            | "data_changevariableby"
            | "data_replaceitemoflist"
            | "motion_gotoxy"
            | "motion_setx"
            | "motion_sety"
            | "motion_changexby"
            | "motion_changeyby"
            | "pen_penDown"
            | "pen_penUp"
            | "pen_setPenColorToColor" => {}
            _ => return None,
        }
        if let Some(next) = block.next.as_deref() {
            pending.push(next);
        }
    }

    // These blocks are emitted inline by the structured loop arm, so their
    // individual dispatcher arms can be omitted only when the Scratch graph
    // gives every block a single parent within this loop body. Malformed or
    // shared subgraphs keep the conservative resumable states.
    if seen.iter().any(|id| {
        target
            .blocks
            .get(id)
            .and_then(|block| block.parent.as_deref())
            .is_none_or(|parent| parent != loop_id && !seen.contains(parent))
    }) {
        return None;
    }

    Some(StructuredBody {
        blocks: seen,
        has_procedure_calls,
    })
}

pub(super) fn structured_sync_statement(opcode: &str) -> bool {
    matches!(
        opcode,
        "data_setvariableto"
            | "data_changevariableby"
            | "data_replaceitemoflist"
            | "motion_gotoxy"
            | "motion_setx"
            | "motion_sety"
            | "motion_changexby"
            | "motion_changeyby"
            | "pen_penDown"
            | "pen_penUp"
            | "pen_setPenColorToColor"
    )
}

// A partially structured region exits to the dispatcher at calls, loops,
// stops, and any other operation whose effects or suspension are not known.
// Every branch remains available, including empty and nested branches.
pub(super) fn partial_structured_body(target: &ScratchTarget, first: &str) -> bool {
    let mut pending = vec![first];
    let mut seen = HashSet::new();
    let mut branch = false;
    let mut synchronous = false;
    while let Some(id) = pending.pop() {
        if !seen.insert(id) || seen.len() > 300 {
            return false;
        }
        let Some(block) = target.blocks.get(id) else {
            return false;
        };
        if matches!(block.opcode.as_str(), "control_if" | "control_if_else") {
            branch = true;
            for name in ["SUBSTACK", "SUBSTACK2"] {
                if let Some(InputValue::BlockReference(id)) =
                    block.inputs.get(name).and_then(parse_input)
                {
                    pending.push(id);
                }
            }
        } else if structured_sync_statement(&block.opcode) {
            synchronous = true;
        } else {
            continue;
        }
        if let Some(next) = block.next.as_deref() {
            pending.push(next);
        }
    }
    branch && synchronous
}

#[derive(Clone)]
pub(super) struct CachedNumericItem {
    pub(super) name: String,
    pub(super) list_slot: usize,
    pub(super) index_slots: HashSet<usize>,
}

// A canonical number has the same Scratch comparison text as the original
// value. Restrict stores to constants whose representation is also canonical.
pub(super) fn canonical_constant(input: Option<&Value>) -> Option<String> {
    match input.and_then(parse_input)? {
        InputValue::Number(value) => Some(number_literal(value)),
        InputValue::String(value) => {
            let text = value.as_str()?;
            let number = text.parse::<i32>().ok()?;
            (number.unsigned_abs() <= 1_000_000 && number.to_string() == text)
                .then(|| format!("{number}.0_f64"))
        }
        _ => None,
    }
}

// These reporters always produce a Scratch Number, independently of the
// types of their operands. Other reporters retain the dynamic path.
pub(super) fn definite_numeric_result(target: &ScratchTarget, input: Option<&Value>) -> bool {
    match input.and_then(parse_input) {
        Some(InputValue::Number(_)) => true,
        Some(InputValue::BlockReference(id)) => target.blocks.get(id).is_some_and(|b| {
            matches!(
                b.opcode.as_str(),
                "operator_add"
                    | "operator_subtract"
                    | "operator_multiply"
                    | "operator_divide"
                    | "operator_mod"
                    | "operator_round"
                    | "operator_mathop"
                    | "operator_random"
                    | "operator_length"
                    | "data_lengthoflist"
                    | "data_itemnumoflist"
                    | "motion_xposition"
                    | "motion_yposition"
                    | "motion_direction"
                    | "sound_volume"
                    | "sensing_mousex"
                    | "sensing_mousey"
                    | "sensing_timer"
                    | "sensing_dayssince2000"
                    | "sensing_loudness"
            )
        }),
        _ => false,
    }
}

pub(super) struct TypedNumericSetContext<'a> {
    pub(super) target: &'a ScratchTarget,
    pub(super) variables: &'a HashMap<String, VariableInfo>,
    pub(super) lists: &'a HashMap<String, ListInfo>,
    pub(super) arguments: &'a HashMap<String, String>,
    pub(super) deferred: &'a HashSet<usize>,
    pub(super) textual: &'a HashSet<usize>,
}

pub(super) fn emit_typed_numeric_set(
    context: TypedNumericSetContext<'_>,
    block: &ScratchBlock,
    variable: &VariableInfo,
    output: &mut String,
    indent: usize,
) {
    let TypedNumericSetContext {
        target,
        variables,
        lists,
        arguments,
        deferred,
        textual,
    } = context;
    let value = expression(
        target,
        block.inputs.get("VALUE"),
        variables,
        lists,
        arguments,
        &mut HashSet::new(),
    );
    line(
        output,
        indent,
        &format!("scratch_typed_{} = {value};", variable.slot),
    );
    if deferred.contains(&variable.slot) {
        line(
            output,
            indent,
            &format!("scratch_dirty_{} = true;", variable.slot),
        );
        if textual.contains(&variable.slot) {
            line(
                output,
                indent,
                &format!("scratch_text_{} = false;", variable.slot),
            );
        }
    } else {
        let setter = if variable.is_stage {
            "set_shared_number"
        } else {
            "set_local_number"
        };
        line(
            output,
            indent,
            &format!("state.{setter}({0}, scratch_typed_{0});", variable.slot),
        );
    }
}

// Track uses which require the original ScratchValue representation. A
// numeric operand is equivalent to a canonical Number or numeric string;
// direct copies and other unknown consumers must keep the original value.
pub(super) fn collect_unsafe_numeric_reads(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    numeric_context: bool,
    reads: &mut HashMap<usize, VariableInfo>,
    unsafe_slots: &mut HashSet<usize>,
    active: &mut HashSet<String>,
) {
    match input.and_then(parse_input) {
        Some(InputValue::VariableReference(reference)) => {
            if let Some(variable) =
                super::super::project::variable_for_target(target, reference.id, variables)
            {
                reads.insert(variable.slot, variable.clone());
                if !numeric_context {
                    unsafe_slots.insert(variable.slot);
                }
            }
        }
        Some(InputValue::BlockReference(id)) if active.insert(id.to_owned()) => {
            if let Some(block) = target.blocks.get(id) {
                if block.opcode == "data_variable"
                    && let Some(variable) = variable_field(block, "VARIABLE", variables)
                {
                    if !numeric_context {
                        unsafe_slots.insert(variable.slot);
                    }
                    reads.insert(variable.slot, variable);
                }
                let numeric_operator = matches!(
                    block.opcode.as_str(),
                    "operator_add"
                        | "operator_subtract"
                        | "operator_multiply"
                        | "operator_divide"
                        | "operator_mod"
                        | "operator_round"
                        | "operator_mathop"
                        | "operator_equals"
                        | "operator_lt"
                        | "operator_gt"
                        | "operator_not"
                        | "operator_and"
                        | "operator_or"
                );
                for (name, child) in &block.inputs {
                    let safe =
                        numeric_operator || (block.opcode == "data_itemoflist" && name == "INDEX");
                    collect_unsafe_numeric_reads(
                        target,
                        Some(child),
                        variables,
                        safe,
                        reads,
                        unsafe_slots,
                        active,
                    );
                }
            }
            active.remove(id);
        }
        _ => {}
    }
}

// Find values which stay canonical on every branch and on the loop backedge.
// Calls are excluded: a callee can change any variable or suspend execution.
pub(super) fn structured_numeric_locals(
    target: &ScratchTarget,
    first: &str,
    variables: &HashMap<String, VariableInfo>,
    include_branches: bool,
    stop_at_unsupported: bool,
) -> (Vec<VariableInfo>, HashSet<usize>, HashSet<usize>) {
    let mut pending = vec![first];
    let mut seen = HashSet::new();
    let mut candidates = HashMap::new();
    let mut unsafe_slots = HashSet::new();
    let mut textual = HashSet::new();
    let mut changed = HashSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(block) = target.blocks.get(id) else {
            return (Vec::new(), HashSet::new(), HashSet::new());
        };
        if stop_at_unsupported
            && !structured_sync_statement(&block.opcode)
            && !matches!(block.opcode.as_str(), "control_if" | "control_if_else")
        {
            continue;
        }
        if block.opcode == "procedures_call" {
            return (Vec::new(), HashSet::new(), HashSet::new());
        }
        if matches!(
            block.opcode.as_str(),
            "data_setvariableto" | "data_changevariableby"
        ) {
            let Some(variable) = variable_field(block, "VARIABLE", variables) else {
                return (Vec::new(), HashSet::new(), HashSet::new());
            };
            if block.opcode == "data_setvariableto" {
                changed.insert(variable.slot);
                if matches!(
                    block.inputs.get("VALUE").and_then(parse_input),
                    Some(InputValue::String(_))
                ) {
                    textual.insert(variable.slot);
                }
            }
            if block.opcode == "data_changevariableby" {
                changed.insert(variable.slot);
            }
            if block.opcode == "data_setvariableto"
                && !definite_numeric_result(target, block.inputs.get("VALUE"))
                && canonical_constant(block.inputs.get("VALUE")).is_none()
            {
                unsafe_slots.insert(variable.slot);
            }
            candidates.insert(variable.slot, variable);
        }
        for (name, input) in &block.inputs {
            if matches!(name.as_str(), "SUBSTACK" | "SUBSTACK2") {
                if include_branches
                    && let Some(InputValue::BlockReference(branch)) = parse_input(input)
                {
                    pending.push(branch);
                }
                continue;
            }
            let safe = matches!(
                block.opcode.as_str(),
                "control_if"
                    | "control_if_else"
                    | "data_changevariableby"
                    | "motion_gotoxy"
                    | "motion_setx"
                    | "motion_sety"
                    | "motion_changexby"
                    | "motion_changeyby"
                    | "data_replaceitemoflist"
            );
            collect_unsafe_numeric_reads(
                target,
                Some(input),
                variables,
                safe,
                &mut candidates,
                &mut unsafe_slots,
                &mut HashSet::new(),
            );
        }
        if let Some(next) = block.next.as_deref() {
            pending.push(next);
        }
    }
    let mut result = candidates
        .into_values()
        .filter(|v| !unsafe_slots.contains(&v.slot))
        .collect::<Vec<_>>();
    result.sort_by_key(|v| v.slot);
    let deferred = result
        .iter()
        .filter(|v| changed.contains(&v.slot))
        .map(|v| v.slot)
        .collect();
    (result, deferred, textual)
}

pub(super) fn emit_deferred_numeric_commits(
    output: &mut String,
    indent: usize,
    numeric: &[VariableInfo],
    deferred: &HashSet<usize>,
    textual: &HashSet<usize>,
) {
    for variable in numeric.iter().filter(|v| deferred.contains(&v.slot)) {
        let setter = if variable.is_stage {
            "set_shared_number"
        } else {
            "set_local_number"
        };
        if textual.contains(&variable.slot) {
            let string_setter = if variable.is_stage {
                "set_shared"
            } else {
                "set_local_value"
            };
            line(
                output,
                indent,
                &format!(
                    "if scratch_dirty_{0} {{ if scratch_text_{0} {{ state.{string_setter}({0}, ScratchValue::Text(scratch_string(scratch_typed_{0}))); }} else {{ state.{setter}({0}, scratch_typed_{0}); }} }}",
                    variable.slot
                ),
            );
        } else {
            line(
                output,
                indent,
                &format!(
                    "if scratch_dirty_{0} {{ state.{setter}({0}, scratch_typed_{0}); }}",
                    variable.slot
                ),
            );
        }
    }
}

pub(super) struct StraightNumericRegion<'a> {
    pub(super) blocks: Vec<&'a str>,
    pub(super) variables: Vec<VariableInfo>,
    pub(super) deferred: HashSet<usize>,
    pub(super) needed: HashSet<usize>,
}

// infer straight-line numeric regions separately from loop analysis, so
// arithmetic results can stay in native locals across successive statements.
pub(super) fn straight_numeric_region<'a>(
    target: &'a ScratchTarget,
    first: &'a str,
    variables: &HashMap<String, VariableInfo>,
) -> Option<StraightNumericRegion<'a>> {
    let mut ids = Vec::new();
    let mut candidates = HashMap::new();
    let mut unsafe_slots = HashSet::new();
    let mut written = HashSet::new();
    let mut needed = HashSet::new();
    let mut current = Some(first);
    while let Some(id) = current {
        let block = target.blocks.get(id)?;
        if !matches!(
            block.opcode.as_str(),
            "data_setvariableto" | "data_changevariableby"
        ) || (block.opcode == "data_setvariableto"
            && !definite_numeric_result(target, block.inputs.get("VALUE")))
        {
            break;
        }
        if ids.contains(&id) || ids.len() >= 200 {
            return None;
        }
        let variable = variable_field(block, "VARIABLE", variables)?;
        let mut reads = HashSet::new();
        collect_input_slots(
            target,
            block.inputs.get("VALUE"),
            variables,
            &mut reads,
            &mut HashSet::new(),
        );
        needed.extend(reads.difference(&written).copied());
        if block.opcode == "data_changevariableby" && !written.contains(&variable.slot) {
            needed.insert(variable.slot);
        }
        collect_unsafe_numeric_reads(
            target,
            block.inputs.get("VALUE"),
            variables,
            block.opcode == "data_changevariableby",
            &mut candidates,
            &mut unsafe_slots,
            &mut HashSet::new(),
        );
        written.insert(variable.slot);
        candidates.insert(variable.slot, variable);
        ids.push(id);
        current = block.next.as_deref();
    }
    if ids.len() < 4 {
        return None;
    }
    let mut numeric = candidates
        .into_values()
        .filter(|v| !unsafe_slots.contains(&v.slot))
        .collect::<Vec<_>>();
    numeric.sort_by_key(|v| v.slot);
    let deferred = numeric
        .iter()
        .filter(|v| written.contains(&v.slot))
        .map(|v| v.slot)
        .collect::<HashSet<_>>();
    (!deferred.is_empty()).then_some(StraightNumericRegion {
        blocks: ids,
        variables: numeric,
        deferred,
        needed,
    })
}

pub(super) fn emit_deferred_numeric_flush(
    output: &mut String,
    indent: usize,
    variables: &HashMap<String, VariableInfo>,
    deferred: &HashSet<usize>,
    textual: &HashSet<usize>,
) {
    let mut slots = deferred.iter().copied().collect::<Vec<_>>();
    slots.sort_unstable();
    for slot in slots {
        if let Some(variable) = variables.values().find(|v| v.slot == slot) {
            let setter = if variable.is_stage {
                "set_shared_number"
            } else {
                "set_local_number"
            };
            if textual.contains(&slot) {
                let string_setter = if variable.is_stage {
                    "set_shared"
                } else {
                    "set_local_value"
                };
                line(
                    output,
                    indent,
                    &format!(
                        "if scratch_dirty_{slot} {{ if scratch_text_{slot} {{ state.{string_setter}({slot}, ScratchValue::Text(scratch_string(scratch_typed_{slot}))); }} else {{ state.{setter}({slot}, scratch_typed_{slot}); }} scratch_dirty_{slot} = false; }}"
                    ),
                );
            } else {
                line(
                    output,
                    indent,
                    &format!(
                        "if scratch_dirty_{slot} {{ state.{setter}({slot}, scratch_typed_{slot}); scratch_dirty_{slot} = false; }}"
                    ),
                );
            }
        }
    }
}

pub(super) fn collect_input_slots(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    slots: &mut HashSet<usize>,
    active: &mut HashSet<String>,
) {
    match input.and_then(parse_input) {
        Some(InputValue::VariableReference(reference)) => {
            if let Some(variable) =
                super::super::project::variable_for_target(target, reference.id, variables)
            {
                slots.insert(variable.slot);
            }
        }
        Some(InputValue::BlockReference(id)) if active.insert(id.to_owned()) => {
            if let Some(block) = target.blocks.get(id) {
                if block.opcode == "data_variable"
                    && let Some(variable) = variable_field(block, "VARIABLE", variables)
                {
                    slots.insert(variable.slot);
                }
                for child in block.inputs.values() {
                    collect_input_slots(target, Some(child), variables, slots, active);
                }
            }
            active.remove(id);
        }
        _ => {}
    }
}

// Preserve the exact ScratchValue for mutable locals whose type is not proven.
// This avoids repeatedly looking them up and writing them to shared state in
// a synchronous region; all writes are still flushed at its exit.
pub(super) fn structured_dynamic_locals(
    target: &ScratchTarget,
    first: &str,
    variables: &HashMap<String, VariableInfo>,
    numeric: &HashSet<usize>,
) -> Vec<VariableInfo> {
    let mut pending = vec![first];
    let mut seen = HashSet::new();
    let mut writes = HashMap::new();
    let mut changed = HashSet::new();
    let mut reads = HashSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(block) = target.blocks.get(id) else {
            return Vec::new();
        };
        if block.opcode == "procedures_call" {
            return Vec::new();
        }
        if let Some(variable) = variable_field(block, "VARIABLE", variables) {
            if block.opcode == "data_setvariableto" {
                writes.insert(variable.slot, variable.clone());
            }
            if block.opcode == "data_changevariableby" {
                changed.insert(variable.slot);
            }
        }
        for (name, input) in &block.inputs {
            if matches!(name.as_str(), "SUBSTACK" | "SUBSTACK2") {
                if let Some(InputValue::BlockReference(branch)) = parse_input(input) {
                    pending.push(branch);
                }
            } else {
                collect_input_slots(
                    target,
                    Some(input),
                    variables,
                    &mut reads,
                    &mut HashSet::new(),
                );
            }
        }
        if let Some(next) = block.next.as_deref() {
            pending.push(next);
        }
    }
    let mut result = writes
        .into_values()
        .filter(|v| {
            reads.contains(&v.slot) && !numeric.contains(&v.slot) && !changed.contains(&v.slot)
        })
        .collect::<Vec<_>>();
    result.sort_by_key(|v| v.slot);
    result
}

pub(super) fn emit_deferred_dynamic_commits(
    output: &mut String,
    indent: usize,
    dynamic: &[VariableInfo],
    clear: bool,
) {
    for variable in dynamic {
        let setter = if variable.is_stage {
            "set_shared"
        } else {
            "set_local_value"
        };
        let reset = if clear {
            format!(" scratch_dynamic_dirty_{} = false;", variable.slot)
        } else {
            String::new()
        };
        line(
            output,
            indent,
            &format!(
                "if scratch_dynamic_dirty_{0} {{ state.{setter}({0}, scratch_dynamic_{0}.clone());{reset} }}",
                variable.slot
            ),
        );
    }
}

// Determine which locals must be loaded on entry. At a branch merge a write
// is definite only if it happened along every path which continues. A region
// exit has no successor here: its state is committed before dispatch resumes.
pub(super) fn collect_region_entry_reads<'a>(
    target: &'a ScratchTarget,
    mut id: Option<&'a str>,
    variables: &HashMap<String, VariableInfo>,
    written: &mut HashSet<usize>,
    needed: &mut HashSet<usize>,
    partial: bool,
) -> bool {
    while let Some(current) = id {
        let Some(block) = target.blocks.get(current) else {
            return false;
        };
        if partial
            && !structured_sync_statement(&block.opcode)
            && !matches!(block.opcode.as_str(), "control_if" | "control_if_else")
        {
            return false;
        }
        for (name, input) in &block.inputs {
            if matches!(name.as_str(), "SUBSTACK" | "SUBSTACK2") {
                continue;
            }
            let mut reads = HashSet::new();
            collect_input_slots(
                target,
                Some(input),
                variables,
                &mut reads,
                &mut HashSet::new(),
            );
            needed.extend(reads.difference(written).copied());
        }
        if matches!(block.opcode.as_str(), "control_if" | "control_if_else") {
            let branch = |name| {
                block
                    .inputs
                    .get(name)
                    .and_then(parse_input)
                    .and_then(|v| match v {
                        InputValue::BlockReference(id) => Some(id),
                        _ => None,
                    })
            };
            let mut yes = written.clone();
            let yes_continues = collect_region_entry_reads(
                target,
                branch("SUBSTACK"),
                variables,
                &mut yes,
                needed,
                partial,
            );
            let mut no = written.clone();
            let no_continues = if block.opcode == "control_if_else" {
                collect_region_entry_reads(
                    target,
                    branch("SUBSTACK2"),
                    variables,
                    &mut no,
                    needed,
                    partial,
                )
            } else {
                true
            };
            match (yes_continues, no_continues) {
                (true, true) => *written = yes.intersection(&no).copied().collect(),
                (true, false) => *written = yes,
                (false, true) => *written = no,
                (false, false) => return false,
            }
        } else if let Some(variable) = variable_field(block, "VARIABLE", variables) {
            if block.opcode == "data_changevariableby" && !written.contains(&variable.slot) {
                needed.insert(variable.slot);
            }
            if matches!(
                block.opcode.as_str(),
                "data_setvariableto" | "data_changevariableby"
            ) {
                written.insert(variable.slot);
            }
        }
        id = block.next.as_deref();
    }
    true
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_partial_structured_chain<'a>(
    target: &'a ScratchTarget,
    mut id: Option<&'a str>,
    graph: &ScriptGraph,
    variables: &HashMap<String, VariableInfo>,
    original_variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
    numeric: &HashSet<usize>,
    deferred: &HashSet<usize>,
    textual: &HashSet<usize>,
    procedures: &[ProcedureSignature],
    entries: &HashMap<String, ProcedureEntry>,
    no_warp_timer: bool,
    repeat_slot: Option<usize>,
    output: &mut String,
    indent: usize,
) -> bool {
    let mut requires_escape_label = false;
    while let Some(current) = id {
        let block = &target.blocks[current];
        if matches!(block.opcode.as_str(), "control_if" | "control_if_else") {
            let condition = boolean_input(
                target,
                block,
                "CONDITION",
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            let branch = |name| {
                block
                    .inputs
                    .get(name)
                    .and_then(parse_input)
                    .and_then(|v| match v {
                        InputValue::BlockReference(id) => Some(id),
                        _ => None,
                    })
            };
            line(output, indent, &format!("if {condition} {{"));
            requires_escape_label |= emit_partial_structured_chain(
                target,
                branch("SUBSTACK"),
                graph,
                variables,
                original_variables,
                lists,
                arguments,
                numeric,
                deferred,
                textual,
                procedures,
                entries,
                no_warp_timer,
                repeat_slot,
                output,
                indent + 1,
            );
            if block.opcode == "control_if_else" {
                line(output, indent, "} else {");
                requires_escape_label |= emit_partial_structured_chain(
                    target,
                    branch("SUBSTACK2"),
                    graph,
                    variables,
                    original_variables,
                    lists,
                    arguments,
                    numeric,
                    deferred,
                    textual,
                    procedures,
                    entries,
                    no_warp_timer,
                    repeat_slot,
                    output,
                    indent + 1,
                );
            }
            line(output, indent, "}");
        } else if structured_sync_statement(&block.opcode) {
            let variable = variable_field(block, "VARIABLE", variables);
            let deferred_change = block.opcode == "data_changevariableby"
                && variable
                    .as_ref()
                    .is_some_and(|v| deferred.contains(&v.slot));
            let typed_set = block.opcode == "data_setvariableto"
                && variable.as_ref().is_some_and(|v| numeric.contains(&v.slot))
                && definite_numeric_result(target, block.inputs.get("VALUE"));
            let typed_text_set = block.opcode == "data_setvariableto"
                && variable
                    .as_ref()
                    .is_some_and(|v| deferred.contains(&v.slot) && textual.contains(&v.slot))
                && canonical_constant(block.inputs.get("VALUE")).is_some()
                && !typed_set;
            if deferred_change {
                let value = expression(
                    target,
                    block.inputs.get("VALUE"),
                    variables,
                    lists,
                    arguments,
                    &mut HashSet::new(),
                );
                line(
                    output,
                    indent,
                    &format!("let scratch_change = scratch_to_number({value});"),
                );
            } else if typed_set {
                emit_typed_numeric_set(
                    TypedNumericSetContext {
                        target,
                        variables,
                        lists,
                        arguments,
                        deferred,
                        textual,
                    },
                    block,
                    variable.as_ref().unwrap(),
                    output,
                    indent,
                );
            } else if typed_text_set {
                let variable = variable.as_ref().unwrap();
                let value = canonical_constant(block.inputs.get("VALUE")).unwrap();
                line(
                    output,
                    indent,
                    &format!(
                        "scratch_typed_{0} = {value}; scratch_text_{0} = true; scratch_dirty_{0} = true;",
                        variable.slot
                    ),
                );
            } else if block.opcode == "data_replaceitemoflist" && !numeric.is_empty() {
                emit_deferred_numeric_flush(output, indent, original_variables, deferred, textual);
                let next = block
                    .next
                    .as_ref()
                    .and_then(|id| graph.block_pcs.get(id).copied())
                    .unwrap_or(graph.fallthrough_pcs[current]);
                generate_resumable_statement(
                    StatementContext {
                        target,
                        variables: original_variables,
                        lists,
                        arguments,
                    },
                    block,
                    next,
                    output,
                    indent,
                );
            } else {
                let next = block
                    .next
                    .as_ref()
                    .and_then(|id| graph.block_pcs.get(id).copied())
                    .unwrap_or(graph.fallthrough_pcs[current]);
                generate_resumable_statement(
                    StatementContext {
                        target,
                        variables,
                        lists,
                        arguments,
                    },
                    block,
                    next,
                    output,
                    indent,
                );
            }
            if let Some(variable) = variable.filter(|v| numeric.contains(&v.slot)) {
                match block.opcode.as_str() {
                    "data_changevariableby" => {
                        line(
                            output,
                            indent,
                            &format!(
                                "scratch_typed_{0} = scratch_number_operand(scratch_typed_{0}) + scratch_change;",
                                variable.slot
                            ),
                        );
                        if deferred_change {
                            line(
                                output,
                                indent,
                                &format!("scratch_dirty_{} = true;", variable.slot),
                            );
                        }
                        if deferred_change && textual.contains(&variable.slot) {
                            line(
                                output,
                                indent,
                                &format!("scratch_text_{} = false;", variable.slot),
                            );
                        }
                    }
                    "data_setvariableto" => {
                        if !typed_set && !typed_text_set {
                            let value = canonical_constant(block.inputs.get("VALUE"))
                                .expect("canonical store was analyzed");
                            line(
                                output,
                                indent,
                                &format!("scratch_typed_{} = {value};", variable.slot),
                            );
                        }
                    }
                    _ => {}
                }
            }
        } else if block.opcode == "procedures_call"
            && procedure_for_call(block, procedures)
                .and_then(|p| entries.get(&p.definition_id))
                .is_some_and(|entry| entry.direct_name.is_some())
        {
            emit_deferred_numeric_flush(output, indent, original_variables, deferred, textual);
            if let Some(slot) = repeat_slot {
                line(
                    output,
                    indent,
                    &format!(
                        "script.frames.last_mut().unwrap().repeat_counts[{slot}] = repeat_remaining;"
                    ),
                );
            }
            let next = block
                .next
                .as_ref()
                .and_then(|id| graph.block_pcs.get(id).copied())
                .unwrap_or(graph.fallthrough_pcs[current]);
            generate_resumable_procedure_call(
                ProcedureCallContext {
                    target,
                    variables: original_variables,
                    lists,
                    arguments,
                    procedures,
                    entries,
                    no_warp_timer,
                },
                block,
                next,
                ProcedureCallOutput {
                    output,
                    indent,
                    emit_continue: false,
                    reusable_indexes: None,
                    reusable_items: None,
                },
            );
            line(output, indent, "scratch_region_escaped = true;");
            line(output, indent, "break 'scratch_compiled_loop;");
            return true;
        } else {
            line(
                output,
                indent,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    graph.block_pcs[current]
                ),
            );
            line(output, indent, "scratch_region_escaped = true;");
            line(output, indent, "break 'scratch_compiled_loop;");
            return true;
        }
        id = block.next.as_deref();
    }
    requires_escape_label
}

pub(super) struct StructuredLoopContext<'a> {
    pub(super) target: &'a ScratchTarget,
    pub(super) graph: &'a ScriptGraph,
    pub(super) variables: &'a HashMap<String, VariableInfo>,
    pub(super) original_variables: &'a HashMap<String, VariableInfo>,
    pub(super) lists: &'a HashMap<String, ListInfo>,
    pub(super) arguments: &'a HashMap<String, String>,
    pub(super) procedures: &'a [ProcedureSignature],
    pub(super) entries: &'a HashMap<String, ProcedureEntry>,
    pub(super) no_warp_timer: bool,
    pub(super) numeric_locals: &'a HashSet<usize>,
    pub(super) deferred: &'a HashSet<usize>,
    pub(super) textual: &'a HashSet<usize>,
    pub(super) dynamic: &'a [VariableInfo],
}

pub(super) fn emit_structured_loop_chain<'a>(
    context: &StructuredLoopContext<'a>,
    mut current: Option<&'a str>,
    output: &mut String,
    indent: usize,
    cached_indexes: &mut HashSet<usize>,
    cached_items: &mut HashMap<String, CachedNumericItem>,
) {
    let StructuredLoopContext {
        target,
        graph,
        variables,
        original_variables,
        lists,
        arguments,
        procedures,
        entries,
        no_warp_timer,
        numeric_locals,
        deferred,
        textual,
        dynamic,
    } = context;
    while let Some(id) = current {
        let block = &target.blocks[id];
        let next_pc = block
            .next
            .as_ref()
            .and_then(|next| graph.block_pcs.get(next).copied())
            .unwrap_or(graph.fallthrough_pcs[id]);
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
                let mut yes = cached_indexes.clone();
                let mut yes_items = cached_items.clone();
                emit_structured_loop_chain(
                    context,
                    branch("SUBSTACK"),
                    output,
                    indent + 1,
                    &mut yes,
                    &mut yes_items,
                );
                let mut no = cached_indexes.clone();
                let mut no_items = cached_items.clone();
                if block.opcode == "control_if_else" {
                    line(output, indent, "} else {");
                    emit_structured_loop_chain(
                        context,
                        branch("SUBSTACK2"),
                        output,
                        indent + 1,
                        &mut no,
                        &mut no_items,
                    );
                }
                cached_indexes.retain(|slot| yes.contains(slot) && no.contains(slot));
                cached_items
                    .retain(|key, _| yes_items.contains_key(key) && no_items.contains_key(key));
                line(output, indent, "}");
            }
            "procedures_call" => {
                let procedure = procedure_for_call(block, procedures).unwrap();
                let entry = &entries[&procedure.definition_id];
                let (_, slots) =
                    cached_procedure_indexes(target, block, procedure, entry, variables);
                generate_resumable_procedure_call(
                    ProcedureCallContext {
                        target,
                        variables,
                        lists,
                        arguments,
                        procedures,
                        entries,
                        no_warp_timer: *no_warp_timer,
                    },
                    block,
                    next_pc,
                    ProcedureCallOutput {
                        output,
                        indent,
                        emit_continue: false,
                        reusable_indexes: Some(cached_indexes),
                        reusable_items: Some(cached_items),
                    },
                );
                cached_indexes.extend(slots);
                let definition = &target.blocks[&procedure.definition_id];
                let mut body = ScriptGraph::new();
                body.add_chain(target, definition.next.as_deref(), usize::MAX);
                for node in body.nodes {
                    if let ScriptNode::Block(body_id) = node {
                        let statement = &target.blocks[&body_id];
                        if matches!(
                            statement.opcode.as_str(),
                            "data_setvariableto" | "data_changevariableby"
                        ) && let Some(variable) =
                            variable_field(statement, "VARIABLE", variables)
                        {
                            cached_indexes.remove(&variable.slot);
                            cached_items
                                .retain(|_, item| !item.index_slots.contains(&variable.slot));
                        }
                        if matches!(
                            statement.opcode.as_str(),
                            "data_addtolist"
                                | "data_deleteoflist"
                                | "data_deletealloflist"
                                | "data_insertatlist"
                                | "data_replaceitemoflist"
                        ) {
                            if let Some(list) = list_field(statement, "LIST", lists) {
                                cached_items.retain(|_, item| item.list_slot != list.slot);
                            } else {
                                cached_items.clear();
                            }
                        }
                    }
                }
            }
            "data_setvariableto" => {
                let numeric_target = variable_field(block, "VARIABLE", variables)
                    .filter(|v| numeric_locals.contains(&v.slot));
                let dynamic_target = variable_field(block, "VARIABLE", variables)
                    .filter(|v| dynamic.iter().any(|d| d.slot == v.slot));
                if let Some(variable) = dynamic_target {
                    let value = expression(
                        target,
                        block.inputs.get("VALUE"),
                        variables,
                        lists,
                        arguments,
                        &mut HashSet::new(),
                    );
                    if is_borrowed_scratch_value_input(
                        target,
                        block.inputs.get("VALUE"),
                        variables,
                    ) {
                        line(
                            output,
                            indent,
                            &format!(
                                "scratch_dynamic_{0}.clone_from({value}); scratch_dynamic_dirty_{0} = true;",
                                variable.slot
                            ),
                        );
                    } else {
                        line(
                            output,
                            indent,
                            &format!(
                                "let scratch_new_value: ScratchValue = ({value}).to_owned().into();"
                            ),
                        );
                        line(
                            output,
                            indent,
                            &format!(
                                "scratch_dynamic_{0} = scratch_new_value; scratch_dynamic_dirty_{0} = true;",
                                variable.slot
                            ),
                        );
                    }
                    cached_items.retain(|_, item| !item.index_slots.contains(&variable.slot));
                    cached_indexes.remove(&variable.slot);
                } else if let Some(variable) = numeric_target
                    .filter(|_| definite_numeric_result(target, block.inputs.get("VALUE")))
                {
                    emit_typed_numeric_set(
                        TypedNumericSetContext {
                            target,
                            variables,
                            lists,
                            arguments,
                            deferred,
                            textual,
                        },
                        block,
                        &variable,
                        output,
                        indent,
                    );
                    cached_items.retain(|_, item| !item.index_slots.contains(&variable.slot));
                    cached_indexes.remove(&variable.slot);
                } else if let Some(variable) =
                    variable_field(block, "VARIABLE", variables).filter(|v| {
                        deferred.contains(&v.slot)
                            && textual.contains(&v.slot)
                            && canonical_constant(block.inputs.get("VALUE")).is_some()
                    })
                {
                    let value = canonical_constant(block.inputs.get("VALUE")).unwrap();
                    line(
                        output,
                        indent,
                        &format!(
                            "scratch_typed_{0} = {value}; scratch_text_{0} = true; scratch_dirty_{0} = true;",
                            variable.slot
                        ),
                    );
                    cached_items.retain(|_, item| !item.index_slots.contains(&variable.slot));
                    cached_indexes.remove(&variable.slot);
                } else if let Some(slot) = emit_shared_list_copy(
                    target, block, variables, lists, arguments, output, indent,
                ) {
                    cached_items.retain(|_, item| !item.index_slots.contains(&slot));
                    cached_indexes.insert(slot);
                } else {
                    generate_resumable_statement(
                        StatementContext {
                            target,
                            variables,
                            lists,
                            arguments,
                        },
                        block,
                        next_pc,
                        output,
                        indent,
                    );
                    if let Some(variable) = variable_field(block, "VARIABLE", variables)
                        .filter(|v| numeric_locals.contains(&v.slot))
                    {
                        let value = canonical_constant(block.inputs.get("VALUE"))
                            .expect("canonical store was analyzed");
                        line(
                            output,
                            indent,
                            &format!("scratch_typed_{} = {value};", variable.slot),
                        );
                    }
                    if let Some(variable) = variable_field(block, "VARIABLE", variables) {
                        cached_indexes.remove(&variable.slot);
                        cached_items.retain(|_, item| !item.index_slots.contains(&variable.slot));
                    }
                }
            }
            _ => {
                let deferred_change = block.opcode == "data_changevariableby"
                    && variable_field(block, "VARIABLE", variables)
                        .is_some_and(|v| deferred.contains(&v.slot));
                if deferred_change {
                    let value = expression(
                        target,
                        block.inputs.get("VALUE"),
                        variables,
                        lists,
                        arguments,
                        &mut HashSet::new(),
                    );
                    line(
                        output,
                        indent,
                        &format!("let scratch_change = scratch_to_number({value});"),
                    );
                } else if block.opcode == "data_replaceitemoflist"
                    && (!numeric_locals.is_empty() || !dynamic.is_empty())
                {
                    // This operation consumes authoritative ScratchValue inputs.
                    // Commit pending locals and use the existing lowering unchanged.
                    emit_deferred_numeric_flush(
                        output,
                        indent,
                        original_variables,
                        deferred,
                        textual,
                    );
                    emit_deferred_dynamic_commits(output, indent, dynamic, true);
                    generate_resumable_statement(
                        StatementContext {
                            target,
                            variables: original_variables,
                            lists,
                            arguments,
                        },
                        block,
                        next_pc,
                        output,
                        indent,
                    );
                } else {
                    generate_resumable_statement(
                        StatementContext {
                            target,
                            variables,
                            lists,
                            arguments,
                        },
                        block,
                        next_pc,
                        output,
                        indent,
                    );
                }
                if block.opcode == "data_changevariableby" {
                    if let Some(variable) = variable_field(block, "VARIABLE", variables)
                        .filter(|v| numeric_locals.contains(&v.slot))
                    {
                        line(
                            output,
                            indent,
                            &format!(
                                "scratch_typed_{0} = scratch_number_operand(scratch_typed_{0}) + scratch_change;",
                                variable.slot
                            ),
                        );
                        if deferred_change {
                            line(
                                output,
                                indent,
                                &format!("scratch_dirty_{} = true;", variable.slot),
                            );
                        }
                        if deferred_change && textual.contains(&variable.slot) {
                            line(
                                output,
                                indent,
                                &format!("scratch_text_{} = false;", variable.slot),
                            );
                        }
                    }
                    if let Some(variable) = variable_field(block, "VARIABLE", variables) {
                        cached_indexes.remove(&variable.slot);
                        cached_items.retain(|_, item| !item.index_slots.contains(&variable.slot));
                    }
                }
                if block.opcode == "data_replaceitemoflist" {
                    if let Some(list) = list_field(block, "LIST", lists) {
                        cached_items.retain(|_, item| item.list_slot != list.slot);
                    } else {
                        cached_items.clear();
                    }
                }
            }
        }
        current = block.next.as_deref();
    }
}

pub(super) fn emit_shared_list_copy(
    target: &ScratchTarget,
    block: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
    output: &mut String,
    indent: usize,
) -> Option<usize> {
    let variable = variable_field(block, "VARIABLE", variables)?;
    if variable.is_stage
        || variable.target != target.name
        || variable.slot < lists.values().map(|list| list.slot + 1).max().unwrap_or(0)
    {
        return None;
    }
    let Some(InputValue::BlockReference(id)) = block.inputs.get("VALUE").and_then(parse_input)
    else {
        return None;
    };
    let reporter = target.blocks.get(id)?;
    if reporter.opcode != "data_itemoflist" {
        return None;
    }
    let list = list_field(reporter, "LIST", lists)?;
    if !list.is_stage {
        return None;
    }
    let index = expression(
        target,
        reporter.inputs.get("INDEX"),
        variables,
        lists,
        arguments,
        &mut HashSet::new(),
    );
    line(
        output,
        indent,
        &format!("let scratch_list_index: ScratchValue = ({index}).to_owned().into();"),
    );
    line(
        output,
        indent,
        &format!(
            "let cached_index_{} = state.set_local_from_shared_list::<{}>({}, {}, scratch_list_index);",
            variable.slot, list.cache_numeric_reads, variable.slot, list.slot
        ),
    );
    Some(variable.slot)
}
