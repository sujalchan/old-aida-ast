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
    numeric_arguments: HashMap<String, Vec<bool>>,
    numeric_frame_arguments: HashMap<String, Vec<bool>>,
    numeric_warp_procedures: HashSet<String>,
}

#[derive(Clone)]
struct ProcedureCallSite {
    block_id: String,
    caller_id: Option<String>,
    callee_id: String,
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
    pcs: Vec<usize>,
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

    let mut arms_by_function = HashMap::<usize, Vec<GeneratedStateArm>>::new();
    for arm in source_arms {
        arms_by_function
            .entry(arm.function_id)
            .or_default()
            .push(arm.clone());
    }

    let mut partitions = Vec::new();
    for (function_id, mut function_arms) in arms_by_function {
        function_arms.sort_by_key(|arm| arm.pc);
        let groups = state_arm_groups(&function_arms);
        let mut current_arms = Vec::new();
        let mut current_bytes = 0;
        let mut partition_index = 0;

        for group in groups {
            let group_bytes = group.iter().map(|arm| arm.source.len()).sum::<usize>();
            if !current_arms.is_empty()
                && current_bytes + group_bytes > RESUMABLE_PARTITION_TARGET_BYTES
            {
                partitions.push(state_partition(
                    function_id,
                    partition_index,
                    std::mem::take(&mut current_arms),
                ));
                current_bytes = 0;
                partition_index += 1;
            }
            current_bytes += group_bytes;
            current_arms.extend(group);

            // Keep an individually large PC or control-flow cycle intact.
            if current_bytes >= RESUMABLE_PARTITION_TARGET_BYTES {
                partitions.push(state_partition(
                    function_id,
                    partition_index,
                    std::mem::take(&mut current_arms),
                ));
                current_bytes = 0;
                partition_index += 1;
            }
        }
        if !current_arms.is_empty() {
            partitions.push(state_partition(function_id, partition_index, current_arms));
        }
    }
    (partitions.len() > 1).then_some(partitions)
}

fn state_partition(
    function_id: usize,
    index: usize,
    mut arms: Vec<GeneratedStateArm>,
) -> GeneratedStatePartition {
    arms.sort_by_key(|arm| arm.pc);
    let pcs = arms.iter().map(|arm| arm.pc).collect::<Vec<_>>();
    GeneratedStatePartition {
        function_id,
        index,
        pcs,
        arms,
    }
}

fn state_arm_groups(arms: &[GeneratedStateArm]) -> Vec<Vec<GeneratedStateArm>> {
    let index_by_pc = arms
        .iter()
        .enumerate()
        .map(|(index, arm)| (arm.pc, index))
        .collect::<HashMap<_, _>>();
    let mut edges = vec![Vec::new(); arms.len()];
    let mut reverse_edges = vec![Vec::new(); arms.len()];
    for (index, arm) in arms.iter().enumerate() {
        for target_pc in emitted_pc_targets(&arm.source) {
            if let Some(&target_index) = index_by_pc.get(&target_pc) {
                edges[index].push(target_index);
                reverse_edges[target_index].push(index);
            }
        }
    }

    fn visit(index: usize, edges: &[Vec<usize>], visited: &mut [bool], order: &mut Vec<usize>) {
        if std::mem::replace(&mut visited[index], true) {
            return;
        }
        for &next in &edges[index] {
            visit(next, edges, visited, order);
        }
        order.push(index);
    }

    let mut order = Vec::with_capacity(arms.len());
    let mut visited = vec![false; arms.len()];
    for index in 0..arms.len() {
        visit(index, &edges, &mut visited, &mut order);
    }
    let mut component_for = vec![usize::MAX; arms.len()];
    let mut components = Vec::<Vec<usize>>::new();
    for &root in order.iter().rev() {
        if component_for[root] != usize::MAX {
            continue;
        }
        let component_id = components.len();
        let mut pending = vec![root];
        let mut component = Vec::new();
        component_for[root] = component_id;
        while let Some(index) = pending.pop() {
            component.push(index);
            for &next in &reverse_edges[index] {
                if component_for[next] == usize::MAX {
                    component_for[next] = component_id;
                    pending.push(next);
                }
            }
        }
        components.push(component);
    }

    let mut grouped = vec![false; arms.len()];
    let mut groups = Vec::new();
    for component in components {
        let cyclic = component.len() > 1 || edges[component[0]].contains(&component[0]);
        if cyclic {
            let mut group = component
                .iter()
                .map(|&index| {
                    grouped[index] = true;
                    arms[index].clone()
                })
                .collect::<Vec<_>>();
            group.sort_by_key(|arm| arm.pc);
            groups.push(group);
        }
    }
    for (index, arm) in arms.iter().enumerate() {
        if !grouped[index] {
            groups.push(vec![arm.clone()]);
        }
    }
    groups.sort_by_key(|group| group[0].pc);
    groups
}

fn emitted_pc_targets(source: &str) -> HashSet<usize> {
    let mut targets = HashSet::new();
    for marker in [
        "__aida_set_pc!(script, ",
        "__aida_continue_at!(script, ",
        "script.frames.last_mut().unwrap().pc = ",
        "script.frame_mut().pc = ",
    ] {
        let mut rest = source;
        while let Some(index) = rest.find(marker) {
            rest = &rest[index + marker.len()..];
            let end = rest.find(if marker.contains(".pc = ") { ";" } else { ");" });
            let Some(end) = end else { break };
            let expression = rest[..end].trim();
            add_pc_expression(expression, source, &mut targets);
            rest = &rest[end..];
        }
    }
    targets
}

