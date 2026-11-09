use super::*;

#[derive(Clone, Copy)]
struct ScriptBlockChainContext<'a> {
    target: &'a ScratchTarget,
    graph: &'a ScriptGraph,
    variables: &'a HashMap<String, VariableInfo>,
    lists: &'a HashMap<String, ListInfo>,
    arguments: &'a HashMap<String, String>,
    procedures: &'a [ProcedureSignature],
    procedure_entries: &'a HashMap<String, ProcedureEntry>,
    fast_math: bool,
    no_warp_timer: bool,
}

pub(in crate::rust) struct ResumableScriptOptions<'a> {
    pub(in crate::rust) function_name: &'a str,
    pub(in crate::rust) fast_math: bool,
    pub(in crate::rust) no_warp_timer: bool,
}

pub(in crate::rust) struct ResumableProcedurePlan {
    target_index: usize,
    graphs: Vec<(ProcedureSignature, ScriptGraph)>,
    entries: HashMap<String, ProcedureEntry>,
    reachable: HashSet<String>,
    numeric_call_procedures: HashSet<String>,
}

const RESUMABLE_PARTITION_THRESHOLD_BYTES: usize = 12 * 1024;
const RESUMABLE_PARTITION_TARGET_BYTES: usize = 12 * 1024;

#[derive(Clone)]
struct GeneratedStateArm {
    function_id: usize,
    pc: usize,
    source: String,
}

struct GeneratedStatePartition {
    function_id: usize,
    index: usize,
    first_pc: usize,
    last_pc: usize,
    arms: Vec<GeneratedStateArm>,
}

fn partition_generated_state_arms(
    source_arms: &[GeneratedStateArm],
) -> Option<Vec<GeneratedStatePartition>> {
    let total_bytes = source_arms
        .iter()
        .map(|arm| arm.source.len())
        .sum::<usize>();
    if total_bytes <= RESUMABLE_PARTITION_THRESHOLD_BYTES {
        return None;
    }

    let mut arms = source_arms.to_vec();
    arms.sort_by_key(|arm| (arm.function_id, arm.pc));
    let mut partitions = Vec::new();
    let mut current_arms: Vec<GeneratedStateArm> = Vec::new();
    let mut current_bytes = 0;
    let mut current_function = None;
    let mut partition_index = 0;

    for arm in arms {
        if !current_arms.is_empty()
            && (current_function != Some(arm.function_id)
                || current_bytes + arm.source.len() > RESUMABLE_PARTITION_TARGET_BYTES)
        {
            let first_pc = current_arms.first().unwrap().pc;
            let last_pc = current_arms.last().unwrap().pc;
            partitions.push(GeneratedStatePartition {
                function_id: current_function.unwrap(),
                index: partition_index,
                first_pc,
                last_pc,
                arms: std::mem::take(&mut current_arms),
            });
            current_bytes = 0;
            partition_index += 1;
        }
        if current_function != Some(arm.function_id) {
            current_function = Some(arm.function_id);
            partition_index = 0;
        }
        current_bytes += arm.source.len();
        current_arms.push(arm);

        // Keep an individually large PC arm intact and alone in its worker.
        if current_bytes >= RESUMABLE_PARTITION_TARGET_BYTES {
            let first_pc = current_arms.first().unwrap().pc;
            let last_pc = current_arms.last().unwrap().pc;
            partitions.push(GeneratedStatePartition {
                function_id: current_function.unwrap(),
                index: partition_index,
                first_pc,
                last_pc,
                arms: std::mem::take(&mut current_arms),
            });
            current_bytes = 0;
            partition_index += 1;
        }
    }
    if !current_arms.is_empty() {
        let first_pc = current_arms.first().unwrap().pc;
        let last_pc = current_arms.last().unwrap().pc;
        partitions.push(GeneratedStatePartition {
            function_id: current_function.unwrap(),
            index: partition_index,
            first_pc,
            last_pc,
            arms: current_arms,
        });
    }
    (partitions.len() > 1).then_some(partitions)
}

fn collect_graph_state_arms(
    context: ScriptBlockChainContext<'_>,
    function_id: usize,
) -> Vec<GeneratedStateArm> {
    let mut arms = Vec::new();
    let mut emitted_blocks = HashSet::new();
    for node in &context.graph.nodes {
        let (pc, mut source) = match node {
            ScriptNode::Block(id) => {
                let pc = context.graph.block_pcs[id];
                let mut source = String::new();
                generate_resumable_block_chain(context, id, &mut emitted_blocks, &mut source);
                (pc, source)
            }
            ScriptNode::LoopBoundary(id) => {
                let pc = context.graph.boundary_pcs[id];
                let mut source = String::new();
                let structured_repeat = structured_repeat_body(
                    context.target,
                    id,
                    context.procedures,
                    context.procedure_entries,
                )
                .is_some_and(|body| !body.has_procedure_calls);
                if !structured_repeat
                    && context
                        .target
                        .blocks
                        .get(id)
                        .and_then(|block| {
                            numeric_loop_plan(context.target, block, context.variables)
                        })
                        .is_none()
                {
                    generate_loop_boundary(
                        context.target,
                        id,
                        context.graph,
                        context.no_warp_timer,
                        &mut source,
                    );
                }
                (pc, source)
            }
        };
        if !source.is_empty() {
            arms.push(GeneratedStateArm {
                function_id,
                pc,
                source: std::mem::take(&mut source),
            });
        }
    }
    arms
}

fn collect_procedure_state_arms(
    target: &ScratchTarget,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    procedures: &[ProcedureSignature],
    plan: &ResumableProcedurePlan,
    fast_math: bool,
    no_warp_timer: bool,
) -> Vec<GeneratedStateArm> {
    let mut arms = Vec::new();
    for (procedure, graph) in &plan.graphs {
        let argument_accesses = procedure
            .argument_names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    name.clone(),
                    format!("script.frames.last().unwrap().arguments[{index}]"),
                )
            })
            .collect::<HashMap<_, _>>();
        let context = ScriptBlockChainContext {
            target,
            graph,
            variables,
            lists,
            arguments: &argument_accesses,
            procedures,
            procedure_entries: &plan.entries,
            fast_math,
            no_warp_timer,
        };
        arms.extend(collect_graph_state_arms(context, procedure.frame_id));
    }
    arms
}

fn emit_unpartitioned_procedure_arms(
    plan: &ResumableProcedurePlan,
    arms: &[GeneratedStateArm],
    output: &mut String,
) {
    for (procedure, _) in &plan.graphs {
        line(output, 3, &format!("{} => match pc {{", procedure.frame_id));
        for arm in arms
            .iter()
            .filter(|arm| arm.function_id == procedure.frame_id)
        {
            output.push_str(&arm.source);
        }
        line(output, 4, "_ => { script.frames.pop(); continue; },");
        line(output, 3, "},");
    }
}

fn wrap_state_arm_status_returns(source: &str) -> String {
    let mut output = Vec::with_capacity(source.len() + 64);
    let mut cursor = 0;
    let mut in_string = false;
    let mut escaped = false;
    let bytes = source.as_bytes();
    while cursor < bytes.len() {
        if in_string {
            let byte = bytes[cursor];
            output.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            cursor += 1;
            continue;
        }
        if bytes[cursor] == b'"' {
            in_string = true;
            output.push(b'"');
            cursor += 1;
            continue;
        }
        const PREFIX: &[u8] = b"return ScriptStatus::";
        if bytes[cursor..].starts_with(PREFIX) {
            output.extend_from_slice(b"return Some(ScriptStatus::");
            cursor += PREFIX.len();
            let mut depth = 0usize;
            let mut expression_in_string = false;
            let mut expression_escaped = false;
            while cursor < bytes.len() {
                let byte = bytes[cursor];
                output.push(byte);
                if expression_in_string {
                    if expression_escaped {
                        expression_escaped = false;
                    } else if byte == b'\\' {
                        expression_escaped = true;
                    } else if byte == b'"' {
                        expression_in_string = false;
                    }
                } else if byte == b'"' {
                    expression_in_string = true;
                } else if matches!(byte, b'(' | b'{' | b'[') {
                    depth += 1;
                } else if matches!(byte, b')' | b'}' | b']') {
                    depth = depth.saturating_sub(1);
                } else if byte == b';' && depth == 0 {
                    output.insert(output.len() - 1, b')');
                    cursor += 1;
                    break;
                }
                cursor += 1;
            }
            continue;
        }
        output.push(bytes[cursor]);
        cursor += 1;
    }
    String::from_utf8(output).expect("generated Rust arm remains valid UTF-8")
}

