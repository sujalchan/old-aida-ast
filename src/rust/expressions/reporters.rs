use super::*;

pub(super) fn list_field(
    block: &ScratchBlock,
    field: &str,
    lists: &HashMap<String, ListInfo>,
) -> Option<ListInfo> {
    let data = block.fields.get(field)?.as_array()?;
    lists.get(data.get(1)?.as_str()?).cloned()
}

// convert a scratch input into a rust value expression, following reporter links recursively.
pub(in crate::rust) fn expression(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
    active: &mut HashSet<String>,
) -> String {
    match input.and_then(parse_input) {
        Some(InputValue::Number(value)) => number_literal(value),
        Some(InputValue::String(value)) => text_literal(value),
        Some(InputValue::VariableReference(reference)) => {
            super::super::project::variable_for_target(target, reference.id, variables)
                .map(VariableInfo::read_expression)
                .unwrap_or_else(|| unsupported_expression("unknown variable"))
        }
        Some(InputValue::BlockReference(id)) => {
            if !active.insert(id.to_owned()) {
                return unsupported_expression(&format!("expression cycle: {id}"));
            }
            let result = target
                .blocks
                .get(id)
                .map(|block| block_expression(target, block, variables, lists, arguments, active))
                .unwrap_or_else(|| unsupported_expression(&format!("missing block: {id}")));
            active.remove(id);
            result
        }
        Some(InputValue::Empty) => unsupported_expression("empty input"),
        Some(InputValue::ListReference(reference)) => lists
            .get(reference.id)
            .map(|list| format!("{}.contents()", list.read_expression()))
            .unwrap_or_else(|| unsupported_expression("unknown list")),
        None => unsupported_expression("missing or invalid input"),
    }
}

