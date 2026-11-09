//! lower Scratch statement chains into direct and resumable Rust code.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::Value;

use crate::parser::{InputValue, ScratchBlock, ScratchTarget, parse_input};

use super::{
    expressions::{
        LocalValueKind, boolean_input, collect_numeric_local_slots, constant_numeric_input,
        constant_random_bounds, expression, expression_argument, local_expression, local_to_bool,
        local_to_number,
    },
    literals::{field_value, line, number_literal, quote},
    project::{ListInfo, VariableInfo},
    scripts::ProcedureSignature,
};

#[derive(Clone, Copy)]
pub(super) struct StatementContext<'a> {
    pub(super) target: &'a ScratchTarget,
    pub(super) variables: &'a HashMap<String, VariableInfo>,
    pub(super) lists: &'a HashMap<String, ListInfo>,
    pub(super) arguments: &'a HashMap<String, String>,
}

#[derive(Clone, Copy)]
pub(super) struct NumericLoopContext<'a> {
    target: &'a ScratchTarget,
    graph: &'a ScriptGraph,
    variables: &'a HashMap<String, VariableInfo>,
    fast_math: bool,
    no_warp_timer: bool,
}

mod direct;
mod graph;
mod opcodes;
mod optimization;
mod procedures;
mod resumable;
mod structured;

use direct::*;
use graph::*;
use opcodes::*;
use optimization::*;
use procedures::*;
use structured::*;

pub(super) use direct::{DirectProcedureOptions, generate_direct_procedures};
pub(super) use opcodes::{procedure_for_call, variable_field};
pub(super) use resumable::{
    ResumableScriptOptions, generate_resumable_procedure_step, generate_resumable_script,
    prepare_resumable_procedures,
};

pub(super) fn variable_write_mask(slots: impl IntoIterator<Item = usize>) -> String {
    let slots = slots.into_iter().collect::<BTreeSet<_>>();
    let Some(max_slot) = slots.last().copied() else {
        return "&[]".to_owned();
    };
    let mut words = vec![0_u64; max_slot / 64 + 1];
    for slot in slots {
        words[slot / 64] |= 1_u64 << (slot % 64);
    }
    format!(
        "&[{}]",
        words
            .iter()
            .map(|word| format!("0x{word:016x}_u64"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}
