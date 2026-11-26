//! Rule frontmatter: when a rule file applies.

use super::Kind;

/// `content` split into its `---` frontmatter (if any) and the rest.
pub(super) fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let Some(rest) = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"))
    else {
        return (None, content);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return (Some(&rest[..offset]), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    (None, content)
}

/// The frontmatter fields Cursor rules use. Parsed by hand: Cursor writes
/// `globs: **/*.rs` unquoted, which is not valid YAML.
#[derive(Debug, Default)]
pub(super) struct RuleMeta {
    pub(super) description: String,
    pub(super) globs: Vec<String>,
    always_apply: bool,
}

pub(super) fn parse_rule_meta(frontmatter: &str) -> RuleMeta {
    let mut meta = RuleMeta::default();
    let mut in_globs_list = false;
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if in_globs_list {
            if let Some(item) = trimmed.strip_prefix('-') {
                meta.globs.extend(split_globs(item));
                continue;
            }
            in_globs_list = false;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "description" => meta.description = unquote(value).to_string(),
            "globs" if value.is_empty() => in_globs_list = true,
            "globs" => meta.globs.extend(split_globs(value)),
            "alwaysApply" => meta.always_apply = value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    meta
}

fn split_globs(value: &str) -> impl Iterator<Item = String> + '_ {
    let value = value.trim();
    let value = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    value
        .split(',')
        .map(unquote)
        .filter(|glob| !glob.is_empty())
        .map(str::to_string)
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner.trim();
        }
    }
    value
}

/// When a rule applies, per its frontmatter. `None` for rules that apply
/// only when the user names them.
pub(super) fn rule_kind(frontmatter: Option<&str>) -> Option<Kind> {
    let Some(frontmatter) = frontmatter else {
        return Some(Kind::Always);
    };
    let meta = parse_rule_meta(frontmatter);
    if meta.always_apply {
        Some(Kind::Always)
    } else if !meta.globs.is_empty() {
        Some(Kind::Scoped(meta.globs.join(", ")))
    } else if !meta.description.is_empty() {
        Some(Kind::OnRequest(meta.description))
    } else {
        None
    }
}
