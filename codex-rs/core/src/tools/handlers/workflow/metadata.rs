use crate::function_tool::FunctionCallError;
use serde::Serialize;

#[derive(Clone, Debug)]
pub(super) struct WorkflowMetadata {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) when_to_use: Option<String>,
    pub(super) input_schema: Option<String>,
    pub(super) phases: Vec<WorkflowPhaseMetadata>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct WorkflowPhaseMetadata {
    pub(super) title: String,
    pub(super) model: Option<String>,
}

#[derive(Debug)]
pub(super) struct ValidatedWorkflowScript {
    pub(super) metadata: WorkflowMetadata,
    pub(super) body: String,
}

pub(super) fn validate_workflow_script(
    code: &str,
) -> Result<ValidatedWorkflowScript, FunctionCallError> {
    let (metadata, body) = parse_workflow_metadata(code)?;
    reject_forbidden_workflow_patterns(body.as_str())?;
    Ok(ValidatedWorkflowScript { metadata, body })
}

fn parse_workflow_metadata(code: &str) -> Result<(WorkflowMetadata, String), FunctionCallError> {
    let trimmed_start = code.len() - code.trim_start().len();
    let trimmed = &code[trimmed_start..];
    let prefix = "export const meta";
    if !trimmed.starts_with(prefix) {
        return Err(FunctionCallError::RespondToModel(
            "workflow script must begin with `export const meta = { name, description, ... }`"
                .to_string(),
        ));
    }

    let mut cursor = trimmed_start + prefix.len();
    cursor = skip_js_whitespace(code, cursor);
    if !code[cursor..].starts_with('=') {
        return Err(FunctionCallError::RespondToModel(
            "workflow meta declaration must assign a pure object literal".to_string(),
        ));
    }
    cursor += '='.len_utf8();
    cursor = skip_js_whitespace(code, cursor);
    if !code[cursor..].starts_with('{') {
        return Err(FunctionCallError::RespondToModel(
            "workflow meta must be a pure object literal".to_string(),
        ));
    }

    let meta_end = find_matching_brace(code, cursor)?;
    let meta_literal = &code[cursor..=meta_end];
    validate_meta_literal(meta_literal)?;
    let name = extract_meta_string_field(meta_literal, "name").ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "workflow meta must include string field `name`".to_string(),
        )
    })?;
    let description = extract_meta_string_field(meta_literal, "description").ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "workflow meta must include string field `description`".to_string(),
        )
    })?;
    let when_to_use = extract_meta_string_field(meta_literal, "whenToUse");
    let input_schema = extract_meta_literal_field(meta_literal, "inputSchema")?;
    let phases = extract_meta_phases(meta_literal)?;

    let mut body_start = meta_end + '}'.len_utf8();
    body_start = skip_js_whitespace(code, body_start);
    if code[body_start..].starts_with(';') {
        body_start += ';'.len_utf8();
    }
    let body = code[body_start..].to_string();
    Ok((
        WorkflowMetadata {
            name,
            description,
            when_to_use,
            input_schema,
            phases,
        },
        body,
    ))
}

fn validate_meta_literal(meta_literal: &str) -> Result<(), FunctionCallError> {
    for (pattern, label) in [
        ("`", "template interpolation"),
        ("...", "spread syntax"),
        ("(", "function calls"),
        ("=>", "function literals"),
        ("function", "function literals"),
    ] {
        if contains_js_pattern_outside_strings(meta_literal, pattern) {
            return Err(FunctionCallError::RespondToModel(format!(
                "workflow meta must be a pure object literal; {label} is not allowed"
            )));
        }
    }
    Ok(())
}