// choose the rust lowering for a reporter block based on its scratch opcode.
pub(super) fn block_expression(
    target: &ScratchTarget,
    block: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
    active: &mut HashSet<String>,
) -> String {
    let input = |name: &str, active: &mut HashSet<String>| {
        expression(
            target,
            block.inputs.get(name),
            variables,
            lists,
            arguments,
            active,
        )
    };
    let binary = |function: &str, left: &str, right: &str, active: &mut HashSet<String>| {
        format!(
            "{function}({}, {})",
            input(left, active),
            input(right, active)
        )
    };
    // A list selected for cached numeric reads still owns its original dynamic
    // values; the runtime invalidates the cache when the list changes.
    let numeric_input = |name: &str, active: &mut HashSet<String>| {
        let candidate = block
            .inputs
            .get(name)
            .and_then(parse_input)
            .and_then(|input| match input {
                InputValue::BlockReference(id) => target.blocks.get(id),
                _ => None,
            })
            .filter(|reporter| reporter.opcode == "data_itemoflist");
        if let Some(reporter) = candidate
            && let Some(list) =
                list_field(reporter, "LIST", lists).filter(|list| list.cache_numeric_reads)
        {
            let index = expression(
                target,
                reporter.inputs.get("INDEX"),
                variables,
                lists,
                arguments,
                active,
            );
            return format!("{}.get_cached_number({index})", list.read_expression());
        }
        input(name, active)
    };
    let numeric_binary = |function: &str, active: &mut HashSet<String>| {
        format!(
            "{function}({}, {})",
            numeric_input("NUM1", active),
            numeric_input("NUM2", active)
        )
    };
    if matches!(
        block.opcode.as_str(),
        "operator_add" | "operator_subtract" | "operator_multiply" | "operator_divide"
    ) {
        let left = constant_numeric_input(target, block.inputs.get("NUM1"), &mut HashSet::new());
        let right = constant_numeric_input(target, block.inputs.get("NUM2"), &mut HashSet::new());
        if let (Some(left), Some(right)) = (left, right) {
            let folded = match block.opcode.as_str() {
                "operator_add" => left + right,
                "operator_subtract" => left - right,
                "operator_multiply" => left * right,
                "operator_divide" => left / right,
                _ => unreachable!(),
            };
            if folded.is_finite() {
                return number_literal(&Value::from(folded));
            }
        }
    }
    // Canonical small integer text has exactly the same Scratch comparison
    // text as its numeric value, including when the other operand falls back
    // to a string comparison. Emit a number to avoid parsing it on every test.
    let comparison = |function: &str, active: &mut HashSet<String>| {
        let operand = |name: &str, active: &mut HashSet<String>| {
            let value = block.inputs.get(name);
            let other = if name == "OPERAND1" {
                "OPERAND2"
            } else {
                "OPERAND1"
            };
            let both_literals = matches!(
                block.inputs.get(other).and_then(parse_input),
                Some(InputValue::String(_) | InputValue::Number(_))
            );
            if let Some(InputValue::String(value)) =
                value.and_then(parse_input).filter(|_| !both_literals)
                && let Some(text) = value.as_str()
                && let Ok(number) = text.parse::<i32>()
                && number.unsigned_abs() <= 1_000_000
                && number.to_string() == text
            {
                return format!("{number}.0_f64");
            }
            input(name, active)
        };
        let left = operand("OPERAND1", active);
        let right = operand("OPERAND2", active);
        let both_literals = ["OPERAND1", "OPERAND2"].iter().all(|name| {
            matches!(
                block.inputs.get(*name).and_then(parse_input),
                Some(InputValue::String(_) | InputValue::Number(_))
            )
        });
        let left_numeric =
            comparison_input_is_numeric(target, block.inputs.get("OPERAND1"), variables);
        let right_numeric =
            comparison_input_is_numeric(target, block.inputs.get("OPERAND2"), variables);
        let op = match function {
            "scratch_equals" => "==",
            "scratch_greater_than" => ">",
            "scratch_less_than" => "<",
            _ => unreachable!(),
        };
        if !both_literals && left_numeric && right_numeric {
            format!(
                "{{ let left = {left}; let right = {right}; if left.is_nan() || right.is_nan() {{ {function}(left, right) }} else {{ left {op} right }} }}"
            )
        } else if !left_numeric
            && right_numeric
            && comparison_input_is_value_ref(target, block.inputs.get("OPERAND1"), variables)
        {
            format!(
                "{{ let left = {left}; let right = {right}; match left {{ ScratchValue::Number(number) if !number.is_nan() && !right.is_nan() => *number {op} right, _ => {function}(left, right) }} }}"
            )
        } else if left_numeric
            && !right_numeric
            && comparison_input_is_value_ref(target, block.inputs.get("OPERAND2"), variables)
        {
            format!(
                "{{ let left = {left}; let right = {right}; match right {{ ScratchValue::Number(number) if !left.is_nan() && !number.is_nan() => left {op} *number, _ => {function}(left, right) }} }}"
            )
        } else if !left_numeric
            && !right_numeric
            && comparison_input_is_value_ref(target, block.inputs.get("OPERAND1"), variables)
            && comparison_input_is_value_ref(target, block.inputs.get("OPERAND2"), variables)
        {
            format!(
                "{{ let left = {left}; let right = {right}; match (left, right) {{ (ScratchValue::Number(a), ScratchValue::Number(b)) if !a.is_nan() && !b.is_nan() => *a {op} *b, _ => {function}(left, right) }} }}"
            )
        } else {
            format!("{function}({left}, {right})")
        }
    };
    match block.opcode.as_str() {
        "operator_add" => numeric_binary("scratch_add", active),
        "operator_subtract" => numeric_binary("scratch_subtract", active),
        "operator_multiply" => numeric_binary("scratch_multiply", active),
        "operator_divide" => numeric_binary("scratch_divide", active),
        "operator_mod" => binary("scratch_mod", "NUM1", "NUM2", active),
        "operator_equals" if identical_literal_inputs(block) => "true".to_owned(),
        "operator_equals" => numeric_list_equality(target, block, variables, lists)
            .unwrap_or_else(|| comparison("scratch_equals", active)),
        "operator_gt" => comparison("scratch_greater_than", active),
        "operator_lt" => comparison("scratch_less_than", active),
        "operator_and" => format!(
            "scratch_and({}, {})",
            boolean_input(
                target, block, "OPERAND1", variables, lists, arguments, active
            ),
            boolean_input(
                target, block, "OPERAND2", variables, lists, arguments, active
            )
        ),
        "operator_or" => format!(
            "scratch_or({}, {})",
            boolean_input(
                target, block, "OPERAND1", variables, lists, arguments, active
            ),
            boolean_input(
                target, block, "OPERAND2", variables, lists, arguments, active
            )
        ),
        "operator_join" => binary("scratch_join", "STRING1", "STRING2", active),
        "operator_letter_of" => binary("scratch_letter", "LETTER", "STRING", active),
        "operator_length" => format!("scratch_length({})", input("STRING", active)),
        "operator_contains" => binary("scratch_contains", "STRING1", "STRING2", active),
        "operator_round" => format!("scratch_round({})", input("NUM", active)),
        "operator_not" => format!(
            "scratch_not({})",
            boolean_input(
                target, block, "OPERAND", variables, lists, arguments, active
            )
        ),
        "operator_random" => random_expression(target, block, variables, lists, arguments, active),
        "operator_mathop" => math_expression(target, block, variables, lists, arguments, active),
        "data_variable" => variable_field(block, "VARIABLE", variables)
            .map(|variable| variable.read_expression())
            .unwrap_or_else(|| unsupported_expression("missing variable reference")),
        "sensing_of" => sensing_of_expression(target, block, variables, lists, arguments, active),
        "sensing_of_object_menu" => field_value(block, "OBJECT")
            .and_then(Value::as_str)
            .map(quote)
            .unwrap_or_else(|| "ScratchValue::Undefined".to_owned()),
        "sound_volume" => "state.sound_volume()".to_owned(),
        "sound_sounds_menu" => field_value(block, "SOUND_MENU")
            .map(text_literal)
            .unwrap_or_else(|| unsupported_expression("missing sound menu value")),
        "data_listcontents" => list_field(block, "LIST", lists)
            .map(|list| format!("{}.contents()", list.read_expression()))
            .unwrap_or_else(|| unsupported_expression("missing list reference")),
        "looks_backdrops" => field_value(block, "BACKDROP")
            .map(text_literal)
            .unwrap_or_else(|| unsupported_expression("missing backdrop menu value")),
        // read each motion reporter from the instance selected by the scheduler.
        "motion_xposition" => "state.x_position()".to_owned(),
        "motion_yposition" => "state.y_position()".to_owned(),
        // report the direction stored for the currently scheduled sprite or clone.
        "motion_direction" => "state.direction()".to_owned(),
        "looks_costume" => field_value(block, "COSTUME")
            .map(text_literal)
            .unwrap_or_else(|| unsupported_expression("missing costume menu value")),
        "pen_menu_colorParam" => field_value(block, "colorParam")
            .map(text_literal)
            .unwrap_or_else(|| unsupported_expression("missing pen color parameter")),
        "looks_costumenumbername" => {
            let number = field_value(block, "NUMBER_NAME")
                .and_then(Value::as_str)
                .is_some_and(|value| value == "number");
            format!("state.costume_number_name({number})")
        }
        "looks_size" => "state.size_reporter()".to_owned(),
        "looks_backdropnumbername" => {
            let number = field_value(block, "NUMBER_NAME")
                .and_then(Value::as_str)
                .is_some_and(|value| value == "number");
            format!("state.backdrop_number_name({number})")
        }
        "data_itemoflist" => list_field(block, "LIST", lists)
            .map(|list| {
                let method = if comparison_input_is_numeric(
                    target,
                    block.inputs.get("INDEX"),
                    variables,
                ) {
                    "get_numeric"
                } else {
                    "get"
                };
                format!(
                    "{}.{method}({})",
                    list.read_expression(),
                    input("INDEX", active)
                )
            })
            .unwrap_or_else(|| unsupported_expression("missing list reference")),
        // forward dropdown selectors unchanged; list operations resolve them when used.
        "data_listindexall" | "data_listindexrandom" => field_value(block, "INDEX")
            .and_then(Value::as_str)
            .map(quote)
            .unwrap_or_else(|| quote("1")),
        "data_lengthoflist" => list_field(block, "LIST", lists)
            .map(|list| format!("{}.length() as f64", list.read_expression()))
            .unwrap_or_else(|| unsupported_expression("missing list reference")),
        "event_broadcast_menu" => block
            .fields
            .get("BROADCAST_OPTION")
            .and_then(Value::as_array)
            .and_then(|field| {
                let name = field.first()?.as_str()?;
                Some(
                    field
                        .get(1)
                        .and_then(Value::as_str)
                        .and_then(|id| target.broadcasts.get(id))
                        .map(String::as_str)
                        .unwrap_or(name),
                )
            })
            .map(quote)
            .unwrap_or_else(|| unsupported_expression("missing broadcast message")),
        "data_itemnumoflist" => list_field(block, "LIST", lists)
            .map(|list| {
                format!(
                    "{}.item_number({}) as f64",
                    list.read_expression(),
                    input("ITEM", active)
                )
            })
            .unwrap_or_else(|| unsupported_expression("missing list reference")),
        // evaluate the current connected reporter, falling back to legacy field data only when absent.
        "sensing_keypressed" => block
            .inputs
            .get("KEY_OPTION")
            .map(|_| format!("state.key_pressed({})", input("KEY_OPTION", active)))
            .or_else(|| {
                field_value(block, "KEY_OPTION")
                    .and_then(Value::as_str)
                    .map(|key| format!("state.key_pressed({})", quote(key)))
            })
            .unwrap_or_else(|| "state.key_pressed(\"\")".to_owned()),
        // preserve the selected serialized dropdown value for reporter and hat inputs.
        "sensing_keyoptions" => field_value(block, "KEY_OPTION")
            .and_then(Value::as_str)
            .map(quote)
            .unwrap_or_else(|| quote("space")),
        "sensing_mousedown" => "state.mouse_down()".to_owned(),
        "sensing_mousex" => "state.mouse_x()".to_owned(),
        "sensing_mousey" => "state.mouse_y()".to_owned(),
        "sensing_timer" => "state.project_timer_seconds(clock.now_millis())".to_owned(),
        "sensing_current" => current_expression(block),
        "sensing_dayssince2000" => "state.days_since_2000()".to_owned(),
        "sensing_online" => "false".to_owned(),
        "sensing_username" => "state.username()".to_owned(),
        "sensing_loudness" => "state.loudness()".to_owned(),
        // legacy vm reporters have no primitive result, so preserve scratch's undefined value.
        "motion_xscroll" | "motion_yscroll" | "sensing_userid" => {
            "ScratchValue::Undefined".to_owned()
        }
        "data_listcontainsitem" => list_field(block, "LIST", lists)
            .map(|list| {
                format!(
                    "{}.contains({})",
                    list.read_expression(),
                    input("ITEM", active)
                )
            })
            .unwrap_or_else(|| unsupported_expression("missing list reference")),
        "argument_reporter_string_number" | "argument_reporter_boolean" => {
            field_value(block, "VALUE")
                .and_then(Value::as_str)
                .and_then(|name| arguments.get(name))
                .map(|name| format!("&{name}"))
                .unwrap_or_else(|| unsupported_expression("missing procedure argument"))
        }
        "math_number"
        | "math_integer"
        | "math_whole_number"
        | "math_positive_number"
        | "math_angle" => field_value(block, "NUM")
            .map(number_literal)
            .unwrap_or_else(|| unsupported_expression("missing numeric field")),
        "text" => field_value(block, "TEXT")
            .map(text_literal)
            .unwrap_or_else(|| unsupported_expression("missing text field")),
        opcode => unsupported_expression(&format!("unsupported reporter: {opcode}")),
    }
}

