//! Parsing the refiner model's reply into refined candidates.

use crate::types::{CandidateType, MemoryCandidate, MemoryScope, RefinedCandidate};

/// Parse a numbered candidate refinement line (`N. KEEP|DROP ...`) into the
/// 0-based candidate index and its refined form.
pub(super) fn parse_candidate_refinement(
    line: &str,
    candidates: &[MemoryCandidate],
) -> Option<(usize, RefinedCandidate)> {
    let (index_text, rest) = line.split_once(". ")?;
    let idx: usize = match index_text.trim().parse::<usize>() {
        Ok(n) if n >= 1 && n <= candidates.len() => n - 1,
        _ => return None,
    };
    let candidate = candidates.get(idx)?;

    let keep = rest.starts_with("KEEP");

    let confidence = extract_float(rest, "confidence=").unwrap_or(candidate.preliminary_confidence);
    let importance = extract_float(rest, "importance=").unwrap_or(candidate.preliminary_importance);
    let key =
        extract_quoted(rest, "key=").unwrap_or_else(|| candidate.key.clone().unwrap_or_default());
    let scope = extract_scope(rest);

    Some((
        idx,
        RefinedCandidate {
            candidate_type: candidate.candidate_type.clone(),
            key,
            value: candidate.value.clone(),
            summary: candidate.summary.clone(),
            confidence,
            importance,
            keep,
            scope,
            trigger: None,
            steps: None,
            skill_name: None,
        },
    ))
}

pub(super) fn kept_by_default(c: &MemoryCandidate) -> RefinedCandidate {
    RefinedCandidate {
        candidate_type: c.candidate_type.clone(),
        key: c.key.clone().unwrap_or_default(),
        value: c.value.clone(),
        summary: c.summary.clone(),
        confidence: c.preliminary_confidence,
        importance: c.preliminary_importance,
        keep: true,
        scope: MemoryScope::Agent,
        trigger: None,
        steps: None,
        skill_name: None,
    }
}

pub(super) fn parse_fact_line(line: &str) -> Option<RefinedCandidate> {
    let key = extract_quoted(line, "key=")?;
    let value = extract_quoted(line, "value=")?;
    let confidence = extract_float(line, "confidence=").unwrap_or(0.8);
    let importance = extract_float(line, "importance=").unwrap_or(0.5);
    let scope = extract_scope(line);

    Some(RefinedCandidate {
        candidate_type: CandidateType::Fact,
        key,
        value: serde_json::Value::String(value),
        summary: None,
        confidence,
        importance,
        keep: true,
        scope,
        trigger: None,
        steps: None,
        skill_name: None,
    })
}

pub(super) fn parse_procedure_line(line: &str) -> Option<RefinedCandidate> {
    let name = extract_quoted(line, "name=")?;
    let trigger = extract_quoted(line, "trigger=")?;
    let steps_raw = extract_quoted(line, "steps=").unwrap_or_default();
    let skill = extract_quoted(line, "skill=").unwrap_or_else(|| "none".to_string());
    let confidence = extract_float(line, "confidence=").unwrap_or(0.8);
    let importance = extract_float(line, "importance=").unwrap_or(0.7);
    let scope = extract_scope(line);

    let steps: Vec<String> = steps_raw
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let skill_name = if skill.eq_ignore_ascii_case("none") || skill.is_empty() {
        None
    } else {
        Some(skill)
    };

    Some(RefinedCandidate {
        candidate_type: CandidateType::Procedure,
        key: name,
        value: serde_json::json!({ "trigger": trigger, "steps": steps }),
        summary: Some(trigger.clone()),
        confidence,
        importance,
        keep: true,
        scope,
        trigger: Some(trigger),
        steps: Some(steps),
        skill_name,
    })
}

pub(super) fn extract_scope(text: &str) -> MemoryScope {
    match extract_unquoted(text, "scope=")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "user" | "personal" => MemoryScope::User,
        "project" | "workspace" | "shared" => MemoryScope::Project,
        _ => MemoryScope::Agent,
    }
}

pub(super) fn extract_unquoted(text: &str, prefix: &str) -> Option<String> {
    let start = text.find(prefix)? + prefix.len();
    let rest = &text[start..];
    let end = rest
        .find(|character: char| character.is_whitespace())
        .unwrap_or(rest.len());
    Some(rest[..end].trim_matches(['"', '\'']).to_string())
}

pub(super) fn extract_float(text: &str, prefix: &str) -> Option<f32> {
    let start = text.find(prefix)? + prefix.len();
    let end = text[start..]
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .map_or(text.len(), |e| start + e);
    text[start..end].parse().ok()
}

pub(super) fn extract_quoted(text: &str, prefix: &str) -> Option<String> {
    let start = text.find(prefix)? + prefix.len();
    let rest = &text[start..];
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_string())
    } else {
        let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}