fn reject_forbidden_workflow_patterns(body: &str) -> Result<(), FunctionCallError> {
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("import ") || trimmed.starts_with("export ") {
            return Err(FunctionCallError::RespondToModel(
                "workflow body must be plain JavaScript without import/export declarations"
                    .to_string(),
            ));
        }
    }

    for (constructor, label) in [("Function", "Function constructor"), ("Date", "new Date")] {
        if contains_js_new_expression_outside_strings(body, constructor) {
            return Err(FunctionCallError::RespondToModel(format!(
                "workflow scripts must be deterministic and self-contained; {label} is not allowed"
            )));
        }
    }

    for (callee, label) in [
        ("import", "dynamic import"),
        ("require", "CommonJS require"),
        ("eval", "eval"),
        ("Function", "Function constructor"),
        ("Date", "Date"),
    ] {
        if contains_js_call_outside_strings(body, callee) {
            return Err(FunctionCallError::RespondToModel(format!(
                "workflow scripts must be deterministic and self-contained; {label} is not allowed"
            )));
        }
    }

    for (object, property, label) in [
        ("Date", "now", "Date.now"),
        ("Math", "random", "Math.random"),
        ("Reflect", "construct", "Reflect.construct"),
    ] {
        if contains_js_member_call_outside_strings(body, object, property) {
            return Err(FunctionCallError::RespondToModel(format!(
                "workflow scripts must be deterministic and self-contained; {label} is not allowed"
            )));
        }
    }

    if let Some(label) = forbidden_bracket_call_label(body) {
        return Err(FunctionCallError::RespondToModel(format!(
            "workflow scripts must be deterministic and self-contained; {label} is not allowed"
        )));
    }

    if contains_js_property_call_outside_strings(body, "constructor") {
        return Err(FunctionCallError::RespondToModel(
            "workflow scripts must be deterministic and self-contained; constructor-chain execution is not allowed"
                .to_string(),
        ));
    }

    if contains_js_pattern_outside_strings(body, "WebAssembly") {
        return Err(FunctionCallError::RespondToModel(
            "workflow scripts must be deterministic and self-contained; WebAssembly is not allowed"
                .to_string(),
        ));
    }

    Ok(())
}

fn find_matching_brace(source: &str, open_index: usize) -> Result<usize, FunctionCallError> {
    let mut state = JsScanState::Code;
    let mut depth = 0usize;
    let mut escaped = false;
    let mut chars = source[open_index..].char_indices().peekable();

    while let Some((relative_index, ch)) = chars.next() {
        let index = open_index + relative_index;
        match state {
            JsScanState::Code => match ch {
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Ok(index);
                    }
                }
                '\'' => state = JsScanState::SingleString,
                '"' => state = JsScanState::DoubleString,
                '`' => state = JsScanState::TemplateString,
                '/' => match chars.peek().map(|(_, next)| *next) {
                    Some('/') => {
                        chars.next();
                        state = JsScanState::LineComment;
                    }
                    Some('*') => {
                        chars.next();
                        state = JsScanState::BlockComment;
                    }
                    _ => {}
                },
                _ => {}
            },
            JsScanState::SingleString => scan_string_char(ch, '\'', &mut escaped, &mut state),
            JsScanState::DoubleString => scan_string_char(ch, '"', &mut escaped, &mut state),
            JsScanState::TemplateString => scan_string_char(ch, '`', &mut escaped, &mut state),
            JsScanState::LineComment => {
                if ch == '\n' {
                    state = JsScanState::Code;
                }
            }
            JsScanState::BlockComment => {
                if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                    chars.next();
                    state = JsScanState::Code;
                }
            }
        }
    }

    Err(FunctionCallError::RespondToModel(
        "workflow meta object is missing its closing brace".to_string(),
    ))
}

fn skip_js_whitespace(source: &str, mut index: usize) -> usize {
    while let Some(ch) = source[index..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        index += ch.len_utf8();
    }
    index
}