fn add_pc_expression(expression: &str, source: &str, targets: &mut HashSet<usize>) {
    if let Ok(target) = expression.parse::<usize>() {
        targets.insert(target);
    } else if expression == "next_pc" {
        if let Some(targets_expression) = source.find("let next_pc = if ").and_then(|start| {
            source[start..]
                .find(';')
                .map(|end| &source[start..start + end])
        }) {
            add_if_pc_targets(targets_expression, targets);
        }
    } else {
        add_if_pc_targets(expression, targets);
    }
}

fn add_if_pc_targets(expression: &str, targets: &mut HashSet<usize>) {
    let Some(if_index) = expression.find("if ") else {
        return;
    };
    let expression = &expression[if_index + 3..];
    let Some(first_open) = expression.find('{') else {
        return;
    };
    let Some(first_close) = expression[first_open + 1..].find('}') else {
        return;
    };
    if let Ok(target) = expression[first_open + 1..first_open + 1 + first_close]
        .trim()
        .parse::<usize>()
    {
        targets.insert(target);
    }
    let after_first = &expression[first_open + first_close + 2..];
    let Some(second_open) = after_first.find('{') else {
        return;
    };
    let Some(second_close) = after_first[second_open + 1..].find('}') else {
        return;
    };
    if let Ok(target) = after_first[second_open + 1..second_open + 1 + second_close]
        .trim()
        .parse::<usize>()
    {
        targets.insert(target);
    }
}

#[cfg(test)]
mod partition_tests {
    use super::*;

    #[test]
    fn generated_state_partitions_keep_control_flow_cycles_together() {
        let arms = [
            GeneratedStateArm {
                function_id: 7,
                pc: 0,
                source: format!(
                    "0 => {{ __aida_continue_at!(script, 2); {} }}",
                    " ".repeat(5_000)
                ),
            },
            GeneratedStateArm {
                function_id: 7,
                pc: 1,
                source: "1 => { return None; }".to_owned(),
            },
            GeneratedStateArm {
                function_id: 7,
                pc: 2,
                source: format!(
                    "2 => {{ let next_pc = if repeat_remaining > 0 {{ 4 }} else {{ 5 }}; script.frames.last_mut().unwrap().pc = next_pc; continue; {} }}",
                    " ".repeat(5_000)
                ),
            },
            GeneratedStateArm {
                function_id: 7,
                pc: 3,
                source: "3 => { return None; }".to_owned(),
            },
            GeneratedStateArm {
                function_id: 7,
                pc: 4,
                source: format!(
                    "4 => {{ __aida_continue_at!(script, 0); {} }}",
                    " ".repeat(5_000)
                ),
            },
            GeneratedStateArm {
                function_id: 7,
                pc: 5,
                source: format!(
                    "5 => {{ {} }}",
                    " ".repeat(RESUMABLE_PARTITION_TARGET_BYTES)
                ),
            },
        ];
        let partitions = partition_generated_state_arms(&arms).unwrap();
        let cycle = partitions
            .iter()
            .find(|partition| partition.pcs == [0, 2, 4])
            .expect("the cycle should stay together despite noncontiguous PCs");
        assert_eq!(partition_pc_guard(cycle), "matches!(pc, 0 | 2 | 4)");
        assert!(partitions.iter().all(|partition| {
            partition
                .pcs
                .iter()
                .filter(|pc| [0, 2, 4].contains(pc))
                .count()
                == 0
                || partition.pcs == [0, 2, 4]
        }));
    }
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
        let numeric_arguments = plan
            .numeric_frame_arguments
            .get(&procedure.definition_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let numeric_only_arguments = !procedure.argument_ids.is_empty()
            && numeric_arguments.len() == procedure.argument_ids.len()
            && numeric_arguments.iter().all(|is_numeric| *is_numeric);
        let argument_accesses = procedure
            .argument_names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    name.clone(),
                    if numeric_arguments.get(index).copied().unwrap_or(false) {
                        format!("__aida_numeric_arg_{index}__")
                    } else {
                        format!("script.argument({index})")
                    },
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
        let mut procedure_arms = collect_graph_state_arms(context, procedure.frame_id);
        for arm in &mut procedure_arms {
            let declarations = numeric_arguments
                .iter()
                .enumerate()
                .filter(|(index, is_numeric)| {
                    **is_numeric
                        && arm
                            .source
                            .contains(&format!("__aida_numeric_arg_{index}__"))
                })
                .map(|(index, _)| {
                    let access = if numeric_only_arguments {
                        format!("script.numeric_argument({index})")
                    } else {
                        format!("script.argument({index}).number_or_nan()")
                    };
                    format!("                    let __aida_numeric_arg_{index}__ = {access};\n")
                })
                .collect::<String>();
            if !declarations.is_empty()
                && let Some(body_start) = arm.source.find(" => {\n").map(|index| index + 6)
            {
                arm.source.insert_str(body_start, &declarations);
            }
        }
        arms.extend(procedure_arms);
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
        line(output, 4, "_ => { script.pop_frame(); continue; },");
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
    format!("matches!(pc, {})", partition_pc_pattern(&partition.pcs))
}

fn partition_pc_pattern(pcs: &[usize]) -> String {
    let mut ranges = Vec::new();
    let mut first = pcs[0];
    let mut last = first;
    for &pc in &pcs[1..] {
        if pc == last + 1 {
            last = pc;
            continue;
        }
        ranges.push(if first == last {
            first.to_string()
        } else {
            format!("{first}..={last}")
        });
        first = pc;
        last = pc;
    }
    ranges.push(if first == last {
        first.to_string()
    } else {
        format!("{first}..={last}")
    });
    ranges.join(" | ")
}

fn emit_partition_worker(
    function_name: &str,
    partition: &GeneratedStatePartition,
    procedure_state: bool,
    output: &mut String,
) {
    let worker_name = partition_worker_name(function_name, partition);
    line(output, 0, "#[inline]");
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
            &format!("_ if {pc_guard} => {{ script.pop_frame(); return None; }},"),
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
                "{} => match {worker_name}(script, state, clock) {{ Some(status) => return status, None => continue, }},",
                partition_pc_pattern(&partition.pcs)
            ),
        );
    }
    if procedure_state {
        line(output, indent, "_ => { script.pop_frame(); continue; },");
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

    pub(in crate::rust) fn numeric_arguments(&self) -> &HashMap<String, Vec<bool>> {
        &self.numeric_arguments
    }

    pub(in crate::rust) fn numeric_warp_procedure_ids(&self) -> &HashSet<String> {
        &self.numeric_warp_procedures
    }
}

