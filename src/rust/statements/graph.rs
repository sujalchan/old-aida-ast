use super::*;

pub(super) struct ScriptGraph {
    pub(super) nodes: Vec<ScriptNode>,
    pub(super) block_pcs: HashMap<String, usize>,
    pub(super) boundary_pcs: HashMap<String, usize>,
    pub(super) fallthrough_pcs: HashMap<String, usize>,
    pub(super) repeat_slots: HashMap<String, usize>,
    pub(super) numeric_loop_slots: HashMap<String, usize>,
}

pub(super) struct ProcedureEntry {
    pub(super) function_id: usize,
    pub(super) start_pc: usize,
    pub(super) repeat_count: usize,
    pub(super) direct_name: Option<String>,
    pub(super) direct_warp_name: Option<String>,
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
            numeric_loop_slots: HashMap::new(),
        }
    }

    pub(super) fn add_chain(
        &mut self,
        target: &ScratchTarget,
        first: Option<&str>,
        continuation: usize,
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
            self.nodes.push(ScriptNode::Block(id.to_owned()));
            let next = block.next.as_deref();
            self.fallthrough_pcs.insert(id.to_owned(), continuation);

            if matches!(
                block.opcode.as_str(),
                "control_repeat" | "control_repeat_until" | "control_while" | "control_forever"
            ) {
                let loop_state_slot = self.numeric_loop_slots.len();
                self.numeric_loop_slots
                    .insert(id.to_owned(), loop_state_slot);
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
                self.add_chain(target, branch, boundary_pc);
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

                self.add_chain(target, next, continuation);
                let branch_continuation = next
                    .and_then(|next_id| self.block_pcs.get(next_id).copied())
                    .unwrap_or(continuation);
                self.fallthrough_pcs
                    .insert(id.to_owned(), branch_continuation);
                self.add_chain(target, then_branch, branch_continuation);
                if block.opcode == "control_if_else" {
                    self.add_chain(target, else_branch, branch_continuation);
                }
                return;
            }

            current = next;
        }
    }

    pub(super) fn frame_state_count(&self) -> usize {
        self.repeat_slots.len() + self.numeric_loop_slots.len()
    }
}

// The whitelist admits only bounded leaf bodies. Loops, calls and every
// potentially suspending command remain on the resumable path.