fn find_meta_field_value_index(meta_literal: &str, field: &str) -> Option<usize> {
    for key in [
        field.to_string(),
        format!("'{field}'"),
        format!("\"{field}\""),
    ] {
        let mut search_start = 0usize;
        while let Some(relative_index) = meta_literal[search_start..].find(key.as_str()) {
            let key_index = search_start + relative_index;
            if key == field && !has_identifier_boundaries(meta_literal, key_index, field.len()) {
                search_start = key_index + key.len();
                continue;
            }
            if !is_code_position(meta_literal, key_index) {
                search_start = key_index + key.len();
                continue;
            }

            let mut value_index = key_index + key.len();
            value_index = skip_js_whitespace(meta_literal, value_index);
            if !meta_literal[value_index..].starts_with(':') {
                search_start = key_index + key.len();
                continue;
            }
            value_index += ':'.len_utf8();
            value_index = skip_js_whitespace(meta_literal, value_index);
            return Some(value_index);
        }
    }
    None
}

fn extract_meta_string_field(meta_literal: &str, field: &str) -> Option<String> {
    let value_index = find_meta_field_value_index(meta_literal, field)?;
    parse_js_string_literal(meta_literal, value_index).map(|(value, _end_index)| value)
}

fn extract_meta_literal_field(
    meta_literal: &str,
    field: &str,
) -> Result<Option<String>, FunctionCallError> {
    let Some(value_index) = find_meta_field_value_index(meta_literal, field) else {
        return Ok(None);
    };
    let Some(open) = meta_literal[value_index..].chars().next() else {
        return Err(FunctionCallError::RespondToModel(format!(
            "workflow meta field `{field}` must be an object or array literal"
        )));
    };
    let close = match open {
        '{' => '}',
        '[' => ']',
        _ => {
            return Err(FunctionCallError::RespondToModel(format!(
                "workflow meta field `{field}` must be an object or array literal"
            )));
        }
    };
    let end = find_matching_delimiter(meta_literal, value_index, open, close).ok_or_else(|| {
        FunctionCallError::RespondToModel(format!(
            "workflow meta field `{field}` is missing its closing delimiter"
        ))
    })?;
    Ok(Some(meta_literal[value_index..=end].trim().to_string()))
}

fn extract_meta_phases(
    meta_literal: &str,
) -> Result<Vec<WorkflowPhaseMetadata>, FunctionCallError> {
    let Some(value_index) = find_meta_field_value_index(meta_literal, "phases") else {
        return Ok(Vec::new());
    };
    if !meta_literal[value_index..].starts_with('[') {
        return Err(FunctionCallError::RespondToModel(
            "workflow meta field `phases` must be an array literal".to_string(),
        ));
    }
    let array_end =
        find_matching_delimiter(meta_literal, value_index, '[', ']').ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "workflow meta field `phases` is missing its closing bracket".to_string(),
            )
        })?;
    let mut phases = Vec::new();
    let mut cursor = value_index + '['.len_utf8();
    while cursor < array_end {
        cursor = skip_js_whitespace(meta_literal, cursor);
        if cursor >= array_end {
            break;
        }
        if meta_literal[cursor..].starts_with(',') {
            cursor += ','.len_utf8();
            continue;
        }
        if !meta_literal[cursor..].starts_with('{') {
            return Err(FunctionCallError::RespondToModel(
                "workflow meta field `phases` must contain object literals".to_string(),
            ));
        }
        let object_end =
            find_matching_delimiter(meta_literal, cursor, '{', '}').ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "workflow meta phase entry is missing its closing brace".to_string(),
                )
            })?;
        let entry = &meta_literal[cursor..=object_end];
        let title = extract_meta_string_field(entry, "title").ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "workflow meta phase entries must include string field `title`".to_string(),
            )
        })?;
        let model = extract_meta_string_field(entry, "model");
        phases.push(WorkflowPhaseMetadata { title, model });
        cursor = object_end + '}'.len_utf8();
        cursor = skip_js_whitespace(meta_literal, cursor);
        if cursor < array_end && !meta_literal[cursor..].starts_with(',') {
            return Err(FunctionCallError::RespondToModel(
                "workflow meta phase entries must be comma-separated".to_string(),
            ));
        }
    }
    Ok(phases)
}