fn partition_worker_name(function_name: &str, partition: &GeneratedStatePartition) -> String {
    format!(
        "step_{function_name}_partition_{}_{}",
        partition.function_id, partition.index
    )
}

fn partition_pc_guard(partition: &GeneratedStatePartition) -> String {
    if partition.first_pc == 0 {
        format!("pc <= {}", partition.last_pc)
    } else {
        format!(
            "pc >= {} && pc <= {}",
            partition.first_pc, partition.last_pc
        )
    }
}

fn emit_partition_worker(
    function_name: &str,
    partition: &GeneratedStatePartition,
    procedure_state: bool,
    output: &mut String,
) {
    let worker_name = partition_worker_name(function_name, partition);
    line(output, 0, "#[inline(never)]");
    line(
        output,
        0,
        "#[allow(dead_code, unused_variables, unused_mut, unused_assignments, unreachable_code)]",
    );
    line(
        output,
        0,
        &format!(
            "fn {worker_name}(script: &mut ScriptState, state: &mut ScratchState, clock: &dyn Clock) -> Option<ScriptStatus> {{"
        ),
    );
    line(output, 1, "loop {");
    line(
        output,
        2,
        "let Some(frame) = script.frames.last() else { return Some(ScriptStatus::Complete); };",
    );
    line(
        output,
        2,
        &format!(
            "if frame.function != {} {{ return None; }}",
            partition.function_id
        ),
    );
    line(output, 2, "let pc = frame.pc;");
    line(output, 2, "match pc {");
    for arm in &partition.arms {
        output.push_str(&wrap_state_arm_status_returns(&arm.source));
    }
    let pc_guard = partition_pc_guard(partition);
    if procedure_state {
        line(
            output,
            3,
            &format!("_ if {pc_guard} => {{ script.frames.pop(); return None; }},"),
        );
    } else {
        line(
            output,
            3,
            &format!("_ if {pc_guard} => return Some(ScriptStatus::Complete),"),
        );
    }
    line(output, 3, "_ => return None,");
    line(output, 2, "}");
    line(output, 1, "}");
    line(output, 0, "}\n");
}

fn emit_partition_pc_dispatch(
    output: &mut String,
    function_name: &str,
    partitions: &[GeneratedStatePartition],
    function_id: usize,
    procedure_state: bool,
    indent: usize,
) {
    for partition in partitions
        .iter()
        .filter(|partition| partition.function_id == function_id)
    {
        let worker_name = partition_worker_name(function_name, partition);
        line(
            output,
            indent,
            &format!(
                "{}..={} => match {worker_name}(script, state, clock) {{ Some(status) => return status, None => continue, }},",
                partition.first_pc, partition.last_pc
            ),
        );
    }
    if procedure_state {
        line(output, indent, "_ => { script.frames.pop(); continue; },");
    } else {
        line(output, indent, "_ => return ScriptStatus::Complete,");
    }
}

impl ResumableProcedurePlan {
    pub(in crate::rust) fn has_resumable_procedures(&self) -> bool {
        !self.graphs.is_empty()
    }

    pub(in crate::rust) fn reachable_ids(&self) -> &HashSet<String> {
        &self.reachable
    }

    pub(in crate::rust) fn numeric_call_procedure_ids(&self) -> &HashSet<String> {
        &self.numeric_call_procedures
    }
}

pub(in crate::rust) fn prepare_resumable_procedures(
    target: &ScratchTarget,
    script_starts: &[Option<&str>],
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    procedures: &[ProcedureSignature],
    target_index: usize,
) -> ResumableProcedurePlan {
    let calls_in = |graph: &ScriptGraph, call_block_ids: &mut HashSet<String>| {
        graph
            .nodes
            .iter()
            .filter_map(|node| match node {
                ScriptNode::Block(id) => target.blocks.get(id).map(|block| (id, block)),
                ScriptNode::LoopBoundary(_) => None,
            })
            .filter(|(_, block)| block.opcode == "procedures_call")
            .filter_map(|(id, block)| {
                call_block_ids.insert(id.clone());
                procedure_for_call(block, procedures)
            })
            .map(|procedure| procedure.definition_id.clone())
            .collect::<Vec<_>>()
    };
    let mut pending = Vec::new();
    let mut call_block_ids = HashSet::new();
    for first in script_starts {
        let mut graph = ScriptGraph::new();
        graph.add_chain(target, *first, usize::MAX);
        pending.extend(calls_in(&graph, &mut call_block_ids));
    }

    // Compute the union once for the target. Every script frame uses this
    // shared, statically generated procedure machine, so a common procedure
    // body is emitted once even when many event hats can call it.
    let mut reachable = HashSet::new();
    while let Some(id) = pending.pop() {
        if !reachable.insert(id.clone()) {
            continue;
        }
        if let Some(definition) = target.blocks.get(&id) {
            let mut graph = ScriptGraph::new();
            graph.add_chain(target, definition.next.as_deref(), usize::MAX);
            pending.extend(calls_in(&graph, &mut call_block_ids));
        }
    }

    let direct_warp_eligibility = direct_warp_eligibility(target, procedures);
    let mut graphs = Vec::new();
    let mut entries = HashMap::new();
    for procedure in procedures
        .iter()
        .filter(|procedure| reachable.contains(&procedure.definition_id))
    {
        let Some(definition) = target.blocks.get(&procedure.definition_id) else {
            continue;
        };
        let mut graph = ScriptGraph::new();
        graph.add_chain(target, definition.next.as_deref(), usize::MAX);
        let start_pc = definition
            .next
            .as_deref()
            .and_then(|id| graph.block_pcs.get(id).copied())
            .unwrap_or(usize::MAX);
        let direct_path_name = direct_eligible(target, procedure, procedures)
            .then(|| direct_name(target_index, procedure.frame_id));
        let direct_path_is_none = direct_path_name.is_none();
        let direct_warp_path = ((direct_path_is_none
            || direct_warp_contains_stop(target, procedure))
            && direct_warp_eligibility
                .get(&procedure.definition_id)
                .copied()
                .unwrap_or(false))
        .then(|| direct_warp_name(target_index, procedure.frame_id));
        let numeric_direct = direct_path_name.is_some()
            && super::super::direct_numeric::generate_numeric_direct(
                target,
                procedure,
                variables,
                lists,
                &direct_name(target_index, procedure.frame_id),
                true,
            )
            .is_some();
        entries.insert(
            procedure.definition_id.clone(),
            ProcedureEntry {
                function_id: procedure.frame_id,
                start_pc,
                repeat_count: graph.frame_state_count(),
                direct_name: direct_path_name,
                direct_warp_name: direct_warp_path,
                numeric_direct,
            },
        );
        graphs.push((procedure.clone(), graph));
    }
    let mut numeric_call_procedures = HashSet::new();
    for block_id in call_block_ids {
        let Some(block) = target.blocks.get(&block_id) else {
            continue;
        };
        let Some(procedure) = procedure_for_call(block, procedures) else {
            continue;
        };
        let Some(entry) = entries.get(&procedure.definition_id) else {
            continue;
        };
        let (argument_caches, _) =
            cached_procedure_indexes(target, block, procedure, entry, variables);
        if numeric_list_call_candidate(target, block, procedure, entry, &argument_caches) {
            numeric_call_procedures.insert(procedure.definition_id.clone());
        }
    }
    ResumableProcedurePlan {
        target_index,
        graphs,
        entries,
        reachable,
        numeric_call_procedures,
    }
}