fn sensing_of_expression(
    target: &ScratchTarget,
    block: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
    active: &mut HashSet<String>,
) -> String {
    let Some(property) = field_value(block, "PROPERTY").and_then(Value::as_str) else {
        return "ScratchValue::Number(0.0_f64)".to_owned();
    };
    let Some(object_input) = block.inputs.get("OBJECT") else {
        return "ScratchValue::Number(0.0_f64)".to_owned();
    };

    match parse_input(object_input) {
        Some(InputValue::Empty) | None => {
            return "ScratchValue::Number(0.0_f64)".to_owned();
        }
        Some(InputValue::BlockReference(id)) => {
            let Some(input_block) = target.blocks.get(id) else {
                return "ScratchValue::Number(0.0_f64)".to_owned();
            };
            if active.contains(id) {
                return "ScratchValue::Number(0.0_f64)".to_owned();
            }
            if input_block.opcode == "sensing_of_object_menu"
                && field_value(input_block, "OBJECT")
                    .and_then(Value::as_str)
                    .is_none()
            {
                return "ScratchValue::Number(0.0_f64)".to_owned();
            }
        }
        Some(InputValue::VariableReference(reference))
            if super::super::project::variable_for_target(target, reference.id, variables)
                .is_none() =>
        {
            return "ScratchValue::Number(0.0_f64)".to_owned();
        }
        Some(InputValue::ListReference(reference)) if !lists.contains_key(reference.id) => {
            return "ScratchValue::Number(0.0_f64)".to_owned();
        }
        _ => {}
    }

    let static_name = match parse_input(object_input) {
        Some(InputValue::String(value)) => value.as_str(),
        Some(InputValue::BlockReference(id)) => target
            .blocks
            .get(id)
            .filter(|menu| menu.opcode == "sensing_of_object_menu")
            .and_then(|menu| field_value(menu, "OBJECT"))
            .and_then(Value::as_str),
        _ => None,
    };

    if let Some(name) = static_name
        && !target.target_identities.is_empty()
    {
        let identity = if name == "_stage_" {
            target
                .target_identities
                .iter()
                .find(|(_, _, is_stage)| *is_stage)
        } else {
            target
                .target_identities
                .iter()
                .find(|(target_name, _, is_stage)| !is_stage && target_name == name)
        };
        let Some((_, target_index, is_stage)) = identity else {
            return "ScratchValue::Number(0.0_f64)".to_owned();
        };
        let variable_slot = variables
            .values()
            .filter(|variable| {
                variable.target_index == *target_index
                    && variable.is_stage == *is_stage
                    && variable.name == property
            })
            .min_by_key(|variable| variable.target_variable_order)
            .map(|variable| variable.slot);
        let is_builtin = if *is_stage {
            matches!(
                property,
                "backdrop #" | "background #" | "backdrop name" | "volume"
            )
        } else {
            matches!(
                property,
                "x position"
                    | "y position"
                    | "direction"
                    | "costume #"
                    | "costume name"
                    | "size"
                    | "volume"
            )
        };
        if !is_builtin && variable_slot.is_none() {
            return "ScratchValue::Number(0.0_f64)".to_owned();
        }
        return format!(
            "state.sensing_of_target_slot({target_index}, {property:?}, {})",
            variable_slot.map_or_else(|| "None".to_owned(), |slot| format!("Some({slot})"))
        );
    }

    let object = expression(
        target,
        Some(object_input),
        variables,
        lists,
        arguments,
        active,
    );
    format!("state.sensing_of_by_name({object}, {property:?})")
}