fn find_matching_delimiter(
    source: &str,
    open_index: usize,
    open: char,
    close: char,
) -> Option<usize> {
    let mut state = JsScanState::Code;
    let mut depth = 0usize;
    let mut escaped = false;
    let mut chars = source[open_index..].char_indices().peekable();

    while let Some((relative_index, ch)) = chars.next() {
        let index = open_index + relative_index;
        match state {
            JsScanState::Code => {
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(index);
                    }
                } else {
                    match ch {
                        '\'' => state = JsScanState::SingleString,
                        '"' => state = JsScanState::DoubleString,
                        '`' => state = JsScanState::TemplateString,
                        '/' => match chars.peek().map(|(_, next)| *next) {
                            Some('/') => {
                                chars.next();
                                state = JsScanState::LineComment;
                            }
                            Some('*') => {
                                chars.next();
                                state = JsScanState::BlockComment;
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
            JsScanState::SingleString => scan_string_char(ch, '\'', &mut escaped, &mut state),
            JsScanState::DoubleString => scan_string_char(ch, '"', &mut escaped, &mut state),
            JsScanState::TemplateString => scan_string_char(ch, '`', &mut escaped, &mut state),
            JsScanState::LineComment => {
                if ch == '\n' {
                    state = JsScanState::Code;
                }
            }
            JsScanState::BlockComment => {
                if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                    chars.next();
                    state = JsScanState::Code;
                }
            }
        }
    }

    None
}

fn has_identifier_boundaries(source: &str, start: usize, len: usize) -> bool {
    let before = source[..start].chars().next_back();
    let after = source[start + len..].chars().next();
    !before.is_some_and(is_js_identifier_char) && !after.is_some_and(is_js_identifier_char)
}

fn parse_js_string_literal(source: &str, start: usize) -> Option<(String, usize)> {
    let quote = source[start..].chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }

    let mut value = String::new();
    let mut escaped = false;
    for (relative_index, ch) in source[start + quote.len_utf8()..].char_indices() {
        let index = start + quote.len_utf8() + relative_index;
        if escaped {
            value.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return Some((value, index + ch.len_utf8()));
        }
        value.push(ch);
    }
    None
}

fn is_code_position(source: &str, target_index: usize) -> bool {
    let mut state = JsScanState::Code;
    let mut escaped = false;
    let mut chars = source.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        if index >= target_index {
            return state == JsScanState::Code;
        }
        match state {
            JsScanState::Code => match ch {
                '\'' => state = JsScanState::SingleString,
                '"' => state = JsScanState::DoubleString,
                '`' => state = JsScanState::TemplateString,
                '/' => match chars.peek().map(|(_, next)| *next) {
                    Some('/') => {
                        chars.next();
                        state = JsScanState::LineComment;
                    }
                    Some('*') => {
                        chars.next();
                        state = JsScanState::BlockComment;
                    }
                    _ => {}
                },
                _ => {}
            },
            JsScanState::SingleString => scan_string_char(ch, '\'', &mut escaped, &mut state),
            JsScanState::DoubleString => scan_string_char(ch, '"', &mut escaped, &mut state),
            JsScanState::TemplateString => scan_string_char(ch, '`', &mut escaped, &mut state),
            JsScanState::LineComment => {
                if ch == '\n' {
                    state = JsScanState::Code;
                }
            }
            JsScanState::BlockComment => {
                if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                    chars.next();
                    state = JsScanState::Code;
                }
            }
        }
    }

    state == JsScanState::Code
}