pub(in crate::rust) fn prepare_resumable_procedures(
    target: &ScratchTarget,
    script_starts: &[Option<&str>],
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    procedures: &[ProcedureSignature],
    target_index: usize,
    no_warp_timer: bool,
) -> ResumableProcedurePlan {
    let calls_in = |graph: &ScriptGraph,
                    call_block_ids: &mut HashSet<String>,
                    caller_id: Option<&str>,
                    call_sites: &mut Vec<ProcedureCallSite>| {
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
                let procedure = procedure_for_call(block, procedures)?;
                call_sites.push(ProcedureCallSite {
                    block_id: id.clone(),
                    caller_id: caller_id.map(str::to_owned),
                    callee_id: procedure.definition_id.clone(),
                });
                Some(procedure)
            })
            .map(|procedure| procedure.definition_id.clone())
            .collect::<Vec<_>>()
    };
    let mut pending = Vec::new();
    let mut call_block_ids = HashSet::new();
    let mut call_sites = Vec::new();
    for first in script_starts {
        let mut graph = ScriptGraph::new();
        graph.add_chain(target, *first, usize::MAX);
        pending.extend(calls_in(&graph, &mut call_block_ids, None, &mut call_sites));
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
            pending.extend(calls_in(
                &graph,
                &mut call_block_ids,
                Some(&id),
                &mut call_sites,
            ));
        }
    }

    let numeric_arguments =
        infer_numeric_procedure_arguments(target, variables, procedures, &reachable, &call_sites);
    let numeric_context_arguments =
        infer_numeric_context_procedure_arguments(target, procedures, &reachable);
    let numeric_frame_arguments = procedures
        .iter()
        .filter(|procedure| reachable.contains(&procedure.definition_id))
        .map(|procedure| {
            let proven = numeric_arguments
                .get(&procedure.definition_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let contextual = numeric_context_arguments
                .get(&procedure.definition_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let arguments = (0..procedure.argument_ids.len())
                .map(|index| {
                    proven.get(index).copied().unwrap_or(false)
                        || contextual.get(index).copied().unwrap_or(false)
                })
                .collect();
            (procedure.definition_id.clone(), arguments)
        })
        .collect::<HashMap<_, _>>();

    let direct_warp_eligibility = direct_warp_eligibility(target, procedures);
    let bounded_warp_eligibility = direct_warp_bounded_eligibility(target, procedures);
    let direct_warp_status_procedures = direct_warp_status_procedures(target, procedures);
    let inline_candidates = procedures
        .iter()
        .filter(|procedure| {
            procedure.warp
                && procedure.argument_ids.is_empty()
                && !direct_eligible(target, procedure)
                && !(no_warp_timer
                    && direct_warp_eligibility
                        .get(&procedure.definition_id)
                        .copied()
                        .unwrap_or(false))
                && !bounded_warp_eligibility
                    .get(&procedure.definition_id)
                    .copied()
                    .unwrap_or(false)
        })
        .filter_map(|procedure| {
            let definition = target.blocks.get(&procedure.definition_id)?;
            let mut body = ScriptGraph::new();
            body.add_chain(target, definition.next.as_deref(), usize::MAX);
            (!body
                .nodes
                .iter()
                .any(|node| matches!(node, ScriptNode::LoopBoundary(_))))
            .then_some(procedure.definition_id.clone())
        })
        .collect::<HashSet<_>>();
    let mut inline_call_counts = HashMap::<(String, String), usize>::new();
    let mut callee_call_site_counts = HashMap::<String, usize>::new();
    let mut procedure_call_graph = HashMap::<String, Vec<String>>::new();
    for call_site in &call_sites {
        *callee_call_site_counts
            .entry(call_site.callee_id.clone())
            .or_default() += 1;
        if let Some(caller_id) = call_site.caller_id.as_ref() {
            procedure_call_graph
                .entry(caller_id.clone())
                .or_default()
                .push(call_site.callee_id.clone());
        }
    }
    for call_site in &call_sites {
        let Some(caller_id) = call_site.caller_id.as_deref() else {
            continue;
        };
        let caller_is_warp = procedures
            .iter()
            .find(|procedure| procedure.definition_id == caller_id)
            .is_some_and(|procedure| procedure.warp);
        let recursive_edge =
            procedure_reaches_procedure(&call_site.callee_id, caller_id, &procedure_call_graph);
        if caller_is_warp
            && !recursive_edge
            && callee_call_site_counts.get(&call_site.callee_id) == Some(&1)
            && inline_candidates.contains(&call_site.callee_id)
        {
            *inline_call_counts
                .entry((caller_id.to_owned(), call_site.callee_id.clone()))
                .or_default() += 1;
        }
    }
    let mut inline_calls_by_caller = HashMap::<String, HashSet<String>>::new();
    for call_site in &call_sites {
        let Some(caller_id) = call_site.caller_id.as_deref() else {
            continue;
        };
        if inline_call_counts.get(&(caller_id.to_owned(), call_site.callee_id.clone())) == Some(&1)
        {
            inline_calls_by_caller
                .entry(caller_id.to_owned())
                .or_default()
                .insert(call_site.block_id.clone());
        }
    }
    let fully_inlined_procedures = reachable
        .iter()
        .filter(|callee_id| {
            let sites = call_sites
                .iter()
                .filter(|call_site| &call_site.callee_id == *callee_id)
                .collect::<Vec<_>>();
            !sites.is_empty()
                && sites.iter().all(|call_site| {
                    call_site.caller_id.as_ref().is_some_and(|caller_id| {
                        inline_calls_by_caller
                            .get(caller_id)
                            .is_some_and(|blocks| blocks.contains(&call_site.block_id))
                    })
                })
        })
        .cloned()
        .collect::<HashSet<_>>();
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
        let inline_calls = expanded_inline_calls(
            &procedure.definition_id,
            &inline_calls_by_caller,
            &call_sites,
        );
        if !inline_calls.is_empty() {
            graph.add_chain_with_inlined_calls(
                target,
                definition.next.as_deref(),
                usize::MAX,
                procedures,
                &inline_calls,
                &procedure.definition_id,
            );
        } else {
            graph.add_chain(target, definition.next.as_deref(), usize::MAX);
        }
        let start_pc = definition
            .next
            .as_deref()
            .and_then(|id| graph.block_pcs.get(id).copied())
            .unwrap_or(usize::MAX);
        let direct_path_name = direct_eligible(target, procedure)
            .then(|| direct_name(target_index, procedure.frame_id));
        let direct_path_is_none = direct_path_name.is_none();
        let direct_warp_path = (direct_path_is_none
            && ((no_warp_timer
                && direct_warp_eligibility
                    .get(&procedure.definition_id)
                    .copied()
                    .unwrap_or(false))
                || bounded_warp_eligibility
                    .get(&procedure.definition_id)
                    .copied()
                    .unwrap_or(false)))
        .then(|| direct_warp_name(target_index, procedure.frame_id));
        let numeric_warp_arguments = (direct_warp_path.is_some()
            && procedure.warp
            && bounded_warp_eligibility
                .get(&procedure.definition_id)
                .copied()
                .unwrap_or(false))
        .then(|| numeric_arguments.get(&procedure.definition_id).cloned())
        .flatten()
        .filter(|arguments| arguments.len() == procedure.argument_ids.len())
        .filter(|arguments| {
            super::super::direct_numeric::generate_numeric_direct(
                target,
                procedure,
                variables,
                lists,
                procedures,
                &direct_warp_name(target_index, procedure.frame_id),
                false,
                Some(arguments),
            )
            .is_some()
        });
        let bounded_warp = bounded_warp_eligibility
            .get(&procedure.definition_id)
            .copied()
            .unwrap_or(false);
        let direct_warp_only = procedure.warp && bounded_warp && direct_warp_path.is_some();
        let numeric_direct = direct_path_name.is_some()
            && super::super::direct_numeric::generate_numeric_direct(
                target,
                procedure,
                variables,
                lists,
                procedures,
                &direct_name(target_index, procedure.frame_id),
                true,
                None,
            )
            .is_some();
        entries.insert(
            procedure.definition_id.clone(),
            ProcedureEntry {
                function_id: procedure.frame_id,
                start_pc,
                repeat_count: graph.repeat_slots.len(),
                direct_name: direct_path_name,
                direct_warp_returns_status: direct_warp_path.is_some()
                    && direct_warp_status_procedures.contains(&procedure.definition_id),
                direct_warp_name: direct_warp_path,
                bounded_warp,
                numeric_arguments: numeric_frame_arguments
                    .get(&procedure.definition_id)
                    .cloned(),
                numeric_call_arguments: numeric_arguments.get(&procedure.definition_id).cloned(),
                numeric_warp_arguments,
                numeric_direct,
            },
        );
        if !direct_warp_only && !fully_inlined_procedures.contains(&procedure.definition_id) {
            graphs.push((procedure.clone(), graph));
        }
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
        let proven_numeric_arguments =
            numeric_arguments
                .get(&procedure.definition_id)
                .is_some_and(|arguments| {
                    arguments.len() == procedure.argument_ids.len()
                        && arguments.iter().all(|is_numeric| *is_numeric)
                });
        if (proven_numeric_arguments && entry.numeric_direct)
            || numeric_list_call_candidate(target, block, procedure, entry, &argument_caches)
        {
            numeric_call_procedures.insert(procedure.definition_id.clone());
        }
    }
    let numeric_warp_procedures = entries
        .iter()
        .filter_map(|(id, entry)| entry.numeric_warp_arguments.is_some().then_some(id.clone()))
        .collect();
    ResumableProcedurePlan {
        target_index,
        graphs,
        entries,
        reachable,
        numeric_call_procedures,
        numeric_arguments,
        numeric_frame_arguments,
        numeric_warp_procedures,
    }
}

