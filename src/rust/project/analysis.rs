use super::*;

pub(super) fn rust_identifiers(source: &str) -> HashSet<String> {
    let characters = source.chars().collect::<Vec<_>>();
    let mut identifiers = HashSet::new();
    let mut index = 0;
    while index < characters.len() {
        if characters[index] == '"' {
            index += 1;
            while index < characters.len() {
                if characters[index] == '\\' {
                    index += 2;
                } else if characters[index] == '"' {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
            continue;
        }
        if characters[index] == '/' && characters.get(index + 1) == Some(&'/') {
            index += 2;
            while index < characters.len() && characters[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if characters[index] == '/' && characters.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < characters.len()
                && !(characters[index] == '*' && characters[index + 1] == '/')
            {
                index += 1;
            }
            index = (index + 2).min(characters.len());
            continue;
        }
        if characters[index] == '_' || characters[index].is_ascii_alphabetic() {
            let start = index;
            index += 1;
            while index < characters.len()
                && (characters[index] == '_' || characters[index].is_ascii_alphanumeric())
            {
                index += 1;
            }
            identifiers.insert(characters[start..index].iter().collect());
            continue;
        }
        index += 1;
    }
    identifiers
}

// assign each Scratch variable ID a stable shared-state slot.
pub(super) fn allocate_variables(project: &ScratchProject) -> HashMap<String, VariableInfo> {
    let numeric_variables = numeric_variables(project);
    let canonical_numeric_variables = canonical_numeric_variables(project);
    let non_nan_variables = non_nan_variables(project);
    let mut entries = BTreeMap::new();
    for (target_index, target) in project.targets.iter().enumerate() {
        let mut variable_ids = target.variable_order.clone();
        let mut seen = variable_ids.iter().cloned().collect::<HashSet<_>>();
        variable_ids.extend(
            target
                .variables
                .keys()
                .filter(|id| seen.insert((*id).clone()))
                .cloned(),
        );
        for (target_variable_order, id) in variable_ids.iter().enumerate() {
            let Some(variable) = target.variables.get(id) else {
                continue;
            };
            let key = variable_key(target_index, id);
            let monitor_visible = project.monitors.iter().any(|monitor| {
                monitor.visible
                    && monitor.id == *id
                    && monitor_variable_target_index(project, monitor) == Some(target_index)
            });
            entries.insert(
                key,
                VariableInfo {
                    id: id.clone(),
                    slot: 0,
                    name: variable.name.clone(),
                    target: target.name.clone(),
                    is_stage: target.is_stage,
                    target_index,
                    target_variable_order,
                    numeric: numeric_variables.contains(&variable_key(target_index, id)),
                    canonical_numeric: canonical_numeric_variables
                        .contains(&variable_key(target_index, id)),
                    non_nan: non_nan_variables.contains(&variable_key(target_index, id)),
                    monitor_visible,
                    index_read_override: None,
                },
            );
        }
    }
    let mut output = HashMap::new();
    for (slot, (key, mut info)) in entries.into_iter().enumerate() {
        info.slot = slot;
        output.insert(key, info);
    }
    output
}

fn monitor_variable_target_index(
    project: &ScratchProject,
    monitor: &crate::parser::ScratchMonitor,
) -> Option<usize> {
    let target = monitor.sprite_name.as_deref().map_or_else(
        || project.targets.iter().position(|target| target.is_stage),
        |sprite_name| {
            project
                .targets
                .iter()
                .position(|target| target.name == sprite_name)
        },
    )?;
    project.targets[target]
        .variables
        .contains_key(&monitor.id)
        .then_some(target)
}

// prove numeric variables non-nan when every write preserves that property.
pub(super) fn non_nan_variables(project: &ScratchProject) -> HashSet<String> {
    let mut non_nan = HashSet::new();
    for (target_index, target) in project.targets.iter().enumerate() {
        for (id, variable) in &target.variables {
            if variable.value.as_f64().is_some_and(|value| !value.is_nan()) {
                non_nan.insert(variable_key(target_index, id));
            }
        }
    }

    loop {
        let previous = non_nan.clone();
        for target in &project.targets {
            for block in target.blocks.values() {
                if !matches!(
                    block.opcode.as_str(),
                    "data_setvariableto" | "data_changevariableby"
                ) {
                    continue;
                }
                let Some(id) = block
                    .fields
                    .get("VARIABLE")
                    .and_then(|field| field.as_array())
                    .and_then(|field| field.get(1))
                    .and_then(|id| id.as_str())
                else {
                    continue;
                };
                let preserves_non_nan = if block.opcode == "data_setvariableto" {
                    input_is_non_nan_number(
                        target,
                        block.inputs.get("VALUE"),
                        &previous,
                        &mut HashSet::new(),
                    )
                } else {
                    constant_numeric_input(target, block.inputs.get("VALUE"), &mut HashSet::new())
                        .is_some_and(f64::is_finite)
                };
                if !preserves_non_nan {
                    non_nan.remove(&variable_key_for_target(target, id));
                }
            }
        }
        if non_nan == previous {
            return non_nan;
        }
    }
}

pub(super) fn input_is_non_nan_number(
    target: &ScratchTarget,
    input: Option<&serde_json::Value>,
    non_nan: &HashSet<String>,
    active: &mut HashSet<String>,
) -> bool {
    match input.and_then(parse_input) {
        Some(InputValue::Number(value)) => value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
            .is_some_and(|value: f64| !value.is_nan()),
        Some(InputValue::VariableReference(reference)) => {
            non_nan.contains(&variable_key_for_target(target, reference.id))
        }
        Some(InputValue::BlockReference(id)) => {
            if !active.insert(id.to_owned()) {
                return false;
            }
            let result = target.blocks.get(id).is_some_and(|block| {
                if block.opcode == "data_variable" {
                    return block
                        .fields
                        .get("VARIABLE")
                        .and_then(|field| field.as_array())
                        .and_then(|field| field.get(1))
                        .and_then(|id| id.as_str())
                        .is_some_and(|id| non_nan.contains(&variable_key_for_target(target, id)));
                }
                matches!(
                    block.opcode.as_str(),
                    "math_number"
                        | "math_integer"
                        | "math_whole_number"
                        | "math_positive_number"
                        | "math_angle"
                ) && block
                    .fields
                    .get("NUM")
                    .and_then(|field| field.as_array())
                    .and_then(|field| field.first())
                    .and_then(|value| {
                        value
                            .as_f64()
                            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
                    })
                    .is_some_and(|value: f64| !value.is_nan())
            });
            active.remove(id);
            result
        }
        _ => false,
    }
}

// prove variables numeric only when their saved value and every assignment are numeric.
pub(super) fn numeric_variables(project: &ScratchProject) -> HashSet<String> {
    let mut numeric = HashSet::new();
    for (target_index, target) in project.targets.iter().enumerate() {
        for (id, variable) in &target.variables {
            if variable.value.is_number() && !variable.is_cloud {
                numeric.insert(variable_key(target_index, id));
            }
        }
    }

    loop {
        let previous = numeric.clone();
        for target in &project.targets {
            for block in target.blocks.values() {
                if block.opcode != "data_setvariableto" {
                    continue;
                }
                let Some(id) = block
                    .fields
                    .get("VARIABLE")
                    .and_then(|field| field.as_array())
                    .and_then(|field| field.get(1))
                    .and_then(|id| id.as_str())
                else {
                    continue;
                };
                if !input_is_numeric(
                    target,
                    block.inputs.get("VALUE"),
                    &previous,
                    &mut HashSet::new(),
                ) {
                    numeric.remove(&variable_key_for_target(target, id));
                }
            }
        }
        if numeric == previous {
            return numeric;
        }
    }
}

/// Prove that a value is either a Number or canonical numeric text. This is
/// sufficient for numeric execution that preserves Scratch's comparison text,
/// while keeping arbitrary strings on the dynamic path.
pub(super) fn canonical_numeric_variables(project: &ScratchProject) -> HashSet<String> {
    let mut canonical_numeric = HashSet::new();
    for (target_index, target) in project.targets.iter().enumerate() {
        for (id, variable) in &target.variables {
            if !variable.is_cloud
                && (variable.value.is_number()
                    || variable
                        .value
                        .as_str()
                        .is_some_and(is_canonical_integer_text))
            {
                canonical_numeric.insert(variable_key(target_index, id));
            }
        }
    }

    loop {
        let previous = canonical_numeric.clone();
        for target in &project.targets {
            for block in target.blocks.values() {
                if block.opcode != "data_setvariableto" {
                    continue;
                }
                let Some(id) = block
                    .fields
                    .get("VARIABLE")
                    .and_then(|field| field.as_array())
                    .and_then(|field| field.get(1))
                    .and_then(|id| id.as_str())
                else {
                    continue;
                };
                if !input_is_canonical_numeric(
                    target,
                    block.inputs.get("VALUE"),
                    &previous,
                    &mut HashSet::new(),
                ) {
                    canonical_numeric.remove(&variable_key_for_target(target, id));
                }
            }
        }
        if canonical_numeric == previous {
            return canonical_numeric;
        }
    }
}

fn input_is_canonical_numeric(
    target: &ScratchTarget,
    input: Option<&serde_json::Value>,
    canonical_numeric: &HashSet<String>,
    active: &mut HashSet<String>,
) -> bool {
    match input.and_then(parse_input) {
        Some(InputValue::Number(_)) => true,
        Some(InputValue::String(value)) => value.as_str().is_some_and(is_canonical_integer_text),
        Some(InputValue::VariableReference(reference)) => {
            canonical_numeric.contains(&variable_key_for_target(target, reference.id))
        }
        Some(InputValue::BlockReference(id)) => {
            if !active.insert(id.to_owned()) {
                return false;
            }
            let result = target
                .blocks
                .get(id)
                .is_some_and(|block| match block.opcode.as_str() {
                    "data_variable" => block
                        .fields
                        .get("VARIABLE")
                        .and_then(|field| field.as_array())
                        .and_then(|field| field.get(1))
                        .and_then(|id| id.as_str())
                        .is_some_and(|id| {
                            canonical_numeric.contains(&variable_key_for_target(target, id))
                        }),
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
                });
            active.remove(id);
            result
        }
        _ => false,
    }
}

fn is_canonical_integer_text(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|number| number.to_string() == value)
}

pub(super) fn input_is_numeric(
    target: &ScratchTarget,
    input: Option<&serde_json::Value>,
    numeric: &HashSet<String>,
    active: &mut HashSet<String>,
) -> bool {
    match input.and_then(parse_input) {
        Some(InputValue::Number(_)) => true,
        Some(InputValue::VariableReference(reference)) => {
            numeric.contains(&variable_key_for_target(target, reference.id))
        }
        Some(InputValue::BlockReference(id)) => {
            if !active.insert(id.to_owned()) {
                return false;
            }
            let result = target
                .blocks
                .get(id)
                .is_some_and(|block| match block.opcode.as_str() {
                    "operator_add" | "operator_subtract" | "operator_multiply"
                    | "operator_divide" | "operator_mod" | "operator_round" | "operator_random"
                    | "operator_mathop" | "data_lengthoflist" | "data_itemnumoflist" => true,
                    "data_variable" => block
                        .fields
                        .get("VARIABLE")
                        .and_then(|field| field.as_array())
                        .and_then(|field| field.get(1))
                        .and_then(|id| id.as_str())
                        .is_some_and(|id| numeric.contains(&variable_key_for_target(target, id))),
                    "math_number"
                    | "math_integer"
                    | "math_whole_number"
                    | "math_positive_number"
                    | "math_angle" => true,
                    _ => false,
                });
            active.remove(id);
            result
        }
        _ => false,
    }
}

// assign each scratch list id a stable shared-state slot.
pub(super) fn allocate_lists(project: &ScratchProject) -> HashMap<String, ListInfo> {
    let written_lists = project
        .targets
        .iter()
        .flat_map(|target| target.blocks.values())
        .filter(|block| {
            matches!(
                block.opcode.as_str(),
                "data_addtolist"
                    | "data_deleteoflist"
                    | "data_deletealloflist"
                    | "data_insertatlist"
                    | "data_replaceitemoflist"
            )
        })
        .filter_map(|block| block.fields.get("LIST")?.as_array()?.get(1)?.as_str())
        .collect::<HashSet<_>>();
    let mut entries = BTreeMap::new();
    for (target_index, target) in project.targets.iter().enumerate() {
        for (id, list) in &target.lists {
            let monitor_possible = project.monitors.iter().any(|monitor| {
                monitor.opcode == "data_listcontents"
                    && monitor.id == *id
                    && monitor_list_target_index(project, monitor) == Some(target_index)
            });
            let info = entries.entry(id.clone()).or_insert_with(|| ListInfo {
                slot: 0,
                name: list.name.clone(),
                target: target.name.clone(),
                is_stage: target.is_stage,
                target_index,
                cache_numeric_reads: list.items.len() >= 1024
                    && !written_lists.contains(id.as_str()),
                monitor_possible,
            });
            info.monitor_possible |= monitor_possible;
            if target.is_stage {
                info.name = list.name.clone();
                info.target = target.name.clone();
                info.is_stage = true;
                info.target_index = target_index;
            }
        }
    }
    let mut output = HashMap::new();
    for (slot, (id, mut info)) in entries.into_iter().enumerate() {
        info.slot = slot;
        output.insert(id, info);
    }
    output
}

fn monitor_list_target_index(
    project: &ScratchProject,
    monitor: &crate::parser::ScratchMonitor,
) -> Option<usize> {
    let target_index = monitor.sprite_name.as_deref().map_or_else(
        || project.targets.iter().position(|target| target.is_stage),
        |sprite_name| {
            project
                .targets
                .iter()
                .position(|target| target.name == sprite_name)
        },
    )?;
    project.targets[target_index]
        .lists
        .contains_key(&monitor.id)
        .then_some(target_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_project;
    use serde_json::json;

    #[test]
    fn same_variable_id_on_two_sprites_keeps_target_values_and_monitors_separate() {
        let project = parse_project(
            &json!({
                "targets": [
                    {"name": "Stage", "isStage": true, "variables": {}, "blocks": {}},
                    {
                        "name": "SpriteA",
                        "isStage": false,
                        "variables": {"shared-id": ["Render Distance", 16]},
                        "blocks": {
                            "reporter_a": {
                                "opcode": "data_variable",
                                "fields": {"VARIABLE": ["Render Distance", "shared-id"]}
                            }
                        }
                    },
                    {
                        "name": "SpriteB",
                        "isStage": false,
                        "variables": {"shared-id": ["Render Distance", 5]},
                        "blocks": {
                            "reporter_b": {
                                "opcode": "data_variable",
                                "fields": {"VARIABLE": ["Render Distance", "shared-id"]}
                            }
                        }
                    }
                ],
                "monitors": [
                    {
                        "id": "shared-id",
                        "opcode": "data_variable",
                        "params": {"VARIABLE": "Render Distance"},
                        "spriteName": "SpriteA",
                        "visible": true
                    },
                    {
                        "id": "shared-id",
                        "opcode": "data_variable",
                        "params": {"VARIABLE": "Render Distance"},
                        "spriteName": "SpriteB",
                        "visible": false
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let variables = allocate_variables(&project);
        let sprite_a = variable_for_target(&project.targets[1], "shared-id", &variables).unwrap();
        let sprite_b = variable_for_target(&project.targets[2], "shared-id", &variables).unwrap();
        assert_ne!(sprite_a.slot, sprite_b.slot);
        assert!(sprite_a.monitor_visible);
        assert!(!sprite_b.monitor_visible);

        for (target_index, expected_slot) in [(1, sprite_a.slot), (2, sprite_b.slot)] {
            let block = project.targets[target_index]
                .blocks
                .values()
                .next()
                .unwrap();
            assert_eq!(
                crate::rust::statements::variable_field(block, "VARIABLE", &variables)
                    .unwrap()
                    .slot,
                expected_slot
            );
        }

        let generated = super::super::generate(&project);
        assert!(generated.contains("(16.0_f64).into(), (5.0_f64).into()"));
        assert!(generated.contains(
            "ScratchVariableMetadata::for_target(\"shared-id\", \"Render Distance\", \"SpriteA\", 1, false).with_monitor_visible(true)"
        ));
        assert!(generated.contains(
            "ScratchVariableMetadata::for_target(\"shared-id\", \"Render Distance\", \"SpriteB\", 2, false).with_monitor_visible(false)"
        ));
    }

    #[test]
    fn sensing_of_uses_saved_order_for_duplicate_variable_names_on_a_target() {
        let project = parse_project(
            r#"{
                "targets": [
                    {"name":"Stage","isStage":true,"variables":{},"blocks":{}},
                    {"name":"Engine","isStage":false,"variables":{
                        "z-first":["Render Distance",5],
                        "a-second":["Render Distance",0]
                    },"blocks":{}},
                    {"name":"Renderer","isStage":false,"variables":{},"blocks":{
                        "flag":{"opcode":"event_whenflagclicked","next":"say","topLevel":true,"inputs":{},"fields":{}},
                        "say":{"opcode":"looks_say","parent":"flag","inputs":{"MESSAGE":[3,"sense"]},"fields":{}},
                        "sense":{"opcode":"sensing_of","parent":"say","inputs":{"OBJECT":[1,"object"]},"fields":{"PROPERTY":["Render Distance",null]}},
                        "object":{"opcode":"sensing_of_object_menu","parent":"sense","inputs":{},"fields":{"OBJECT":["Engine",null]}}
                    }}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(
            project.targets[1].variable_order,
            vec!["z-first".to_owned(), "a-second".to_owned()]
        );

        let generated = super::super::generate(&project);
        assert!(
            generated.contains("state.sensing_of_target_slot(1, \"Render Distance\", Some(1))")
        );
    }

    #[test]
    fn minecraft_vcbt_render_distance_lookup_uses_the_first_engine_definition() {
        let input = concat!(env!("CARGO_MANIFEST_DIR"), "/programs/minecraft-vcbt.sb3");
        let json = crate::extractor::read_project_json(input).unwrap();
        let project = parse_project(&json).unwrap();
        let engine = project
            .targets
            .iter()
            .find(|target| target.name == "Engine")
            .unwrap();
        let ordered_ids = engine
            .variable_order
            .iter()
            .filter(|id| {
                engine
                    .variables
                    .get(*id)
                    .is_some_and(|variable| variable.name == "Render Distance")
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(ordered_ids[0], "w33SmpN4$*A-T6i%kR*^");
        assert_eq!(ordered_ids[1], "(x+o5V%u%|Ec7a#?o3vu");

        let variables = allocate_variables(&project);
        let first = variable_for_target(engine, &ordered_ids[0], &variables).unwrap();
        let second = variable_for_target(engine, &ordered_ids[1], &variables).unwrap();
        assert!(first.target_variable_order < second.target_variable_order);

        let generated = super::super::generate(&project);
        assert!(generated.contains(&format!(
            "state.sensing_of_target_slot(2, \"Render Distance\", Some({}))",
            first.slot
        )));
    }
}

// convert a Scratch variable name into a valid Rust identifier.
pub(in crate::rust) fn rust_identifier(name: &str) -> String {
    let mut output = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_alphanumeric() || character == '_' {
            if index == 0 && character.is_ascii_digit() {
                output.push('_');
            }
            output.push(character);
        } else if !output.ends_with('_') {
            output.push('_');
        }
    }
    let output = output.trim_matches('_');
    let output = if output.is_empty() {
        "variable"
    } else {
        output
    };
    let output = if output.as_bytes()[0].is_ascii_digit() {
        format!("_{output}")
    } else {
        output.to_owned()
    };
    if is_keyword(&output) {
        format!("_{output}")
    } else {
        output
    }
}

// identify Rust keywords that need a prefix before they can be used as variables.
pub(super) fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
    )
}