pub(in crate::rust) fn generate_resumable_procedure_step(
    target: &ScratchTarget,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    procedures: &[ProcedureSignature],
    plan: &ResumableProcedurePlan,
    fast_math: bool,
    no_warp_timer: bool,
    share_procedures: bool,
    output: &mut String,
) {
    if plan.graphs.is_empty() || !share_procedures {
        return;
    }

    let procedure_arms = collect_procedure_state_arms(
        target,
        variables,
        lists,
        procedures,
        plan,
        fast_math,
        no_warp_timer,
    );
    if let Some(partitions) = partition_generated_state_arms(&procedure_arms) {
        line(
            output,
            0,
            "#[allow(dead_code, unused_variables, unused_mut, unused_assignments, unreachable_code)]",
        );
        line(
            output,
            0,
            &format!(
                "fn step_procedures_{}(script: &mut ScriptState, state: &mut ScratchState, clock: &dyn Clock, returned_to_script: &mut bool) -> ScriptStatus {{",
                plan.target_index
            ),
        );
        line(output, 1, "loop {");
        line(
            output,
            2,
            "let (function, pc) = match script.frames.last() {",
        );
        line(output, 3, "Some(frame) => (frame.function, frame.pc),");
        line(output, 3, "None => return ScriptStatus::Complete,");
        line(output, 2, "};");
        line(output, 2, "match function {");
        line(
            output,
            3,
            "0 => { *returned_to_script = true; return ScriptStatus::Complete; },",
        );
        for (procedure, _) in &plan.graphs {
            line(output, 3, &format!("{} => match pc {{", procedure.frame_id));
            emit_partition_pc_dispatch(
                output,
                &format!("procedures_{}", plan.target_index),
                &partitions,
                procedure.frame_id,
                true,
                4,
            );
            line(output, 3, "},");
        }
        line(output, 3, "_ => return ScriptStatus::Complete,");
        line(output, 2, "}");
        line(output, 1, "}");
        line(output, 0, "}\n");
        for partition in &partitions {
            emit_partition_worker(
                &format!("procedures_{}", plan.target_index),
                partition,
                true,
                output,
            );
        }
        return;
    }

    line(
        output,
        0,
        "#[allow(dead_code, unused_variables, unused_mut, unused_assignments, unreachable_code)]",
    );
    line(
        output,
        0,
        &format!(
            "fn step_procedures_{}(script: &mut ScriptState, state: &mut ScratchState, clock: &dyn Clock, returned_to_script: &mut bool) -> ScriptStatus {{",
            plan.target_index
        ),
    );
    line(output, 1, "loop {");
    line(
        output,
        2,
        "let (function, pc) = match script.frames.last() {",
    );
    line(output, 3, "Some(frame) => (frame.function, frame.pc),");
    line(output, 3, "None => return ScriptStatus::Complete,");
    line(output, 2, "};");
    line(output, 2, "match function {");
    line(
        output,
        3,
        "0 => { *returned_to_script = true; return ScriptStatus::Complete; },",
    );
    emit_unpartitioned_procedure_arms(plan, &procedure_arms, output);
    line(output, 3, "_ => return ScriptStatus::Complete,");
    line(output, 2, "}");
    line(output, 1, "}");
    line(output, 0, "}\n");
}

pub(in crate::rust) fn generate_resumable_script(
    target: &ScratchTarget,
    first: Option<&str>,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    procedures: &[ProcedureSignature],
    procedure_plan: &ResumableProcedurePlan,
    share_procedures: bool,
    options: ResumableScriptOptions<'_>,
    output: &mut String,
) -> (usize, usize) {
    let ResumableScriptOptions {
        function_name,
        fast_math,
        no_warp_timer,
    } = options;
    let mut graph = ScriptGraph::new();
    graph.add_chain(target, first, usize::MAX);
    let start_pc = first
        .and_then(|id| graph.block_pcs.get(id).copied())
        .unwrap_or(usize::MAX);

    let no_arguments = HashMap::new();
    let script_context = ScriptBlockChainContext {
        target,
        graph: &graph,
        variables,
        lists,
        arguments: &no_arguments,
        procedures,
        procedure_entries: &procedure_plan.entries,
        fast_math,
        no_warp_timer,
    };
    let mut state_arms = collect_graph_state_arms(script_context, 0);
    if !share_procedures {
        state_arms.extend(collect_procedure_state_arms(
            target,
            variables,
            lists,
            procedures,
            procedure_plan,
            fast_math,
            no_warp_timer,
        ));
    }
    let partitions = partition_generated_state_arms(&state_arms);
    let has_inline_procedures = !share_procedures && !procedure_plan.graphs.is_empty();

    line(
        output,
        0,
        "#[allow(dead_code, unused_variables, unused_mut, unused_assignments, unreachable_code)]",
    );
    line(
        output,
        0,
        &format!(
            "fn step_{function_name}(script: &mut ScriptState, state: &mut ScratchState, clock: &dyn Clock) -> ScriptStatus {{"
        ),
    );
    line(output, 1, "loop {");
    if share_procedures && !procedure_plan.graphs.is_empty() {
        line(
            output,
            2,
            "if script.frames.last().is_some_and(|frame| frame.function != 0) {",
        );
        line(output, 3, "let mut returned_to_script = false;");
        line(
            output,
            3,
            &format!(
                "let status = step_procedures_{}(script, state, clock, &mut returned_to_script);",
                procedure_plan.target_index
            ),
        );
        line(output, 3, "if !returned_to_script { return status; }");
        line(output, 3, "continue;");
        line(output, 2, "}");
    }
    if partitions.is_some() {
        line(
            output,
            2,
            "let (function, pc) = match script.frames.last() {",
        );
        line(output, 3, "Some(frame) => (frame.function, frame.pc),");
        line(output, 3, "None => return ScriptStatus::Complete,");
        line(output, 2, "};");
        line(output, 2, "match function {");
        line(output, 3, "0 => match pc {");
        emit_partition_pc_dispatch(
            output,
            function_name,
            partitions.as_ref().unwrap(),
            0,
            false,
            4,
        );
        line(output, 3, "},");
        if has_inline_procedures {
            for (procedure, _) in &procedure_plan.graphs {
                line(output, 3, &format!("{} => match pc {{", procedure.frame_id));
                emit_partition_pc_dispatch(
                    output,
                    function_name,
                    partitions.as_ref().unwrap(),
                    procedure.frame_id,
                    true,
                    4,
                );
                line(output, 3, "},");
            }
        }
        line(output, 3, "_ => return ScriptStatus::Complete,");
        line(output, 2, "}");
    } else if has_inline_procedures {
        line(
            output,
            2,
            "let (function, pc) = match script.frames.last() {",
        );
        line(output, 3, "Some(frame) => (frame.function, frame.pc),");
        line(output, 3, "None => return ScriptStatus::Complete,");
        line(output, 2, "};");
        line(output, 2, "match function {");
        line(output, 3, "0 => match pc {");
    } else {
        line(output, 2, "let pc = match script.frames.last() {");
        line(output, 3, "Some(frame) => frame.pc,");
        line(output, 3, "None => return ScriptStatus::Complete,");
        line(output, 2, "};");
        line(output, 2, "match pc {");
    }
    if partitions.is_none() && has_inline_procedures {
        for arm in state_arms.iter().filter(|arm| arm.function_id == 0) {
            output.push_str(&arm.source);
        }
        line(output, 3, "_ => return ScriptStatus::Complete,");
        line(output, 3, "},");
        emit_unpartitioned_procedure_arms(procedure_plan, &state_arms, output);
        line(output, 3, "_ => return ScriptStatus::Complete,");
        line(output, 2, "}");
    } else if partitions.is_none() {
        for arm in state_arms.iter().filter(|arm| arm.function_id == 0) {
            output.push_str(&arm.source);
        }
        line(output, 3, "_ => return ScriptStatus::Complete,");
        line(output, 2, "}");
    }
    line(output, 1, "}");
    line(output, 0, "}\n");
    if let Some(partitions) = &partitions {
        for partition in partitions {
            emit_partition_worker(function_name, partition, partition.function_id != 0, output);
        }
    }
    (start_pc, graph.frame_state_count())
}

fn structured_repeat_body(
    target: &ScratchTarget,
    loop_id: &str,
    procedures: &[ProcedureSignature],
    entries: &HashMap<String, ProcedureEntry>,
) -> Option<StructuredBody> {
    let block = target.blocks.get(loop_id)?;
    if block.opcode != "control_repeat" {
        return None;
    }
    let Some(InputValue::BlockReference(first)) =
        block.inputs.get("SUBSTACK").and_then(parse_input)
    else {
        return None;
    };
    structured_loop_body(target, loop_id, first, procedures, entries)
}

fn structured_if_body(
    target: &ScratchTarget,
    owner_id: &str,
    block: &ScratchBlock,
    procedures: &[ProcedureSignature],
    entries: &HashMap<String, ProcedureEntry>,
) -> Option<StructuredBody> {
    let mut blocks = HashSet::new();
    let mut has_procedure_calls = false;
    for name in ["SUBSTACK", "SUBSTACK2"] {
        let Some(InputValue::BlockReference(first)) = block.inputs.get(name).and_then(parse_input)
        else {
            continue;
        };
        let body = structured_loop_body(target, owner_id, first, procedures, entries)?;
        blocks.extend(body.blocks);
        has_procedure_calls |= body.has_procedure_calls;
    }
    Some(StructuredBody {
        blocks,
        has_procedure_calls,
    })
}