fn procedure_reaches_procedure(
    start: &str,
    target: &str,
    call_graph: &HashMap<String, Vec<String>>,
) -> bool {
    let mut pending = vec![start.to_owned()];
    let mut seen = HashSet::new();
    while let Some(procedure_id) = pending.pop() {
        if procedure_id == target {
            return true;
        }
        if !seen.insert(procedure_id.clone()) {
            continue;
        }
        pending.extend(call_graph.get(&procedure_id).into_iter().flatten().cloned());
    }
    false
}

fn expanded_inline_calls(
    caller_id: &str,
    inline_calls_by_caller: &HashMap<String, HashSet<String>>,
    call_sites: &[ProcedureCallSite],
) -> HashSet<String> {
    let mut pending = vec![caller_id.to_owned()];
    let mut expanded_callers = HashSet::new();
    let mut inline_calls = HashSet::new();
    while let Some(current_caller) = pending.pop() {
        if !expanded_callers.insert(current_caller.clone()) {
            continue;
        }
        let Some(selected_calls) = inline_calls_by_caller.get(&current_caller) else {
            continue;
        };
        for call_site in call_sites.iter().filter(|site| {
            site.caller_id.as_deref() == Some(current_caller.as_str())
                && selected_calls.contains(&site.block_id)
        }) {
            inline_calls.insert(call_site.block_id.clone());
            pending.push(call_site.callee_id.clone());
        }
    }
    inline_calls
}