fn contains_js_pattern_outside_strings(source: &str, needle: &str) -> bool {
    let mut state = JsScanState::Code;
    let mut escaped = false;
    let mut chars = source.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        match state {
            JsScanState::Code => {
                if source[index..].starts_with(needle) {
                    return true;
                }
                match ch {
                    '\'' => state = JsScanState::SingleString,
                    '"' => state = JsScanState::DoubleString,
                    '`' => state = JsScanState::TemplateString,
                    '/' => match chars.peek().map(|(_, next)| *next) {
                        Some('/') => {
                            chars.next();
                            state = JsScanState::LineComment;
                        }
                        Some('*') => {
                            chars.next();
                            state = JsScanState::BlockComment;
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            JsScanState::SingleString => scan_string_char(ch, '\'', &mut escaped, &mut state),
            JsScanState::DoubleString => scan_string_char(ch, '"', &mut escaped, &mut state),
            JsScanState::TemplateString => scan_string_char(ch, '`', &mut escaped, &mut state),
            JsScanState::LineComment => {
                if ch == '\n' {
                    state = JsScanState::Code;
                }
            }
            JsScanState::BlockComment => {
                if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                    chars.next();
                    state = JsScanState::Code;
                }
            }
        }
    }

    false
}

fn contains_js_call_outside_strings(source: &str, callee: &str) -> bool {
    find_code_matches(source, callee).any(|index| {
        if !has_identifier_boundaries(source, index, callee.len()) {
            return false;
        }
        let cursor = skip_js_whitespace_and_comments(source, index + callee.len());
        source[cursor..].starts_with('(')
    })
}

fn contains_js_member_call_outside_strings(source: &str, object: &str, property: &str) -> bool {
    find_code_matches(source, object).any(|index| {
        if !has_identifier_boundaries(source, index, object.len()) {
            return false;
        }
        let mut cursor = skip_js_whitespace_and_comments(source, index + object.len());
        if !source[cursor..].starts_with('.') {
            return false;
        }
        cursor += '.'.len_utf8();
        cursor = skip_js_whitespace_and_comments(source, cursor);
        if !source[cursor..].starts_with(property) {
            return false;
        }
        if !has_identifier_boundaries(source, cursor, property.len()) {
            return false;
        }
        let cursor = skip_js_whitespace_and_comments(source, cursor + property.len());
        source[cursor..].starts_with('(')
    })
}

fn forbidden_bracket_call_label(source: &str) -> Option<&'static str> {
    for property in find_bracket_call_properties_outside_strings(source) {
        match property.as_str() {
            "eval" => return Some("eval"),
            "Function" => return Some("Function constructor"),
            "require" => return Some("CommonJS require"),
            "import" => return Some("dynamic import"),
            "Date" => return Some("Date"),
            _ => {}
        }
    }
    if contains_js_bracket_member_call_outside_strings(source, "Date", "now") {
        return Some("Date.now");
    }
    if contains_js_bracket_member_call_outside_strings(source, "Math", "random") {
        return Some("Math.random");
    }
    None
}

fn find_bracket_call_properties_outside_strings(source: &str) -> Vec<String> {
    let mut properties = Vec::new();
    for index in find_code_matches(source, "[") {
        let mut cursor = skip_js_whitespace_and_comments(source, index + '['.len_utf8());
        let Some((property, end)) = parse_js_string_literal(source, cursor) else {
            continue;
        };
        cursor = skip_js_whitespace_and_comments(source, end);
        if !source[cursor..].starts_with(']') {
            continue;
        }
        cursor += ']'.len_utf8();
        cursor = skip_js_whitespace_and_comments(source, cursor);
        if source[cursor..].starts_with('(') {
            properties.push(property);
        }
    }
    properties
}

fn contains_js_bracket_member_call_outside_strings(
    source: &str,
    object: &str,
    property: &str,
) -> bool {
    find_code_matches(source, object).any(|index| {
        if !has_identifier_boundaries(source, index, object.len()) {
            return false;
        }
        let mut cursor = skip_js_whitespace_and_comments(source, index + object.len());
        if !source[cursor..].starts_with('[') {
            return false;
        }
        cursor += '['.len_utf8();
        cursor = skip_js_whitespace_and_comments(source, cursor);
        let Some((parsed_property, end)) = parse_js_string_literal(source, cursor) else {
            return false;
        };
        if parsed_property != property {
            return false;
        }
        cursor = skip_js_whitespace_and_comments(source, end);
        if !source[cursor..].starts_with(']') {
            return false;
        }
        cursor += ']'.len_utf8();
        cursor = skip_js_whitespace_and_comments(source, cursor);
        source[cursor..].starts_with('(')
    })
}

fn contains_js_property_call_outside_strings(source: &str, property: &str) -> bool {
    find_code_matches(source, property).any(|index| {
        if !has_identifier_boundaries(source, index, property.len()) {
            return false;
        }
        let Some(prefix) = source[..index].trim_end().chars().next_back() else {
            return false;
        };
        if prefix != '.' {
            return false;
        }
        let cursor = skip_js_whitespace_and_comments(source, index + property.len());
        source[cursor..].starts_with('(')
    })
}

fn contains_js_new_expression_outside_strings(source: &str, constructor: &str) -> bool {
    find_code_matches(source, "new").any(|index| {
        if !has_identifier_boundaries(source, index, "new".len()) {
            return false;
        }
        let cursor_after_new = index + "new".len();
        let cursor = skip_js_whitespace_and_comments(source, cursor_after_new);
        if !source[cursor..].starts_with(constructor) {
            return false;
        }
        if !has_identifier_boundaries(source, cursor, constructor.len()) {
            return false;
        }
        let cursor = skip_js_whitespace_and_comments(source, cursor + constructor.len());
        source[cursor..].starts_with('(')
    })
}

fn skip_js_whitespace_and_comments(source: &str, mut index: usize) -> usize {
    loop {
        index = skip_js_whitespace(source, index);
        if source[index..].starts_with("//") {
            index += "//".len();
            while let Some(ch) = source[index..].chars().next() {
                index += ch.len_utf8();
                if ch == '\n' {
                    break;
                }
            }
            continue;
        }
        if source[index..].starts_with("/*") {
            index += "/*".len();
            if let Some(relative_end) = source[index..].find("*/") {
                index += relative_end + "*/".len();
                continue;
            }
            return source.len();
        }
        return index;
    }
}

fn find_code_matches<'a>(source: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    let mut state = JsScanState::Code;
    let mut escaped = false;
    let mut chars = source.char_indices().peekable();
    std::iter::from_fn(move || {
        while let Some((index, ch)) = chars.next() {
            match state {
                JsScanState::Code => {
                    if source[index..].starts_with(needle) {
                        return Some(index);
                    }
                    match ch {
                        '\'' => state = JsScanState::SingleString,
                        '"' => state = JsScanState::DoubleString,
                        '`' => state = JsScanState::TemplateString,
                        '/' => match chars.peek().map(|(_, next)| *next) {
                            Some('/') => {
                                chars.next();
                                state = JsScanState::LineComment;
                            }
                            Some('*') => {
                                chars.next();
                                state = JsScanState::BlockComment;
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                JsScanState::SingleString => scan_string_char(ch, '\'', &mut escaped, &mut state),
                JsScanState::DoubleString => scan_string_char(ch, '"', &mut escaped, &mut state),
                JsScanState::TemplateString => scan_string_char(ch, '`', &mut escaped, &mut state),
                JsScanState::LineComment => {
                    if ch == '\n' {
                        state = JsScanState::Code;
                    }
                }
                JsScanState::BlockComment => {
                    if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                        chars.next();
                        state = JsScanState::Code;
                    }
                }
            }
        }

        None
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsScanState {
    Code,
    SingleString,
    DoubleString,
    TemplateString,
    LineComment,
    BlockComment,
}

fn scan_string_char(ch: char, quote: char, escaped: &mut bool, state: &mut JsScanState) {
    if *escaped {
        *escaped = false;
    } else if ch == '\\' {
        *escaped = true;
    } else if ch == quote {
        *state = JsScanState::Code;
    }
}

fn is_js_identifier_char(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()
}