pub(super) fn comparison_input_is_value_ref(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
) -> bool {
    match input.and_then(parse_input) {
        Some(InputValue::VariableReference(reference)) => {
            super::super::project::variable_for_target(target, reference.id, variables).is_some_and(
                |v| {
                    v.index_read_override
                        .as_deref()
                        .is_none_or(|s| s.starts_with("&scratch_dynamic_"))
                },
            )
        }
        Some(InputValue::BlockReference(id)) => target.blocks.get(id).is_some_and(|b| {
            b.opcode == "data_itemoflist"
                || (b.opcode == "data_variable"
                    && variable_field(b, "VARIABLE", variables).is_some_and(|v| {
                        v.index_read_override
                            .as_deref()
                            .is_none_or(|s| s.starts_with("&scratch_dynamic_"))
                    }))
        }),
        _ => false,
    }
}

// Numeric comparison operands can use native operators except for NaN,
// where Scratch deliberately falls back to text ordering.
pub(super) fn comparison_input_is_numeric(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
) -> bool {
    let typed = |variable: &VariableInfo| {
        variable
            .index_read_override
            .as_deref()
            .is_some_and(|name| name.starts_with("scratch_typed_"))
    };
    match input.and_then(parse_input) {
        Some(InputValue::Number(_)) => true,
        Some(InputValue::String(value)) => value
            .as_str()
            .and_then(|text| text.parse::<i32>().ok().map(|n| (text, n)))
            .is_some_and(|(text, n)| n.unsigned_abs() <= 1_000_000 && n.to_string() == text),
        Some(InputValue::VariableReference(reference)) => {
            super::super::project::variable_for_target(target, reference.id, variables)
                .is_some_and(typed)
        }
        Some(InputValue::BlockReference(id)) => target.blocks.get(id).is_some_and(|block| {
            (block.opcode == "data_variable"
                && variable_field(block, "VARIABLE", variables).is_some_and(|v| typed(&v)))
                || matches!(
                    block.opcode.as_str(),
                    "operator_add"
                        | "operator_subtract"
                        | "operator_multiply"
                        | "operator_divide"
                        | "operator_mod"
                        | "operator_round"
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

// specialize direct numeric list comparisons while retaining dynamic fallbacks.
pub(super) fn numeric_list_equality(
    target: &ScratchTarget,
    block: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
) -> Option<String> {
    for (list_input, other_input) in [("OPERAND1", "OPERAND2"), ("OPERAND2", "OPERAND1")] {
        let InputValue::BlockReference(id) = parse_input(block.inputs.get(list_input)?)? else {
            continue;
        };
        let reporter = target.blocks.get(id)?;
        if reporter.opcode != "data_itemoflist" {
            continue;
        }
        let list = list_field(reporter, "LIST", lists)?;
        let index = direct_variable_input(target, reporter.inputs.get("INDEX"), variables)?;
        let other = direct_variable_input(target, block.inputs.get(other_input), variables)?;
        if !index.numeric || !other.numeric {
            continue;
        }
        let index_expression = index.read_expression();
        let other_expression = other.read_expression();
        let list_expression = list.read_expression();
        return Some(format!(
            "{{ let scratch_numeric_list_match = match ({index_expression}, {other_expression}) {{ (ScratchValue::Number(scratch_index), ScratchValue::Number(scratch_value)) => {list_expression}.equals_number(*scratch_index, *scratch_value), _ => None }}; scratch_numeric_list_match.unwrap_or_else(|| scratch_equals({list_expression}.get({index_expression}), {other_expression})) }}"
        ));
    }
    None
}

pub(super) fn direct_variable_input(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
) -> Option<VariableInfo> {
    match input.and_then(parse_input)? {
        InputValue::VariableReference(reference) => {
            super::super::project::variable_for_target(target, reference.id, variables).cloned()
        }
        InputValue::BlockReference(id) => {
            let reporter = target.blocks.get(id)?;
            if reporter.opcode == "data_variable" {
                variable_field(reporter, "VARIABLE", variables)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(super) fn current_expression(block: &ScratchBlock) -> String {
    block
        .fields
        .get("CURRENTMENU")
        .and_then(Value::as_array)
        .and_then(|field| field.first())
        .and_then(Value::as_str)
        .and_then(current_method)
        .map_or_else(|| "0.0".to_owned(), |method| format!("state.{method}()"))
}

pub(super) fn current_method(menu: &str) -> Option<&'static str> {
    match menu.to_ascii_uppercase().as_str() {
        "YEAR" => Some("current_year"),
        "MONTH" => Some("current_month"),
        "DATE" => Some("current_date"),
        "DAYOFWEEK" => Some("current_day_of_week"),
        "HOUR" => Some("current_hour"),
        "MINUTE" => Some("current_minute"),
        "SECOND" => Some("current_second"),
        _ => None,
    }
}

pub(super) fn identical_literal_inputs(block: &ScratchBlock) -> bool {
    let left = block.inputs.get("OPERAND1").and_then(parse_input);
    let right = block.inputs.get("OPERAND2").and_then(parse_input);
    match (left, right) {
        (Some(InputValue::Number(left)), Some(InputValue::Number(right))) => {
            number_literal(left) == number_literal(right)
        }
        (Some(InputValue::String(left)), Some(InputValue::String(right))) => left == right,
        _ => false,
    }
}

// evaluate constant arithmetic using scratch's numeric input conversion.
pub(in crate::rust) fn constant_numeric_input(
    target: &ScratchTarget,
    input: Option<&Value>,
    active: &mut HashSet<String>,
) -> Option<f64> {
    let normalize = |value: f64| if value.is_nan() { 0.0 } else { value };
    match input.and_then(parse_input)? {
        InputValue::Number(value) => Some(normalize(
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
                .unwrap_or(0.0),
        )),
        InputValue::String(value) => Some(normalize(
            value
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .and_then(|text| text.parse().ok())
                .unwrap_or(0.0),
        )),
        InputValue::BlockReference(id) => {
            if !active.insert(id.to_owned()) {
                return None;
            }
            let block = target.blocks.get(id)?;
            let result = if block.opcode == "operator_equals" && identical_literal_inputs(block) {
                Some(1.0)
            } else {
                let names = match block.opcode.as_str() {
                    "operator_add" | "operator_subtract" => ("NUM1", "NUM2"),
                    "operator_multiply" | "operator_divide" => ("NUM1", "NUM2"),
                    _ => {
                        active.remove(id);
                        return None;
                    }
                };
                let left = normalize(constant_numeric_input(
                    target,
                    block.inputs.get(names.0),
                    active,
                )?);
                let right = normalize(constant_numeric_input(
                    target,
                    block.inputs.get(names.1),
                    active,
                )?);
                Some(match block.opcode.as_str() {
                    "operator_add" => left + right,
                    "operator_subtract" => left - right,
                    "operator_multiply" => left * right,
                    "operator_divide" => left / right,
                    _ => unreachable!(),
                })
            };
            active.remove(id);
            result
        }
        _ => None,
    }
}

// collect variables that a native numeric loop can safely keep in f64 locals.
pub(super) fn math_expression(
    target: &ScratchTarget,
    block: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
    active: &mut HashSet<String>,
) -> String {
    let value = expression(
        target,
        block.inputs.get("NUM"),
        variables,
        lists,
        arguments,
        active,
    );
    let numeric = numeric_literal(target, block.inputs.get("NUM"))
        .map(|literal| {
            if literal.starts_with('-') {
                format!("({literal})")
            } else {
                literal
            }
        })
        .unwrap_or_else(|| format!("scratch_to_number({value})"));
    let Some(operator) = field_value(block, "OPERATOR").and_then(Value::as_str) else {
        return unsupported_expression("missing math operator");
    };
    match operator {
        "sin" => return format!("scratch_sin({value})"),
        "cos" => return format!("scratch_cos({value})"),
        "tan" => return format!("scratch_tan({value})"),
        "asin" => return format!("scratch_asin_degrees({value})"),
        "acos" => return format!("scratch_acos_degrees({value})"),
        "atan" => return format!("scratch_atan_degrees({value})"),
        _ => {}
    }
    let operation = match operator {
        "abs" => "abs()",
        "floor" => "floor()",
        "ceiling" => "ceil()",
        "sqrt" => "sqrt()",
        "ln" => "ln()",
        "log" => "log10()",
        "e ^" => "exp()",
        "10 ^" => return format!("10.0_f64.powf({numeric})"),
        _ => {
            return unsupported_expression(&format!("unsupported math operator: {operator}"));
        }
    };
    format!("{numeric}.{operation}")
}

// return a direct numeric literal when the input is a number primitive or number reporter.
pub(super) fn numeric_literal(target: &ScratchTarget, input: Option<&Value>) -> Option<String> {
    match input.and_then(parse_input)? {
        InputValue::Number(value) => Some(number_literal(value)),
        InputValue::BlockReference(id) => {
            let block = target.blocks.get(id)?;
            matches!(
                block.opcode.as_str(),
                "math_number"
                    | "math_integer"
                    | "math_whole_number"
                    | "math_positive_number"
                    | "math_angle"
            )
            .then(|| field_value(block, "NUM").map(number_literal))?
        }
        _ => None,
    }
}

// preserve numeric spelling for random inputs because scratch distinguishes 10 from 10.0.
// lower fixed random bounds once while preserving scratch's integer-range decision.
pub(super) fn random_expression(
    target: &ScratchTarget,
    block: &ScratchBlock,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
    active: &mut HashSet<String>,
) -> String {
    if let Some((from, to, integer_bounds)) = constant_random_bounds(target, block) {
        return format!(
            "scratch_random_constant({from:?}_f64, {to:?}_f64, {})",
            integer_bounds
        );
    }
    format!(
        "scratch_random({}, {})",
        random_argument(
            target,
            block.inputs.get("FROM"),
            variables,
            lists,
            arguments,
            active,
        ),
        random_argument(
            target,
            block.inputs.get("TO"),
            variables,
            lists,
            arguments,
            active,
        )
    )
}

pub(in crate::rust) fn constant_random_bounds(
    target: &ScratchTarget,
    block: &ScratchBlock,
) -> Option<(f64, f64, bool)> {
    if block.opcode != "operator_random" {
        return None;
    }
    let (from, from_integer) = constant_random_bound(target, block.inputs.get("FROM"))?;
    let (to, to_integer) = constant_random_bound(target, block.inputs.get("TO"))?;
    Some((from, to, from_integer && to_integer))
}

pub(super) fn constant_random_bound(
    target: &ScratchTarget,
    input: Option<&Value>,
) -> Option<(f64, bool)> {
    let spelling = match input.and_then(parse_input)? {
        InputValue::Number(value) => random_literal_spelling(value)?,
        InputValue::BlockReference(id) => {
            let block = target.blocks.get(id)?;
            let field = match block.opcode.as_str() {
                "math_number"
                | "math_integer"
                | "math_whole_number"
                | "math_positive_number"
                | "math_angle" => "NUM",
                "text" => "TEXT",
                _ => return None,
            };
            random_literal_spelling(field_value(block, field)?)?
        }
        _ => return None,
    };
    let text = spelling.trim();
    let number = if text.is_empty() {
        0.0
    } else {
        text.parse::<f64>().unwrap_or(0.0)
    };
    let number = if number.is_nan() { 0.0 } else { number };
    number
        .is_finite()
        .then_some((number, !spelling.contains('.')))
}

pub(super) fn random_literal_spelling(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_number().map(|number| number.to_string()))
}

pub(super) fn random_argument(
    target: &ScratchTarget,
    input: Option<&Value>,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
    active: &mut HashSet<String>,
) -> String {
    match input.and_then(parse_input) {
        Some(InputValue::Number(value)) => value
            .as_str()
            .map(quote)
            .or_else(|| value.as_number().map(|number| quote(&number.to_string())))
            .unwrap_or_else(|| expression(target, input, variables, lists, arguments, active)),
        Some(InputValue::BlockReference(id)) => {
            let Some(block) = target.blocks.get(id) else {
                return expression(target, input, variables, lists, arguments, active);
            };
            if matches!(
                block.opcode.as_str(),
                "math_number"
                    | "math_integer"
                    | "math_whole_number"
                    | "math_positive_number"
                    | "math_angle"
            ) && let Some(value) = field_value(block, "NUM")
            {
                if let Some(text) = value.as_str() {
                    return quote(text);
                }
                if let Some(number) = value.as_number() {
                    return quote(&number.to_string());
                }
            }
            expression(target, input, variables, lists, arguments, active)
        }
        _ => expression(target, input, variables, lists, arguments, active),
    }
}

// emit a typed placeholder that reports why this expression could not be lowered.
pub(super) fn unsupported_expression(reason: &str) -> String {
    format!("ScratchValue::Number(todo!({}))", quote(reason))
}

pub(in crate::rust) fn boolean_input(
    target: &ScratchTarget,
    block: &ScratchBlock,
    name: &str,
    variables: &HashMap<String, VariableInfo>,
    lists: &HashMap<String, ListInfo>,
    arguments: &HashMap<String, String>,
    active: &mut HashSet<String>,
) -> String {
    match block.inputs.get(name).and_then(parse_input) {
        Some(InputValue::Empty) | None => "false".to_owned(),
        Some(InputValue::BlockReference(id))
            if target.blocks.get(id).is_some_and(|reporter| {
                matches!(
                    reporter.opcode.as_str(),
                    "operator_equals"
                        | "operator_gt"
                        | "operator_lt"
                        | "operator_and"
                        | "operator_or"
                        | "operator_not"
                        | "operator_contains"
                        | "data_listcontainsitem"
                )
            }) =>
        {
            expression(
                target,
                block.inputs.get(name),
                variables,
                lists,
                arguments,
                active,
            )
        }
        Some(_) => format!(
            "scratch_bool({})",
            expression(
                target,
                block.inputs.get(name),
                variables,
                lists,
                arguments,
                active
            )
        ),
    }
}