fn infer_numeric_procedure_arguments(
    target: &ScratchTarget,
    variables: &HashMap<String, VariableInfo>,
    procedures: &[ProcedureSignature],
    reachable: &HashSet<String>,
    call_sites: &[ProcedureCallSite],
) -> HashMap<String, Vec<bool>> {
    let mut numeric = procedures
        .iter()
        .filter(|procedure| reachable.contains(&procedure.definition_id))
        .map(|procedure| {
            let arity_matches = procedure.argument_names.len() == procedure.argument_ids.len();
            (
                procedure.definition_id.clone(),
                vec![arity_matches; procedure.argument_ids.len()],
            )
        })
        .collect::<HashMap<_, _>>();
    let signatures = procedures
        .iter()
        .map(|procedure| (procedure.definition_id.as_str(), procedure))
        .collect::<HashMap<_, _>>();

    loop {
        let previous = numeric.clone();
        for call_site in call_sites {
            let (Some(callee), Some(block)) = (
                signatures.get(call_site.callee_id.as_str()).copied(),
                target.blocks.get(&call_site.block_id),
            ) else {
                continue;
            };
            let caller = call_site
                .caller_id
                .as_deref()
                .and_then(|id| signatures.get(id).copied());
            let caller_arguments = call_site
                .caller_id
                .as_deref()
                .and_then(|id| previous.get(id));
            let Some(callee_arguments) = numeric.get_mut(&callee.definition_id) else {
                continue;
            };
            for (index, argument_id) in callee.argument_ids.iter().enumerate() {
                let proven_number = input_is_proven_numeric_argument(
                    target,
                    block.inputs.get(argument_id),
                    variables,
                    caller,
                    caller_arguments.map(Vec::as_slice),
                );
                callee_arguments[index] &= proven_number;
            }
        }
        if numeric == previous {
            return numeric;
        }
    }
}

fn infer_numeric_context_procedure_arguments(
    target: &ScratchTarget,
    procedures: &[ProcedureSignature],
    reachable: &HashSet<String>,
) -> HashMap<String, Vec<bool>> {
    let mut numeric = procedures
        .iter()
        .filter(|procedure| reachable.contains(&procedure.definition_id))
        .map(|procedure| {
            (
                procedure.definition_id.clone(),
                vec![true; procedure.argument_ids.len()],
            )
        })
        .collect::<HashMap<_, _>>();

    let procedure_blocks = procedures
        .iter()
        .filter(|procedure| reachable.contains(&procedure.definition_id))
        .map(|procedure| {
            let mut graph = ScriptGraph::new();
            let first = target
                .blocks
                .get(&procedure.definition_id)
                .and_then(|definition| definition.next.as_deref());
            graph.add_chain(target, first, usize::MAX);
            let blocks = graph
                .nodes
                .iter()
                .filter_map(|node| match node {
                    ScriptNode::Block(id) => Some(id.clone()),
                    ScriptNode::LoopBoundary(_) => None,
                })
                .collect::<HashSet<_>>();
            (procedure.definition_id.clone(), blocks)
        })
        .collect::<HashMap<_, _>>();
    let mut used_arguments = procedures
        .iter()
        .filter(|procedure| reachable.contains(&procedure.definition_id))
        .map(|procedure| {
            (
                procedure.definition_id.clone(),
                vec![false; procedure.argument_ids.len()],
            )
        })
        .collect::<HashMap<_, _>>();

    loop {
        let previous = numeric.clone();
        for procedure in procedures
            .iter()
            .filter(|procedure| reachable.contains(&procedure.definition_id))
        {
            let Some(block_ids) = procedure_blocks.get(&procedure.definition_id) else {
                continue;
            };
            let Some(arguments) = numeric.get_mut(&procedure.definition_id) else {
                continue;
            };
            for (id, block) in &target.blocks {
                if !matches!(
                    block.opcode.as_str(),
                    "argument_reporter_string_number" | "argument_reporter_boolean"
                ) || !argument_reporter_belongs_to_procedure(target, block, block_ids)
                {
                    continue;
                }
                let is_number_reporter = block.opcode == "argument_reporter_string_number";
                let Some(index) = field_value(block, "VALUE")
                    .and_then(Value::as_str)
                    .and_then(|name| {
                        procedure
                            .argument_names
                            .iter()
                            .position(|candidate| candidate == name)
                    })
                else {
                    continue;
                };
                if let Some(used) = used_arguments
                    .get_mut(&procedure.definition_id)
                    .and_then(|arguments| arguments.get_mut(index))
                {
                    *used = true;
                }
                let numeric_use = is_number_reporter
                    && block
                        .parent
                        .as_deref()
                        .and_then(|parent_id| target.blocks.get(parent_id))
                        .is_some_and(|parent| {
                            argument_input_is_numeric_context(parent, id, procedures, &previous)
                        });
                if !numeric_use {
                    arguments[index] = false;
                }
            }
        }
        if numeric == previous {
            for (procedure_id, arguments) in &mut numeric {
                let Some(used) = used_arguments.get(procedure_id) else {
                    continue;
                };
                for (is_numeric, is_used) in arguments.iter_mut().zip(used) {
                    *is_numeric &= *is_used;
                }
            }
            return numeric;
        }
    }
}

