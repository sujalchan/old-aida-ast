use super::*;

pub(super) struct ScriptGraph {
    pub(super) nodes: Vec<ScriptNode>,
    pub(super) block_pcs: HashMap<String, usize>,
    pub(super) boundary_pcs: HashMap<String, usize>,
    pub(super) fallthrough_pcs: HashMap<String, usize>,
    pub(super) repeat_slots: HashMap<String, usize>,
    pub(super) inlined_calls: HashMap<String, usize>,
    pub(super) inlined_return_pcs: HashMap<String, usize>,
}

pub(super) struct ProcedureEntry {
    pub(super) function_id: usize,
    pub(super) start_pc: usize,
    pub(super) repeat_count: usize,
    pub(super) direct_name: Option<String>,
    pub(super) direct_warp_name: Option<String>,
    pub(super) direct_warp_returns_status: bool,
    pub(super) bounded_warp: bool,
    pub(super) numeric_arguments: Option<Vec<bool>>,
    pub(super) numeric_call_arguments: Option<Vec<bool>>,
    pub(super) numeric_warp_arguments: Option<Vec<bool>>,
    pub(super) numeric_direct: bool,
}

pub(super) enum ScriptNode {
    Block(String),
    LoopBoundary(String),
}

impl ScriptGraph {
    pub(super) fn new() -> Self {
        Self {
            nodes: Vec::new(),
            block_pcs: HashMap::new(),
            boundary_pcs: HashMap::new(),
            fallthrough_pcs: HashMap::new(),
            repeat_slots: HashMap::new(),
            inlined_calls: HashMap::new(),
            inlined_return_pcs: HashMap::new(),
        }
    }

    pub(super) fn add_chain(
        &mut self,
        target: &ScratchTarget,
        first: Option<&str>,
        continuation: usize,
    ) {
        self.add_chain_inner(
            target,
            first,
            continuation,
            None,
            &HashSet::new(),
            &mut HashSet::new(),
            None,
        );
    }

    pub(super) fn add_chain_with_inlined_calls(
        &mut self,
        target: &ScratchTarget,
        first: Option<&str>,
        continuation: usize,
        procedures: &[ProcedureSignature],
        inline_calls: &HashSet<String>,
        active_procedure: &str,
    ) {
        let mut active_procedures = HashSet::new();
        active_procedures.insert(active_procedure.to_owned());
        self.add_chain_inner(
            target,
            first,
            continuation,
            Some(procedures),
            inline_calls,
            &mut active_procedures,
            None,
        );
    }

    fn add_chain_inner(
        &mut self,
        target: &ScratchTarget,
        first: Option<&str>,
        continuation: usize,
        procedures: Option<&[ProcedureSignature]>,
        inline_calls: &HashSet<String>,
        active_procedures: &mut HashSet<String>,
        inlined_return_pc: Option<usize>,
    ) {
        let mut current = first;
        let mut chain_seen = HashSet::new();
        while let Some(id) = current {
            if !chain_seen.insert(id.to_owned()) || self.block_pcs.contains_key(id) {
                break;
            }
            let Some(block) = target.blocks.get(id) else {
                break;
            };
            let pc = self.nodes.len();
            self.block_pcs.insert(id.to_owned(), pc);
            if let Some(return_pc) = inlined_return_pc {
                self.inlined_return_pcs.insert(id.to_owned(), return_pc);
            }
            self.nodes.push(ScriptNode::Block(id.to_owned()));
            let next = block.next.as_deref();
            self.fallthrough_pcs.insert(id.to_owned(), continuation);

            if inline_calls.contains(id)
                && let Some(procedure) =
                    procedures.and_then(|procedures| procedure_for_call(block, procedures))
                && active_procedures.insert(procedure.definition_id.clone())
            {
                self.add_chain_inner(
                    target,
                    next,
                    continuation,
                    procedures,
                    inline_calls,
                    active_procedures,
                    inlined_return_pc,
                );
                let return_pc = next
                    .and_then(|next_id| self.block_pcs.get(next_id).copied())
                    .unwrap_or(continuation);
                let callee_first = target
                    .blocks
                    .get(&procedure.definition_id)
                    .and_then(|definition| definition.next.as_deref());
                let entry_pc = callee_first.map(|_| self.nodes.len()).unwrap_or(return_pc);
                self.inlined_calls.insert(id.to_owned(), entry_pc);
                self.add_chain_inner(
                    target,
                    callee_first,
                    return_pc,
                    procedures,
                    inline_calls,
                    active_procedures,
                    Some(return_pc),
                );
                active_procedures.remove(&procedure.definition_id);
                return;
            }

            if matches!(
                block.opcode.as_str(),
                "control_repeat" | "control_repeat_until" | "control_while" | "control_forever"
            ) {
                let boundary_pc = self.nodes.len();
                self.boundary_pcs.insert(id.to_owned(), boundary_pc);
                self.nodes.push(ScriptNode::LoopBoundary(id.to_owned()));
                if block.opcode == "control_repeat" {
                    let slot = self.repeat_slots.len();
                    self.repeat_slots.insert(id.to_owned(), slot);
                }
                let branch =
                    block.inputs.get("SUBSTACK").and_then(parse_input).and_then(
                        |value| match value {
                            InputValue::BlockReference(id) => Some(id),
                            _ => None,
                        },
                    );
                self.add_chain_inner(
                    target,
                    branch,
                    boundary_pc,
                    procedures,
                    inline_calls,
                    active_procedures,
                    inlined_return_pc,
                );
            }

            if matches!(block.opcode.as_str(), "control_if" | "control_if_else") {
                let then_branch = block.inputs.get("SUBSTACK").and_then(parse_input).and_then(
                    |value| match value {
                        InputValue::BlockReference(id) => Some(id),
                        _ => None,
                    },
                );
                let else_branch = block
                    .inputs
                    .get("SUBSTACK2")
                    .and_then(parse_input)
                    .and_then(|value| match value {
                        InputValue::BlockReference(id) => Some(id),
                        _ => None,
                    });

                self.add_chain_inner(
                    target,
                    next,
                    continuation,
                    procedures,
                    inline_calls,
                    active_procedures,
                    inlined_return_pc,
                );
                let branch_continuation = next
                    .and_then(|next_id| self.block_pcs.get(next_id).copied())
                    .unwrap_or(continuation);
                self.fallthrough_pcs
                    .insert(id.to_owned(), branch_continuation);
                self.add_chain_inner(
                    target,
                    then_branch,
                    branch_continuation,
                    procedures,
                    inline_calls,
                    active_procedures,
                    inlined_return_pc,
                );
                if block.opcode == "control_if_else" {
                    self.add_chain_inner(
                        target,
                        else_branch,
                        branch_continuation,
                        procedures,
                        inline_calls,
                        active_procedures,
                        inlined_return_pc,
                    );
                }
                return;
            }

            current = next;
        }
    }
}

// The whitelist admits only bounded leaf bodies. Loops, calls and every
// potentially suspending command remain on the resumable path.
