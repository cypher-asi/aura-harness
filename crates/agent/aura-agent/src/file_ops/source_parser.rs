pub fn extract_struct_fields(content: &str, type_name: &str) -> Option<String> {
    let struct_prefix_pub = format!("pub struct {type_name}");
    let struct_prefix = format!("struct {type_name}");
    let lines: Vec<&str> = content.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let after = trimmed
            .strip_prefix(&struct_prefix_pub)
            .or_else(|| trimmed.strip_prefix(&struct_prefix));
        let Some(after) = after else {
            continue;
        };
        match after.chars().next() {
            Some('{' | ' ' | '<') | None => {}
            _ => continue,
        }
        if !trimmed.contains('{') {
            if trimmed.ends_with(';') {
                return None;
            }
            continue;
        }

        return Some(extract_braced_block(&lines, i));
    }
    None
}

pub fn extract_pub_signatures(content: &str, type_name: &str) -> Vec<String> {
    let mut signatures = Vec::new();
    let mut in_impl = false;
    let mut impl_depth: i32 = 0;
    let mut body_entered = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if !in_impl {
            if is_impl_for_type(trimmed, type_name) {
                in_impl = true;
                impl_depth = 0;
                body_entered = false;
                for ch in trimmed.chars() {
                    match ch {
                        '{' => impl_depth += 1,
                        '}' => impl_depth -= 1,
                        _ => {}
                    }
                }
                if impl_depth > 0 {
                    body_entered = true;
                }
            }
            continue;
        }

        for ch in trimmed.chars() {
            match ch {
                '{' => impl_depth += 1,
                '}' => impl_depth -= 1,
                _ => {}
            }
        }

        if !body_entered {
            if impl_depth > 0 {
                body_entered = true;
            }
            continue;
        }

        if trimmed.starts_with("pub fn ") || trimmed.starts_with("pub async fn ") {
            let sig = match trimmed.find('{') {
                Some(pos) => trimmed[..pos].trim(),
                None => trimmed,
            };
            if !sig.is_empty() {
                signatures.push(sig.to_string());
            }
        }

        if impl_depth <= 0 {
            in_impl = false;
            body_entered = false;
        }
    }

    signatures
}

/// Shared helper: extract a brace-delimited block starting at `start_idx`.
fn extract_braced_block(lines: &[&str], start_idx: usize) -> String {
    let mut result = String::new();
    let mut depth: i32 = 0;
    for line in &lines[start_idx..] {
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        result.push_str(line.trim());
        result.push('\n');
        if depth <= 0 {
            break;
        }
    }
    result
}

/// Check whether a line is an `impl` block header for the given type name.
fn is_impl_for_type(line: &str, type_name: &str) -> bool {
    if !line.starts_with("impl") {
        return false;
    }
    if line.len() > 4 {
        let fifth = line.as_bytes()[4];
        if fifth.is_ascii_alphanumeric() || fifth == b'_' {
            return false;
        }
    }

    let bytes = line.as_bytes();
    let tn_bytes = type_name.as_bytes();
    let tn_len = tn_bytes.len();

    let mut i = 4;
    while i + tn_len <= bytes.len() {
        if &bytes[i..i + tn_len] == tn_bytes {
            let before_ok =
                i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            let after_ok = i + tn_len >= bytes.len()
                || !(bytes[i + tn_len].is_ascii_alphanumeric() || bytes[i + tn_len] == b'_');
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}