fn argument_reporter_belongs_to_procedure(
    target: &ScratchTarget,
    reporter: &ScratchBlock,
    procedure_blocks: &HashSet<String>,
) -> bool {
    let mut parent = reporter.parent.as_deref();
    let mut seen = HashSet::new();
    while let Some(parent_id) = parent {
        if !seen.insert(parent_id) {
            return false;
        }
        if procedure_blocks.contains(parent_id) {
            return true;
        }
        parent = target
            .blocks
            .get(parent_id)
            .and_then(|block| block.parent.as_deref());
    }
    false
}

fn argument_input_is_numeric_context(
    parent: &ScratchBlock,
    child_id: &str,
    procedures: &[ProcedureSignature],
    numeric_arguments: &HashMap<String, Vec<bool>>,
) -> bool {
    let input_name = parent.inputs.iter().find_map(|(name, input)| {
        matches!(parse_input(input), Some(InputValue::BlockReference(id)) if id == child_id)
            .then_some(name.as_str())
    });
    let Some(input_name) = input_name else {
        return false;
    };
    match parent.opcode.as_str() {
        "operator_add" | "operator_subtract" | "operator_multiply" | "operator_divide"
        | "operator_mod" | "operator_round" | "operator_random" | "operator_mathop" => true,
        "data_changevariableby" => input_name == "VALUE",
        "control_repeat" => input_name == "TIMES",
        "data_itemoflist"
        | "data_deleteoflist"
        | "data_replaceitemoflist"
        | "data_insertatlist" => input_name == "INDEX",
        "motion_movesteps"
        | "motion_turnright"
        | "motion_turnleft"
        | "motion_goto"
        | "motion_glideto"
        | "motion_changexby"
        | "motion_setx"
        | "motion_changeyby"
        | "motion_sety"
        | "motion_pointindirection"
        | "motion_pointtowards"
        | "motion_gotoxy"
        | "motion_glidesecstoxy"
        | "motion_changexyby" => !matches!(input_name, "TO" | "TOWARDS"),
        "procedures_call" => {
            let Some(callee) = procedure_for_call(parent, procedures) else {
                return false;
            };
            let argument_ids = parent
                .mutation
                .get("argumentids")
                .and_then(Value::as_str)
                .and_then(|encoded| serde_json::from_str::<Vec<String>>(encoded).ok())
                .unwrap_or_default();
            let Some(index) = argument_ids.iter().position(|id| id == input_name) else {
                return false;
            };
            numeric_arguments
                .get(&callee.definition_id)
                .and_then(|arguments| arguments.get(index))
                .copied()
                .unwrap_or(false)
        }
        _ => false,
    }
}