fn generate_resumable_block(context: ScriptBlockChainContext<'_>, id: &str, output: &mut String) {
    let ScriptBlockChainContext {
        target,
        graph,
        variables,
        lists,
        arguments,
        procedures,
        procedure_entries,
        no_warp_timer,
        ..
    } = context;
    let pc = graph.block_pcs[id];
    let indent = 4;
    line(output, indent, &format!("{pc} => {{"));
    let Some(block) = target.blocks.get(id) else {
        line(output, indent + 1, "script.frames.pop(); continue;");
        line(output, indent, "},");
        return;
    };
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
            let branch_pc = |input_name: &str| {
                block
                    .inputs
                    .get(input_name)
                    .and_then(parse_input)
                    .and_then(|input| match input {
                        InputValue::BlockReference(id) => graph.block_pcs.get(id).copied(),
                        _ => None,
                    })
                    .unwrap_or(next_pc)
            };
            line(output, indent + 1, &format!("if {condition} {{"));
            line(
                output,
                indent + 2,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    branch_pc("SUBSTACK")
                ),
            );
            line(output, indent + 1, "} else {");
            line(
                output,
                indent + 2,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    if block.opcode == "control_if_else" {
                        branch_pc("SUBSTACK2")
                    } else {
                        next_pc
                    }
                ),
            );
            line(output, indent + 1, "}");
            line(output, indent + 1, "continue;");
        }
        "control_repeat" => {
            let times = expression(
                target,
                block.inputs.get("TIMES"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            let slot = graph.repeat_slots[id];
            let boundary = graph.boundary_pcs[id];
            let branch_pc = block
                .inputs
                .get("SUBSTACK")
                .and_then(parse_input)
                .and_then(|input| match input {
                    InputValue::BlockReference(id) => graph.block_pcs.get(id).copied(),
                    _ => None,
                });
            let fused = generate_warp_repeat_list_append(
                target,
                block,
                graph,
                variables,
                lists,
                arguments,
                &times,
                slot,
                next_pc,
                output,
                indent,
                no_warp_timer,
            );
            if !fused {
                line(
                    output,
                    4,
                    &format!("let repeat_count = scratch_round({times}).max(0.0) as usize;"),
                );
                line(
                    output,
                    4,
                    &format!(
                        "script.frames.last_mut().unwrap().repeat_counts[{slot}] = repeat_count;"
                    ),
                );
                line(output, indent + 1, "if repeat_count == 0 {");
                line(
                    output,
                    indent + 2,
                    &format!(
                        "script.frames.last_mut().unwrap().pc = {};",
                        pc_literal(next_pc)
                    ),
                );
                line(output, indent + 1, "} else {");
                line(
                    output,
                    indent + 2,
                    &format!(
                        "script.frames.last_mut().unwrap().pc = {};",
                        branch_pc.unwrap_or(boundary)
                    ),
                );
                if branch_pc.is_none() {
                    line(output, indent + 2, "continue;");
                }
                line(output, indent + 1, "}");
                line(output, indent + 1, "continue;");
            }
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
            let continue_when = if block.opcode == "control_repeat_until" {
                format!("!({condition})")
            } else {
                condition
            };
            let branch_pc = block
                .inputs
                .get("SUBSTACK")
                .and_then(parse_input)
                .and_then(|input| match input {
                    InputValue::BlockReference(id) => graph.block_pcs.get(id).copied(),
                    _ => None,
                });
            line(output, indent + 1, &format!("if {continue_when} {{"));
            if let Some(branch_pc) = branch_pc {
                line(
                    output,
                    indent + 2,
                    &format!("script.frames.last_mut().unwrap().pc = {branch_pc};"),
                );
            } else {
                line(
                    output,
                    indent + 2,
                    &format!(
                        "script.frames.last_mut().unwrap().pc = {};",
                        graph.boundary_pcs[id]
                    ),
                );
            }
            line(output, indent + 2, "continue;");
            line(output, indent + 1, "}");
            line(
                output,
                indent + 2,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(output, indent + 1, "continue;");
        }
        "control_forever" => {
            let branch_pc = block
                .inputs
                .get("SUBSTACK")
                .and_then(parse_input)
                .and_then(|input| match input {
                    InputValue::BlockReference(id) => graph.block_pcs.get(id).copied(),
                    _ => None,
                })
                .unwrap_or(graph.boundary_pcs[id]);
            line(
                output,
                indent + 1,
                &format!("script.frames.last_mut().unwrap().pc = {branch_pc};"),
            );
            line(output, indent + 1, "continue;");
        }
        "control_wait_until" => {
            let condition = boolean_input(
                target,
                block,
                "CONDITION",
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(output, indent + 1, &format!("if !({condition}) {{"));
            line(
                output,
                indent + 2,
                &format!("script.frames.last_mut().unwrap().pc = {pc};"),
            );
            line(
                output,
                indent + 2,
                "if script.yield_at_safe_point(clock) { return ScriptStatus::Yield; }",
            );
            line(output, indent + 2, "continue;");
            line(output, indent + 1, "}");
            line(
                output,
                indent + 1,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(output, indent + 1, "continue;");
        }
        "control_wait" => {
            let duration = expression(
                target,
                block.inputs.get("DURATION"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent + 1,
                &format!(
                    "let duration_ms = (scratch_to_number({duration}).max(0.0) * 1000.0).ceil() as u64; let wait_deadline = clock.now_millis().saturating_add(duration_ms);"
                ),
            );
            line(
                output,
                indent + 1,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(
                output,
                indent + 1,
                "return ScriptStatus::WaitUntil(wait_deadline);",
            );
        }
        "sound_playuntildone" => {
            let selector = owned_input(
                target,
                block.inputs.get("SOUND_MENU"),
                variables,
                lists,
                arguments,
            );
            line(
                output,
                indent + 1,
                &format!("let scratch_sound_selector: ScratchValue = {selector};"),
            );
            line(
                output,
                indent + 1,
                "let sound_wait = state.start_sound(scratch_sound_selector, clock.now_millis());",
            );
            line(
                output,
                indent + 1,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(
                output,
                indent + 1,
                "if let Some(sound_wait) = sound_wait { return ScriptStatus::WaitForSignal(sound_wait); }",
            );
            line(output, indent + 1, "continue;");
        }
        "sound_seteffectto" | "sound_changeeffectby" => {
            let effect = field_value(block, "EFFECT")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let effect = match effect.as_str() {
                "pitch" => "Some(ScratchSoundEffect::Pitch)",
                "pan" => "Some(ScratchSoundEffect::Pan)",
                _ => "None",
            };
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
                indent + 1,
                &format!("if let Some(scratch_sound_effect) = {effect} {{"),
            );
            line(
                output,
                indent + 2,
                &format!(
                    "state.set_sound_effect(scratch_sound_effect, scratch_to_number({value}), {}, clock.now_millis());",
                    block.opcode == "sound_changeeffectby"
                ),
            );
            line(
                output,
                indent + 2,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(output, indent + 2, "return ScriptStatus::Yield;");
            line(output, indent + 1, "} else {");
            line(
                output,
                indent + 2,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(output, indent + 2, "continue;");
            line(output, indent + 1, "}");
        }
        "sound_setvolumeto" | "sound_changevolumeby" => {
            let value = expression(
                target,
                block.inputs.get("VOLUME"),
                variables,
                lists,
                arguments,
                &mut HashSet::new(),
            );
            line(
                output,
                indent + 1,
                &format!(
                    "state.set_sound_volume(scratch_to_number({value}), {}, clock.now_millis());",
                    block.opcode == "sound_changevolumeby"
                ),
            );
            line(
                output,
                indent + 1,
                "if REFRESH_SOUND_VOLUME_MONITOR { state.request_presentation(); }",
            );
            line(
                output,
                indent + 1,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(output, indent + 1, "return ScriptStatus::Yield;");
        }
        "looks_switchbackdropto" | "looks_nextbackdrop" => {
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
                indent + 1,
            );
            // redraw ends the scheduler round after runnable scripts finish their turn.
            line(
                output,
                indent + 1,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(output, indent + 1, "continue;");
        }
        "motion_glideto" | "motion_glidesecstoxy" => generate_resumable_statement(
            StatementContext {
                target,
                variables,
                lists,
                arguments,
            },
            block,
            next_pc,
            output,
            indent + 1,
        ),
        "procedures_call" => generate_resumable_procedure_call(
            ProcedureCallContext {
                target,
                variables,
                lists,
                arguments,
                procedures,
                entries: procedure_entries,
                no_warp_timer,
            },
            block,
            next_pc,
            ProcedureCallOutput {
                output,
                indent: indent + 1,
                emit_continue: true,
                reusable_indexes: None,
                reusable_items: None,
            },
        ),
        _ => {
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
                indent + 1,
            );
            line(
                output,
                indent + 1,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(next_pc)
                ),
            );
            line(output, indent + 1, "continue;");
        }
    }
    line(output, indent, "},");
}

// fuse a warp repeat whose only body action appends to one list, checking the watchdog by chunk.
#[allow(clippy::too_many_arguments)]
pub(super) fn generate_warp_repeat_list_append(
    target: &ScratchTarget,
    repeat: &ScratchBlock,
    graph: &ScriptGraph,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
    times: &str,
    repeat_slot: usize,
    next_pc: usize,
    output: &mut String,
    indent: usize,
    no_warp_timer: bool,
) -> bool {
    let Some(InputValue::BlockReference(body_id)) =
        repeat.inputs.get("SUBSTACK").and_then(parse_input)
    else {
        return false;
    };
    let Some(body) = target.blocks.get(body_id) else {
        return false;
    };
    if body.opcode != "data_addtolist" || body.next.is_some() {
        return false;
    }
    let Some(list) = list_field(body, "LIST", lists) else {
        return false;
    };
    let Some(body_pc) = graph.block_pcs.get(body_id).copied() else {
        return false;
    };
    let item = owned_input(target, body.inputs.get("ITEM"), variables, lists, arguments);
    let item_reads_state = item.contains("state.");
    let numeric_random =
        body.inputs
            .get("ITEM")
            .and_then(parse_input)
            .and_then(|input| match input {
                InputValue::BlockReference(id) => target
                    .blocks
                    .get(id)
                    .and_then(|block| constant_random_bounds(target, block)),
                _ => None,
            });
    let numeric_random_is_finite = numeric_random.is_some_and(|(from, to, _)| {
        let (low, high) = if from <= to { (from, to) } else { (to, from) };
        (high - low).is_finite()
    });

    line(
        output,
        indent + 1,
        "if script.frames.last().unwrap().warp_mode {",
    );
    line(
        output,
        indent + 2,
        &format!("let repeat_count = scratch_round({times}).max(0.0) as usize;"),
    );
    line(
        output,
        indent + 2,
        &format!("script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = repeat_count;"),
    );
    if !item_reads_state && let Some((from, to, integer_bounds)) = numeric_random {
        let append_method = if numeric_random_is_finite {
            "add_finite_numbers"
        } else {
            "add_numbers"
        };
        line(output, indent + 2, "let mut scratch_list_changed = false;");
        line(
            output,
            indent + 2,
            "let yielded_repeat = with_scratch_random(|scratch_rng| {",
        );
        line(
            output,
            indent + 3,
            &format!(
                "state.list_mut({}).with_generated_finite_number_appender(repeat_count, |scratch_list_append| {{",
                list.slot
            ),
        );
        line(
            output,
            indent + 4,
            "let mut repeat_remaining = repeat_count;",
        );
        line(output, indent + 4, "while repeat_remaining > 0 {");
        line(
            output,
            indent + 5,
            "let repeat_chunk = repeat_remaining.min(1024);",
        );
        line(
            output,
            indent + 5,
            &format!(
                "scratch_list_changed |= scratch_list_append.{append_method}(repeat_chunk, || scratch_rng.between_with_integer_bounds({from:?}_f64, {to:?}_f64, {integer_bounds}));"
            ),
        );
        line(output, indent + 5, "repeat_remaining -= repeat_chunk;");
        if !no_warp_timer {
            line(
                output,
                indent + 5,
                "if script.warp_budget.expired_now(clock) {",
            );
            line(output, indent + 6, "return Some(repeat_remaining);");
            line(output, indent + 5, "}");
        }
        line(output, indent + 4, "}");
        line(output, indent + 4, "None");
        line(output, indent + 3, "})");
        line(output, indent + 2, "});");
        line(
            output,
            indent + 2,
            "if let Some(repeat_remaining) = yielded_repeat {",
        );
        line(
            output,
            indent + 3,
            &format!(
                "script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = repeat_remaining;"
            ),
        );
        line(
            output,
            indent + 3,
            &format!(
                "script.frames.last_mut().unwrap().pc = if repeat_remaining > 0 {{ {body_pc} }} else {{ {} }};",
                pc_literal(next_pc)
            ),
        );
        line(
            output,
            indent + 3,
            &format!(
                "state.list_changed_result({}, scratch_list_changed);",
                list.slot
            ),
        );
        line(output, indent + 3, "return ScriptStatus::Yield;");
        line(output, indent + 2, "}");
        line(
            output,
            indent + 2,
            &format!("script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = 0;"),
        );
        line(
            output,
            indent + 2,
            &format!(
                "state.list_changed_result({}, scratch_list_changed);",
                list.slot
            ),
        );
    } else {
        line(
            output,
            indent + 2,
            "let mut repeat_remaining = repeat_count;",
        );
        line(output, indent + 2, "while repeat_remaining > 0 {");
        line(
            output,
            indent + 3,
            "let repeat_chunk = repeat_remaining.min(1024);",
        );
        line(output, indent + 3, "let mut scratch_list_changed = false;");
        if item_reads_state {
            line(output, indent + 3, "for _ in 0..repeat_chunk {");
            line(
                output,
                indent + 4,
                &format!("let scratch_list_item: ScratchValue = {item};"),
            );
            line(
                output,
                indent + 4,
                &format!(
                    "scratch_list_changed |= state.list_mut({}).add(scratch_list_item);",
                    list.slot
                ),
            );
            line(output, indent + 3, "}");
        } else {
            line(
                output,
                indent + 3,
                &format!(
                    "scratch_list_changed = state.list_mut({}).add_generated(repeat_chunk, || {{ let scratch_list_item: ScratchValue = {item}; scratch_list_item }});",
                    list.slot
                ),
            );
        }
        line(output, indent + 3, "repeat_remaining -= repeat_chunk;");
        line(
            output,
            indent + 3,
            &format!(
                "script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = repeat_remaining;"
            ),
        );
        line(
            output,
            indent + 3,
            &format!(
                "state.list_changed_result({}, scratch_list_changed);",
                list.slot
            ),
        );
        if !no_warp_timer {
            line(
                output,
                indent + 3,
                "if script.warp_budget.expired_now(clock) {",
            );
            line(
                output,
                indent + 4,
                &format!(
                    "script.frames.last_mut().unwrap().pc = if repeat_remaining > 0 {{ {body_pc} }} else {{ {} }};",
                    pc_literal(next_pc)
                ),
            );
            line(output, indent + 4, "return ScriptStatus::Yield;");
            line(output, indent + 3, "}");
        }
        line(output, indent + 2, "}");
    }
    line(
        output,
        indent + 2,
        &format!(
            "script.frames.last_mut().unwrap().pc = {};",
            pc_literal(next_pc)
        ),
    );
    line(output, indent + 2, "continue;");
    line(output, indent + 1, "} else {");
    line(
        output,
        indent + 2,
        &format!("let repeat_count = scratch_round({times}).max(0.0) as usize;"),
    );
    line(
        output,
        indent + 2,
        &format!("script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = repeat_count;"),
    );
    line(output, indent + 2, "if repeat_count == 0 {");
    line(
        output,
        indent + 3,
        &format!(
            "script.frames.last_mut().unwrap().pc = {};",
            pc_literal(next_pc)
        ),
    );
    line(output, indent + 2, "} else {");
    line(
        output,
        indent + 3,
        &format!("script.frames.last_mut().unwrap().pc = {body_pc};"),
    );
    line(output, indent + 2, "}");
    line(output, indent + 2, "continue;");
    line(output, indent + 1, "}");
    true
}

// A loop body containing only synchronous statements and direct leaf calls
// can execute as structured Rust between the existing loop safe points. Each
// call still records its own continuation before a possible budget yield.
fn generate_resumable_block_chain(
    context: ScriptBlockChainContext<'_>,
    id: &str,
    emitted_blocks: &mut HashSet<String>,
    output: &mut String,
) {
    let ScriptBlockChainContext {
        target,
        graph,
        variables,
        lists,
        arguments,
        procedures,
        procedure_entries,
        fast_math,
        no_warp_timer,
    } = context;
    if emitted_blocks.contains(id) {
        return;
    }
    let Some(block) = target.blocks.get(id) else {
        generate_resumable_block(context, id, output);
        return;
    };
    let enclosing_loop = graph.nodes.iter().find_map(|node| match node {
        ScriptNode::Block(loop_id) => target.blocks.get(loop_id).and_then(|candidate| {
            (matches!(candidate.opcode.as_str(), "control_repeat" | "control_repeat_until" | "control_while" | "control_forever")
                && matches!(candidate.inputs.get("SUBSTACK").and_then(parse_input), Some(InputValue::BlockReference(body)) if body == id))
                .then_some((loop_id.as_str(), candidate))
        }),
        ScriptNode::LoopBoundary(_) => None,
    });
    let structured_loop_blocks = enclosing_loop.and_then(|(loop_id, _)| {
        structured_loop_body(target, loop_id, id, procedures, procedure_entries)
    });
    if let (Some((loop_id, enclosing)), Some(body_plan)) = (enclosing_loop, structured_loop_blocks)
    {
        if !body_plan.has_procedure_calls {
            emitted_blocks.extend(body_plan.blocks);
        }
        line(output, 4, &format!("{} => {{", graph.block_pcs[id]));
        let repeat = enclosing.opcode == "control_repeat";
        let (numeric, deferred, textual) = if repeat {
            structured_numeric_locals(target, id, variables, true, false)
        } else {
            (Vec::new(), HashSet::new(), HashSet::new())
        };
        let mut needed = HashSet::new();
        collect_region_entry_reads(
            target,
            Some(id),
            variables,
            &mut HashSet::new(),
            &mut needed,
            false,
        );
        let numeric_slots = numeric.iter().map(|v| v.slot).collect::<HashSet<_>>();
        let dynamic = if repeat {
            structured_dynamic_locals(target, id, variables, &numeric_slots)
        } else {
            Vec::new()
        };
        let has_locals = !numeric.is_empty() || !dynamic.is_empty();
        let has_visible_mutation = deferred.iter().any(|slot| {
            variables
                .values()
                .any(|variable| variable.slot == *slot && variable.monitor_visible)
        }) || dynamic.iter().any(|variable| variable.monitor_visible);
        // The fast branch can never pass can_defer_variable_writes when a
        // written variable has a visible monitor. Omit that unreachable copy;
        // the generic path already preserves every monitor update.
        let has_optimized_locals = has_locals && !has_visible_mutation;
        let mut typed_variables = variables.clone();
        for variable in typed_variables.values_mut() {
            if numeric_slots.contains(&variable.slot) {
                variable.index_read_override = Some(format!("scratch_typed_{}", variable.slot));
            } else if dynamic.iter().any(|v| v.slot == variable.slot) {
                variable.index_read_override = Some(format!("&scratch_dynamic_{}", variable.slot));
            }
        }
        if has_optimized_locals {
            let pattern = numeric
                .iter()
                .filter(|v| needed.contains(&v.slot))
                .map(|v| format!("Some(mut scratch_typed_{})", v.slot))
                .collect::<Vec<_>>();
            let values = numeric
                .iter()
                .filter(|v| needed.contains(&v.slot))
                .map(|v| format!("{}.canonical_number()", v.read_expression()))
                .collect::<Vec<_>>();
            let slots = numeric
                .iter()
                .filter(|v| deferred.contains(&v.slot))
                .chain(dynamic.iter())
                .map(|v| v.slot.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let pattern = std::iter::once("true".to_owned())
                .chain(pattern)
                .collect::<Vec<_>>()
                .join(", ");
            let values = std::iter::once(format!("state.can_defer_variable_writes(&[{slots}])"))
                .chain(values)
                .collect::<Vec<_>>()
                .join(", ");
            line(output, 5, &format!("if let ({pattern},) = ({values},) {{"));
            for variable in numeric.iter().filter(|v| !needed.contains(&v.slot)) {
                line(
                    output,
                    6,
                    &format!("let mut scratch_typed_{} = 0.0_f64;", variable.slot),
                );
            }
            for variable in numeric.iter().filter(|v| deferred.contains(&v.slot)) {
                line(
                    output,
                    6,
                    &format!("let mut scratch_dirty_{} = false;", variable.slot),
                );
                if textual.contains(&variable.slot) {
                    line(
                        output,
                        6,
                        &format!("let mut scratch_text_{} = false;", variable.slot),
                    );
                }
            }
            for variable in &dynamic {
                let initial = if needed.contains(&variable.slot) {
                    format!("{}.clone()", variable.read_expression())
                } else {
                    "ScratchValue::Undefined".to_owned()
                };
                line(
                    output,
                    6,
                    &format!("let mut scratch_dynamic_{} = {initial};", variable.slot),
                );
                line(
                    output,
                    6,
                    &format!("let mut scratch_dynamic_dirty_{} = false;", variable.slot),
                );
            }
        }
        let mut last = id;
        while let Some(next) = target.blocks[last].next.as_deref() {
            last = next;
        }
        let no_numeric_slots = HashSet::new();
        for optimized in 0..if has_optimized_locals { 2 } else { 1 } {
            let typed = has_optimized_locals && optimized == 0;
            let block_indent = if has_optimized_locals { 6 } else { 5 };
            if repeat && has_locals {
                let slot = graph.repeat_slots[loop_id];
                line(
                    output,
                    block_indent,
                    &format!(
                        "let mut repeat_remaining = script.frames.last().unwrap().repeat_counts[{slot}];"
                    ),
                );
                line(
                    output,
                    block_indent,
                    "let scratch_warp_mode = script.frames.last().unwrap().warp_mode;",
                );
            }
            if repeat {
                line(output, block_indent, "loop {");
            }
            let body_indent = block_indent + usize::from(repeat);
            let loop_context = StructuredLoopContext {
                target,
                graph,
                variables: if typed { &typed_variables } else { variables },
                original_variables: variables,
                lists,
                arguments,
                procedures,
                entries: procedure_entries,
                no_warp_timer,
                numeric_locals: if typed {
                    &numeric_slots
                } else {
                    &no_numeric_slots
                },
                deferred: if typed { &deferred } else { &no_numeric_slots },
                textual: if typed { &textual } else { &no_numeric_slots },
                dynamic: if typed { &dynamic } else { &[] },
            };
            emit_structured_loop_chain(
                &loop_context,
                Some(id),
                output,
                body_indent,
                &mut HashSet::new(),
                &mut HashMap::new(),
            );
            if repeat {
                let repeat_slot = graph.repeat_slots[loop_id];
                let next_pc = enclosing
                    .next
                    .as_ref()
                    .and_then(|next| graph.block_pcs.get(next).copied())
                    .unwrap_or(graph.fallthrough_pcs[loop_id]);
                if !has_locals {
                    line(
                        output,
                        body_indent,
                        &format!(
                            "let repeat_remaining = script.frames.last().unwrap().repeat_counts[{repeat_slot}].saturating_sub(1);"
                        ),
                    );
                    line(
                        output,
                        body_indent,
                        &format!(
                            "script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = repeat_remaining;"
                        ),
                    );
                    line(
                        output,
                        body_indent,
                        &format!(
                            "script.frames.last_mut().unwrap().pc = if repeat_remaining > 0 {{ {} }} else {{ {} }};",
                            graph.block_pcs[id],
                            pc_literal(next_pc)
                        ),
                    );
                } else {
                    line(
                        output,
                        body_indent,
                        "repeat_remaining = repeat_remaining.saturating_sub(1);",
                    );
                }
                let yield_condition = if !has_locals {
                    if no_warp_timer {
                        "!script.frames.last().unwrap().warp_mode"
                    } else {
                        "script.yield_at_safe_point(clock)"
                    }
                } else if no_warp_timer {
                    "!scratch_warp_mode"
                } else {
                    "!scratch_warp_mode || script.warp_budget.expired(clock)"
                };
                line(output, body_indent, &format!("if {yield_condition} {{"));
                if has_locals {
                    if !no_warp_timer {
                        line(
                            output,
                            body_indent + 1,
                            "if !scratch_warp_mode { script.warp_budget.reset(); }",
                        );
                    }
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
                        &format!(
                            "script.frames.last_mut().unwrap().pc = if repeat_remaining > 0 {{ {} }} else {{ {} }};",
                            graph.block_pcs[id],
                            pc_literal(next_pc)
                        ),
                    );
                }
                if typed {
                    emit_deferred_numeric_commits(
                        output,
                        body_indent + 1,
                        &numeric,
                        &deferred,
                        &textual,
                    );
                    emit_deferred_dynamic_commits(output, body_indent + 1, &dynamic, false);
                }
                line(output, body_indent + 1, "return ScriptStatus::Yield;");
                line(output, body_indent, "}");
                line(output, body_indent, "if repeat_remaining == 0 { break; }");
                line(output, block_indent, "}");
                if has_locals {
                    line(
                        output,
                        block_indent,
                        &format!(
                            "script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = 0;"
                        ),
                    );
                    line(
                        output,
                        block_indent,
                        &format!(
                            "script.frames.last_mut().unwrap().pc = {};",
                            pc_literal(next_pc)
                        ),
                    );
                }
                if typed {
                    emit_deferred_numeric_commits(
                        output,
                        block_indent,
                        &numeric,
                        &deferred,
                        &textual,
                    );
                    emit_deferred_dynamic_commits(output, block_indent, &dynamic, false);
                }
            } else {
                line(
                    output,
                    block_indent,
                    &format!(
                        "script.frames.last_mut().unwrap().pc = {};",
                        pc_literal(graph.fallthrough_pcs[last])
                    ),
                );
            }
            if has_optimized_locals && optimized == 0 {
                line(output, 5, "} else {");
            }
        }
        if has_optimized_locals {
            line(output, 5, "}");
        }
        line(output, 5, "continue;");
        line(output, 4, "},");
        return;
    }
    if let Some((loop_id, enclosing)) = enclosing_loop.filter(|(_, block)| {
        block.opcode == "control_repeat" && partial_structured_body(target, id)
    }) {
        emitted_blocks.insert(id.to_owned());
        let (numeric, deferred, textual) =
            structured_numeric_locals(target, id, variables, true, true);
        let mut needed = HashSet::new();
        collect_region_entry_reads(
            target,
            Some(id),
            variables,
            &mut HashSet::new(),
            &mut needed,
            true,
        );
        let numeric_slots = numeric.iter().map(|v| v.slot).collect::<HashSet<_>>();
        let mut typed_variables = variables.clone();
        for variable in typed_variables.values_mut() {
            if numeric_slots.contains(&variable.slot) {
                variable.index_read_override = Some(format!("scratch_typed_{}", variable.slot));
            }
        }
        line(output, 4, &format!("{} => {{", graph.block_pcs[id]));
        if !numeric.is_empty() {
            let pattern = numeric
                .iter()
                .filter(|v| needed.contains(&v.slot))
                .map(|v| format!("Some(mut scratch_typed_{})", v.slot))
                .collect::<Vec<_>>();
            let values = numeric
                .iter()
                .filter(|v| needed.contains(&v.slot))
                .map(|v| format!("{}.canonical_number()", v.read_expression()))
                .collect::<Vec<_>>();
            let slots = numeric
                .iter()
                .filter(|v| deferred.contains(&v.slot))
                .map(|v| v.slot.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let pattern = std::iter::once("true".to_owned())
                .chain(pattern)
                .collect::<Vec<_>>()
                .join(", ");
            let values = std::iter::once(format!("state.can_defer_variable_writes(&[{slots}])"))
                .chain(values)
                .collect::<Vec<_>>()
                .join(", ");
            line(output, 5, &format!("if let ({pattern},) = ({values},) {{"));
            for variable in numeric.iter().filter(|v| !needed.contains(&v.slot)) {
                line(
                    output,
                    6,
                    &format!("let mut scratch_typed_{} = 0.0_f64;", variable.slot),
                );
            }
            for variable in numeric.iter().filter(|v| deferred.contains(&v.slot)) {
                line(
                    output,
                    6,
                    &format!("let mut scratch_dirty_{} = false;", variable.slot),
                );
                if textual.contains(&variable.slot) {
                    line(
                        output,
                        6,
                        &format!("let mut scratch_text_{} = false;", variable.slot),
                    );
                }
            }
        }
        let empty = HashSet::new();
        for version in 0..if numeric.is_empty() { 1 } else { 2 } {
            let typed = !numeric.is_empty() && version == 0;
            let level = if numeric.is_empty() { 5 } else { 6 };
            let repeat_slot = graph.repeat_slots[loop_id];
            let next_pc = enclosing
                .next
                .as_ref()
                .and_then(|next| graph.block_pcs.get(next).copied())
                .unwrap_or(graph.fallthrough_pcs[loop_id]);
            line(
                output,
                level,
                &format!(
                    "let mut repeat_remaining = script.frames.last().unwrap().repeat_counts[{repeat_slot}];"
                ),
            );
            line(
                output,
                level,
                "let scratch_warp_mode = script.frames.last().unwrap().warp_mode;",
            );
            line(output, level, "let mut scratch_region_escaped = false;");
            line(output, level, "'scratch_compiled_loop: loop {");
            let partial_outcome = emit_partial_structured_chain(
                target,
                Some(id),
                graph,
                if typed { &typed_variables } else { variables },
                variables,
                lists,
                arguments,
                if typed { &numeric_slots } else { &empty },
                if typed { &deferred } else { &empty },
                if typed { &textual } else { &empty },
                procedures,
                procedure_entries,
                no_warp_timer,
                Some(repeat_slot),
                true,
                output,
                level + 1,
            );
            line(
                output,
                level + 1,
                "repeat_remaining = repeat_remaining.saturating_sub(1);",
            );
            let yield_condition = if no_warp_timer {
                "!scratch_warp_mode"
            } else {
                "!scratch_warp_mode || script.warp_budget.expired(clock)"
            };
            line(output, level + 1, &format!("if {yield_condition} {{"));
            if !no_warp_timer {
                line(
                    output,
                    level + 2,
                    "if !scratch_warp_mode { script.warp_budget.reset(); }",
                );
            }
            line(
                output,
                level + 2,
                &format!(
                    "script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = repeat_remaining;"
                ),
            );
            line(
                output,
                level + 2,
                &format!(
                    "script.frames.last_mut().unwrap().pc = if repeat_remaining > 0 {{ {} }} else {{ {} }};",
                    graph.block_pcs[id],
                    pc_literal(next_pc)
                ),
            );
            if typed && !partial_outcome.deferred_flushed {
                emit_deferred_numeric_commits(output, level + 2, &numeric, &deferred, &textual);
            }
            line(output, level + 2, "return ScriptStatus::Yield;");
            line(output, level + 1, "}");
            line(output, level + 1, "if repeat_remaining == 0 { break; }");
            line(output, level, "}");
            line(
                output,
                level,
                &format!(
                    "script.frames.last_mut().unwrap().repeat_counts[{repeat_slot}] = repeat_remaining;"
                ),
            );
            line(
                output,
                level,
                &format!(
                    "if !scratch_region_escaped {{ script.frames.last_mut().unwrap().pc = {}; }}",
                    pc_literal(next_pc)
                ),
            );
            if typed && !partial_outcome.deferred_flushed {
                emit_deferred_numeric_commits(output, level, &numeric, &deferred, &textual);
                line(output, 5, "} else {");
            } else if typed {
                line(output, 5, "} else {");
            }
        }
        if !numeric.is_empty() {
            line(output, 5, "}");
        }
        line(output, 5, "continue;");
        line(output, 4, "},");
        return;
    }
    if let Some(plan) = fused_repeat_plan(target, block, variables) {
        emitted_blocks.insert(id.to_owned());
        emitted_blocks.extend(plan.blocks.iter().cloned());
        generate_fused_repeat_loop(
            StatementContext {
                target,
                variables,
                lists,
                arguments,
            },
            id,
            block,
            graph,
            &plan,
            no_warp_timer,
            output,
        );
        return;
    }
    if let Some(plan) = numeric_loop_plan(target, block, variables) {
        emitted_blocks.insert(id.to_owned());
        emitted_blocks.extend(plan.body.iter().cloned());
        generate_native_numeric_loop(
            NumericLoopContext {
                target,
                graph,
                variables,
                fast_math,
                no_warp_timer,
            },
            id,
            block,
            &plan,
            output,
        );
        return;
    }
    if matches!(block.opcode.as_str(), "control_if" | "control_if_else")
        && partial_structured_body(target, id)
        && structured_if_body(target, id, block, procedures, procedure_entries)
            .is_none_or(|body| body.has_procedure_calls)
    {
        emitted_blocks.insert(id.to_owned());
        let (numeric, deferred, textual) =
            structured_numeric_locals(target, id, variables, true, true);
        let mut needed = HashSet::new();
        collect_region_entry_reads(
            target,
            Some(id),
            variables,
            &mut HashSet::new(),
            &mut needed,
            true,
        );
        let numeric_slots = numeric.iter().map(|v| v.slot).collect::<HashSet<_>>();
        let mut typed_variables = variables.clone();
        for variable in typed_variables.values_mut() {
            if numeric_slots.contains(&variable.slot) {
                variable.index_read_override = Some(format!("scratch_typed_{}", variable.slot));
            }
        }
        let mut last = id;
        while let Some(next) = target.blocks[last].next.as_deref() {
            last = next;
        }
        let continuation = graph.fallthrough_pcs[last];
        line(output, 4, &format!("{} => {{", graph.block_pcs[id]));
        if !numeric.is_empty() {
            let pattern = std::iter::once("true".to_owned())
                .chain(
                    numeric
                        .iter()
                        .filter(|v| needed.contains(&v.slot))
                        .map(|v| format!("Some(mut scratch_typed_{})", v.slot)),
                )
                .collect::<Vec<_>>()
                .join(", ");
            let slots = numeric
                .iter()
                .filter(|v| deferred.contains(&v.slot))
                .map(|v| v.slot.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let values = std::iter::once(format!("state.can_defer_variable_writes(&[{slots}])"))
                .chain(
                    numeric
                        .iter()
                        .filter(|v| needed.contains(&v.slot))
                        .map(|v| format!("{}.canonical_number()", v.read_expression())),
                )
                .collect::<Vec<_>>()
                .join(", ");
            line(output, 5, &format!("if let ({pattern},) = ({values},) {{"));
            for variable in numeric.iter().filter(|v| !needed.contains(&v.slot)) {
                line(
                    output,
                    6,
                    &format!("let mut scratch_typed_{} = 0.0_f64;", variable.slot),
                );
            }
            for variable in numeric.iter().filter(|v| deferred.contains(&v.slot)) {
                line(
                    output,
                    6,
                    &format!("let mut scratch_dirty_{} = false;", variable.slot),
                );
                if textual.contains(&variable.slot) {
                    line(
                        output,
                        6,
                        &format!("let mut scratch_text_{} = false;", variable.slot),
                    );
                }
            }
        }
        let empty = HashSet::new();
        for version in 0..if numeric.is_empty() { 1 } else { 2 } {
            let typed = !numeric.is_empty() && version == 0;
            let level = if numeric.is_empty() { 5 } else { 6 };
            line(output, level, "let mut scratch_region_escaped = false;");
            let mut region_output = String::new();
            let partial_outcome = emit_partial_structured_chain(
                target,
                Some(id),
                graph,
                if typed { &typed_variables } else { variables },
                variables,
                lists,
                arguments,
                if typed { &numeric_slots } else { &empty },
                if typed { &deferred } else { &empty },
                if typed { &textual } else { &empty },
                procedures,
                procedure_entries,
                no_warp_timer,
                None,
                true,
                &mut region_output,
                level + 1,
            );
            line(
                output,
                level,
                if partial_outcome.requires_escape_label {
                    "'scratch_compiled_loop: {"
                } else {
                    "{"
                },
            );
            output.push_str(&region_output);
            line(output, level, "}");
            if typed && !partial_outcome.deferred_flushed {
                emit_deferred_numeric_commits(output, level, &numeric, &deferred, &textual);
            }
            line(
                output,
                level,
                &format!(
                    "if !scratch_region_escaped {{ script.frames.last_mut().unwrap().pc = {}; }}",
                    pc_literal(continuation)
                ),
            );
            if typed {
                line(output, 5, "} else {");
            }
        }
        if !numeric.is_empty() {
            line(output, 5, "}");
        }
        line(output, 5, "continue;");
        line(output, 4, "},");
        return;
    }
    if let Some(region) = straight_numeric_region(target, id, variables).filter(|region| {
        region
            .blocks
            .iter()
            .all(|id| graph.block_pcs.contains_key(*id) && !emitted_blocks.contains(*id))
    }) {
        let StraightNumericRegion {
            blocks: ids,
            variables: numeric,
            deferred,
            needed,
        } = region;
        for id in &ids {
            emitted_blocks.insert((*id).to_owned());
        }
        let slots = numeric.iter().map(|v| v.slot).collect::<HashSet<_>>();
        let mut typed_variables = variables.clone();
        for variable in typed_variables.values_mut() {
            if slots.contains(&variable.slot) {
                variable.index_read_override = Some(format!("scratch_typed_{}", variable.slot));
            }
        }
        let pattern = std::iter::once("true".to_owned())
            .chain(
                numeric
                    .iter()
                    .filter(|v| needed.contains(&v.slot))
                    .map(|v| format!("Some(mut scratch_typed_{})", v.slot)),
            )
            .collect::<Vec<_>>()
            .join(", ");
        let guarded = numeric
            .iter()
            .filter(|v| needed.contains(&v.slot))
            .map(|v| format!("{}.canonical_number()", v.read_expression()));
        let deferred_slots = numeric
            .iter()
            .filter(|v| deferred.contains(&v.slot))
            .map(|v| v.slot.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let values = std::iter::once(format!(
            "state.can_defer_variable_writes(&[{deferred_slots}])"
        ))
        .chain(guarded)
        .collect::<Vec<_>>()
        .join(", ");
        line(output, 4, &format!("{} => {{", graph.block_pcs[id]));
        line(output, 5, &format!("if let ({pattern},) = ({values},) {{"));
        for variable in numeric.iter().filter(|v| !needed.contains(&v.slot)) {
            line(
                output,
                6,
                &format!("let mut scratch_typed_{};", variable.slot),
            );
        }
        for typed in [true, false] {
            let active = if typed { &typed_variables } else { variables };
            for current in &ids {
                let statement = &target.blocks[*current];
                let variable = variable_field(statement, "VARIABLE", active)
                    .expect("region target was analyzed");
                if typed && slots.contains(&variable.slot) {
                    if statement.opcode == "data_setvariableto" {
                        emit_typed_numeric_set(
                            TypedNumericSetContext {
                                target,
                                variables: active,
                                lists,
                                arguments,
                                deferred: &deferred,
                                textual: &HashSet::new(),
                                dirty_tracking: false,
                            },
                            statement,
                            &variable,
                            output,
                            6,
                        );
                    } else {
                        let value = expression(
                            target,
                            statement.inputs.get("VALUE"),
                            active,
                            lists,
                            arguments,
                            &mut HashSet::new(),
                        );
                        line(
                            output,
                            6,
                            &format!("let scratch_change = scratch_to_number({value});"),
                        );
                        line(
                            output,
                            6,
                            &format!(
                                "scratch_typed_{0} = scratch_number_operand(scratch_typed_{0}) + scratch_change;",
                                variable.slot
                            ),
                        );
                    }
                } else {
                    generate_resumable_statement(
                        StatementContext {
                            target,
                            variables: active,
                            lists,
                            arguments,
                        },
                        statement,
                        usize::MAX,
                        output,
                        6,
                    );
                }
            }
            if typed {
                emit_definite_numeric_commits(output, 6, &numeric, &deferred);
                line(output, 5, "} else {");
            }
        }
        line(output, 5, "}");
        let last = ids.last().unwrap();
        let next = target.blocks[*last]
            .next
            .as_ref()
            .and_then(|id| graph.block_pcs.get(id).copied())
            .unwrap_or(graph.fallthrough_pcs[*last]);
        line(
            output,
            5,
            &format!(
                "script.frames.last_mut().unwrap().pc = {};",
                pc_literal(next)
            ),
        );
        line(output, 5, "continue;");
        line(output, 4, "},");
        return;
    }
    if !is_straight_line_block(block) {
        emitted_blocks.insert(id.to_owned());
        generate_resumable_block(context, id, output);
        return;
    }

    let start_pc = graph.block_pcs[id];
    line(output, 4, &format!("{start_pc} => {{"));
    let mut current = Some(id);
    let mut last = id;
    while let Some(current_id) = current {
        if !emitted_blocks.insert(current_id.to_owned()) {
            break;
        }
        let Some(current_block) = target.blocks.get(current_id) else {
            break;
        };
        generate_resumable_statement(
            StatementContext {
                target,
                variables,
                lists,
                arguments,
            },
            current_block,
            current_block
                .next
                .as_ref()
                .and_then(|next| graph.block_pcs.get(next).copied())
                .unwrap_or(graph.fallthrough_pcs[current_id]),
            output,
            5,
        );
        last = current_id;
        current = current_block.next.as_deref().filter(|next_id| {
            graph.block_pcs.contains_key(*next_id)
                && target
                    .blocks
                    .get(*next_id)
                    .is_some_and(is_straight_line_block)
        });
    }
    let last_block = &target.blocks[last];
    let next_pc = last_block
        .next
        .as_ref()
        .and_then(|next| graph.block_pcs.get(next).copied())
        .unwrap_or(graph.fallthrough_pcs[last]);
    line(
        output,
        5,
        &format!(
            "script.frames.last_mut().unwrap().pc = {};",
            pc_literal(next_pc)
        ),
    );
    line(output, 5, "continue;");
    line(output, 4, "},");
}
