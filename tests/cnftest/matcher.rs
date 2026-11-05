//! Variable expansion and error matching for the conformance corpus.

use std::sync::OnceLock;

use regex::Regex;

use super::corpus::{Case, Data};

// One ${name} or ${name:form} reference; the forms are l, c, lc.
fn var_ref() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\$\{([a-z]+)(?::([a-z]+))?\}").unwrap())
}

/// Substitute the case's variables into target, from source.
/// An undefined name, an unknown form, or a locator that finds
/// nothing is the error.
pub fn expand_vars(case: &Case, source: &str, target: &str) -> Result<String, String> {
    if !target.contains("${") {
        return Ok(target.to_string());
    }
    let mut out = String::new();
    let mut prev = 0;
    let mut broken: Option<String> = None;
    for m in var_ref().captures_iter(target) {
        let whole = m.get(0).unwrap();
        let name = m.get(1).unwrap().as_str();
        let form = m.get(2).map_or("", |g| g.as_str());
        out.push_str(&target[prev..whole.start()]);
        match case.vars.get(name) {
            None => {
                if broken.is_none() {
                    broken = Some(format!("reference {} is not a defined var", whole.as_str()));
                }
                out.push_str(whole.as_str());
            }
            Some(data) if form.is_empty() && !data.regexp => out.push_str(&data.text),
            Some(data) => match locate(source, data) {
                Err(err) => {
                    if broken.is_none() {
                        broken = Some(err);
                    }
                    out.push_str(whole.as_str());
                }
                Ok((text, line, col)) => match form {
                    "" => {
                        out.push_str(&text);
                    }
                    "l" => out.push_str(&line.to_string()),
                    "c" => out.push_str(&col.to_string()),
                    "lc" => out.push_str(&format!("{line}:{col}")),
                    _ => {
                        if broken.is_none() {
                            broken =
                                Some(format!("reference {} has an unknown form", whole.as_str()));
                        }
                        out.push_str(whole.as_str());
                    }
                },
            },
        }
        prev = whole.end();
    }
    out.push_str(&target[prev..]);
    if let Some(err) = broken {
        return Err(err);
    }
    // Any ${...} left over - a reference whose name part is
    // not [a-z]+ - is undefined by the same rule.
    if let Some(rest) = out.find("${").map(|i| &out[i..]) {
        let reference = match rest.find('}') {
            Some(end) => &rest[..end + 1],
            None => rest,
        };
        return Err(format!("reference {reference} is not a defined var"));
    }
    Ok(out)
}

/// Check an error message against a case's error expectation:
/// a literal compares for equality, a regexp matches anywhere.
/// The error reports a broken case - an undefined reference or
/// a locator that matches nothing.
pub fn match_error(want: &Data, msg: &str) -> Result<bool, String> {
    if !want.regexp {
        return Ok(msg == want.text);
    }
    match Regex::new(&want.text) {
        Ok(re) => Ok(re.is_match(msg)),
        Err(err) => Err(format!("bad error regexp: {err}")),
    }
}

// Find the expectation in the source; return its text and 1-based
// line and column, counted in characters within the line. A literal
// is pinned at its first occurrence; a regexp matches anywhere, its
// first group the pin.
fn locate(source: &str, data: &Data) -> Result<(String, usize, usize), String> {
    let (start, end) = if !data.regexp {
        let start = source
            .find(&data.text)
            .ok_or_else(|| format!("literal {:?} matches nothing in the block", data.text))?;
        (start, start + data.text.len())
    } else {
        let re = Regex::new(&format!("(?m){}", data.text))
            .map_err(|err| format!("bad locator regexp: {err}"))?;
        let m = re.captures(source)
            .ok_or_else(|| format!("locator regexp {:?} matches nothing in the block", data.text))?;
        let whole = m.get(0).unwrap();
        if re.captures_len() > 1 {
            match m.get(1) {
                Some(g) => (g.start(), g.end()),
                None => (whole.start(), whole.end()),
            }
        } else {
            (whole.start(), whole.end())
        }
    };
    let text = source[start..end].to_string();
    let before = &source[..start];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let col = source[line_start..start].chars().count() + 1;
    Ok((text, line, col))
}