fn input_is_proven_numeric_argument(
    target: &ScratchTarget,
    input: Option<&serde_json::Value>,
    variables: &HashMap<String, VariableInfo>,
    caller: Option<&ProcedureSignature>,
    caller_arguments: Option<&[bool]>,
) -> bool {
    match input.and_then(parse_input) {
        Some(InputValue::Number(_)) => true,
        Some(InputValue::VariableReference(reference)) => {
            super::super::project::variable_for_target(target, reference.id, variables)
                .is_some_and(|variable| variable.numeric && !variable.monitor_visible)
        }
        Some(InputValue::BlockReference(id)) => {
            let Some(block) = target.blocks.get(id) else {
                return false;
            };
            match block.opcode.as_str() {
                "argument_reporter_string_number" | "argument_reporter_boolean" => {
                    let Some(index) = field_value(block, "VALUE")
                        .and_then(Value::as_str)
                        .and_then(|name| {
                            caller?
                                .argument_names
                                .iter()
                                .position(|candidate| candidate == name)
                        })
                    else {
                        return false;
                    };
                    caller_arguments
                        .and_then(|arguments| arguments.get(index))
                        .copied()
                        .unwrap_or(false)
                }
                "data_variable" => variable_field(block, "VARIABLE", variables)
                    .is_some_and(|variable| variable.numeric && !variable.monitor_visible),
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
                | "math_angle" => true,
                _ => false,
            }
        }
        _ => false,
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
    (start_pc, graph.repeat_slots.len())
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
        line(output, indent + 1, "script.pop_frame(); continue;");
        line(output, indent, "},");
        return;
    };
    let next_pc = block
        .next
        .as_ref()
        .and_then(|next| graph.block_pcs.get(next).copied())
        .unwrap_or(graph.fallthrough_pcs[id]);
    if block.opcode == "control_stop"
        && field_value(block, "STOP_OPTION").and_then(Value::as_str) == Some("this script")
        && let Some(return_pc) = graph.inlined_return_pcs.get(id).copied()
    {
        line(
            output,
            indent + 1,
            &format!(
                "script.frames.last_mut().unwrap().pc = {};",
                pc_literal(return_pc)
            ),
        );
        line(output, indent + 1, "continue;");
        line(output, indent, "},");
        return;
    }
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
        "procedures_call" if graph.inlined_calls.contains_key(id) => {
            let entry_pc = graph.inlined_calls[id];
            if !no_warp_timer {
                line(output, indent + 1, "if script.warp_budget.expired(clock) {");
                line(
                    output,
                    indent + 2,
                    &format!(
                        "script.frames.last_mut().unwrap().pc = {};",
                        pc_literal(entry_pc)
                    ),
                );
                line(output, indent + 2, "return ScriptStatus::Yield;");
                line(output, indent + 1, "}");
            }
            line(
                output,
                indent + 1,
                &format!(
                    "script.frames.last_mut().unwrap().pc = {};",
                    pc_literal(entry_pc)
                ),
            );
            line(output, indent + 1, "continue;");
        }
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
                call_pc: pc,
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
    } else if let Some((from, to, integer_bounds)) = numeric_random {
        line(
            output,
            indent + 3,
            &format!(
                "scratch_list_changed = with_scratch_random(|scratch_rng| state.list_mut({}).add_generated_numbers(repeat_chunk, || scratch_rng.between_with_integer_bounds({from:?}_f64, {to:?}_f64, {integer_bounds})));",
                list.slot
            ),
        );
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
            emitted_blocks.extend(body_plan.blocks.iter().cloned());
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
        let has_optimized_locals = has_locals;
        let entry_guards = numeric_entry_guards(&numeric, &needed);
        let has_dynamic_fallback = has_optimized_locals && !entry_guards.is_empty();
        let mut typed_variables = variables.clone();
        for variable in typed_variables.values_mut() {
            if numeric_slots.contains(&variable.slot) {
                variable.index_read_override = Some(format!("scratch_typed_{}", variable.slot));
            } else if dynamic.iter().any(|v| v.slot == variable.slot) {
                variable.index_read_override = Some(format!("&scratch_dynamic_{}", variable.slot));
            }
        }
        if has_optimized_locals {
            let defer_slots = deferred
                .iter()
                .copied()
                .chain(dynamic.iter().map(|variable| variable.slot))
                .collect::<HashSet<_>>();
            if !defer_slots.is_empty() {
                line(
                    output,
                    5,
                    &format!(
                        "let scratch_defer_writes = state.can_defer_variable_write_mask({});",
                        variable_write_mask(defer_slots.iter().copied())
                    ),
                );
            }
            if has_dynamic_fallback {
                emit_numeric_entry_guard(output, 5, &entry_guards);
            }
            let local_indent = 5 + usize::from(has_dynamic_fallback);
            emit_numeric_local_initializers(output, local_indent, &numeric, &needed);
            for variable in numeric.iter().filter(|v| deferred.contains(&v.slot)) {
                line(
                    output,
                    local_indent,
                    &format!("let mut scratch_dirty_{} = false;", variable.slot),
                );
                if textual.contains(&variable.slot) {
                    line(
                        output,
                        local_indent,
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
                    local_indent,
                    &format!("let mut scratch_dynamic_{} = {initial};", variable.slot),
                );
                line(
                    output,
                    local_indent,
                    &format!("let mut scratch_dynamic_dirty_{} = false;", variable.slot),
                );
            }
        }
        let mut last = id;
        while let Some(next) = target.blocks[last].next.as_deref() {
            last = next;
        }
        let no_numeric_slots = HashSet::new();
        for optimized in 0..if has_dynamic_fallback { 2 } else { 1 } {
            let typed = has_optimized_locals && optimized == 0;
            let block_indent = if has_dynamic_fallback { 6 } else { 5 };
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
            let loop_indent = block_indent;
            if repeat {
                line(output, loop_indent, "loop {");
            }
            let body_indent = loop_indent + usize::from(repeat);
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
                line(output, loop_indent, "}");
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
            if has_dynamic_fallback && optimized == 0 {
                line(output, 5, "} else {");
            }
        }
        if has_dynamic_fallback {
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
        let entry_guards = numeric_entry_guards(&numeric, &needed);
        let has_dynamic_fallback = !entry_guards.is_empty();
        let mut typed_variables = variables.clone();
        for variable in typed_variables.values_mut() {
            if numeric_slots.contains(&variable.slot) {
                variable.index_read_override = Some(format!("scratch_typed_{}", variable.slot));
            }
        }
        line(output, 4, &format!("{} => {{", graph.block_pcs[id]));
        if !deferred.is_empty() {
            line(
                output,
                5,
                &format!(
                    "let scratch_defer_writes = state.can_defer_variable_write_mask({});",
                    variable_write_mask(deferred.iter().copied())
                ),
            );
        }
        if has_dynamic_fallback {
            emit_numeric_entry_guard(output, 5, &entry_guards);
        }
        if !numeric.is_empty() {
            let local_indent = 5 + usize::from(has_dynamic_fallback);
            emit_numeric_local_initializers(output, local_indent, &numeric, &needed);
            for variable in numeric.iter().filter(|v| deferred.contains(&v.slot)) {
                line(
                    output,
                    local_indent,
                    &format!("let mut scratch_dirty_{} = false;", variable.slot),
                );
                if textual.contains(&variable.slot) {
                    line(
                        output,
                        local_indent,
                        &format!("let mut scratch_text_{} = false;", variable.slot),
                    );
                }
            }
        }
        let empty = HashSet::new();
        for version in 0..if has_dynamic_fallback { 2 } else { 1 } {
            let typed = !numeric.is_empty() && version == 0;
            let level = 5 + usize::from(has_dynamic_fallback);
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
            emit_partial_structured_chain(
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
            if typed {
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
            if typed {
                emit_deferred_numeric_commits(output, level, &numeric, &deferred, &textual);
                if has_dynamic_fallback {
                    line(output, 5, "} else {");
                }
            }
        }
        if has_dynamic_fallback {
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
        let entry_guards = numeric_entry_guards(&numeric, &needed);
        let has_dynamic_fallback = !entry_guards.is_empty();
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
        if !deferred.is_empty() {
            line(
                output,
                5,
                &format!(
                    "let scratch_defer_writes = state.can_defer_variable_write_mask({});",
                    variable_write_mask(deferred.iter().copied())
                ),
            );
        }
        if !numeric.is_empty() {
            if has_dynamic_fallback {
                emit_numeric_entry_guard(output, 5, &entry_guards);
            }
            let local_indent = 5 + usize::from(has_dynamic_fallback);
            emit_numeric_local_initializers(output, local_indent, &numeric, &needed);
            for variable in numeric.iter().filter(|v| deferred.contains(&v.slot)) {
                line(
                    output,
                    local_indent,
                    &format!("let mut scratch_dirty_{} = false;", variable.slot),
                );
                if textual.contains(&variable.slot) {
                    line(
                        output,
                        local_indent,
                        &format!("let mut scratch_text_{} = false;", variable.slot),
                    );
                }
            }
        }
        let empty = HashSet::new();
        for version in 0..if has_dynamic_fallback { 2 } else { 1 } {
            let typed = !numeric.is_empty() && version == 0;
            let level = 5 + usize::from(has_dynamic_fallback);
            line(output, level, "let mut scratch_region_escaped = false;");
            let mut region_output = String::new();
            let requires_escape_label = emit_partial_structured_chain(
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
                &mut region_output,
                level + 1,
            );
            line(
                output,
                level,
                if requires_escape_label {
                    "'scratch_compiled_loop: {"
                } else {
                    "{"
                },
            );
            output.push_str(&region_output);
            line(output, level, "}");
            if typed {
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
            if has_dynamic_fallback && typed {
                line(output, 5, "} else {");
            }
        }
        if has_dynamic_fallback {
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
        let entry_guards = numeric_entry_guards(&numeric, &needed);
        let has_dynamic_fallback = !entry_guards.is_empty();
        let mut typed_variables = variables.clone();
        for variable in typed_variables.values_mut() {
            if slots.contains(&variable.slot) {
                variable.index_read_override = Some(format!("scratch_typed_{}", variable.slot));
            }
        }
        line(output, 4, &format!("{} => {{", graph.block_pcs[id]));
        if !deferred.is_empty() {
            line(
                output,
                5,
                &format!(
                    "let scratch_defer_writes = state.can_defer_variable_write_mask({});",
                    variable_write_mask(deferred.iter().copied())
                ),
            );
        }
        if has_dynamic_fallback {
            emit_numeric_entry_guard(output, 5, &entry_guards);
        }
        let body_indent = 5 + usize::from(has_dynamic_fallback);
        emit_numeric_local_initializers(output, body_indent, &numeric, &needed);
        for variable in numeric.iter().filter(|v| deferred.contains(&v.slot)) {
            line(
                output,
                body_indent,
                &format!("let mut scratch_dirty_{} = false;", variable.slot),
            );
        }
        for version in 0..if has_dynamic_fallback { 2 } else { 1 } {
            let typed = version == 0;
            let statement_indent = body_indent;
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
                            },
                            statement,
                            &variable,
                            output,
                            statement_indent,
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
                            statement_indent,
                            &format!("let scratch_change = scratch_to_number({value});"),
                        );
                        line(
                            output,
                            statement_indent,
                            &format!(
                                "scratch_typed_{0} = scratch_number_operand(scratch_typed_{0}) + scratch_change;",
                                variable.slot
                            ),
                        );
                        emit_numeric_write_commit(output, statement_indent, &variable, &deferred);
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
                        statement_indent,
                    );
                }
            }
            if typed {
                emit_deferred_numeric_commits(
                    output,
                    body_indent,
                    &numeric,
                    &deferred,
                    &HashSet::new(),
                );
                if has_dynamic_fallback {
                    line(output, 5, "} else {");
                }
            }
        }
        if has_dynamic_fallback {
            line(output, 5, "}");
        }
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
