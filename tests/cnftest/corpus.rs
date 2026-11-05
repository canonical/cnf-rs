//! The conformance-corpus reader: the case files under spec/cases.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

use regex::Regex;

// A topic is one cases/<name>.md file: a name and its cases.
pub struct Topic {
    pub name: String,
    pub cases: Vec<Case>,
}

// One expected text a case carries: a literal, or a regexp.
#[derive(Clone)]
pub struct Data {
    pub text: String,
    pub regexp: bool,
}

impl Default for Data {
    fn default() -> Self {
        Data { text: String::new(), regexp: false }
    }
}

// One direction of one case.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Direction {
    #[default]
    None,
    ToJson,
    FromJson,
    ToAst,
    FromAst,
}

// The kind a directive expects, "" when the direction is absent.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Kind {
    #[default]
    None,
    AdvisedText,
    ExactValue,
    AdvisedValue,
    Error,
}

// One direction's expected outcome.
#[derive(Clone, Default)]
pub struct Directive {
    pub kind: Kind,
}

impl Directive {
    pub fn is_error(&self) -> bool {
        self.kind == Kind::Error
    }
}

// The reformat settings a case declares, the corpus's vocabulary.
#[derive(Clone, Default)]
pub struct Reformat {
    pub unfold_list_from_items: i64,
    pub unfold_map_from_items: i64,
    pub fold_list_upto_items: i64,
    pub fold_map_upto_items: i64,
    pub fold_path_upto_depth: i64,
    pub fold_path_from_depth: i64,
    pub unfold_path_from_depth: i64,
    pub unfold_path_upto_depth: i64,
    pub soft_wrap_string_from_width: i64,
    pub hard_wrap_string_from_width: i64,
    pub reindent: String,
    pub ignore_comments: bool,
}

impl Reformat {
    // set sets one named setting; the vocabulary is fixed here.
    pub fn set(&mut self, name: &str, val: i64) -> Result<(), String> {
        match name {
            "unfold-list-from-items" => self.unfold_list_from_items = val,
            "unfold-map-from-items" => self.unfold_map_from_items = val,
            "fold-list-upto-items" => self.fold_list_upto_items = val,
            "fold-map-upto-items" => self.fold_map_upto_items = val,
            "fold-path-upto-depth" => self.fold_path_upto_depth = val,
            "fold-path-from-depth" => self.fold_path_from_depth = val,
            "unfold-path-from-depth" => self.unfold_path_from_depth = val,
            "unfold-path-upto-depth" => self.unfold_path_upto_depth = val,
            "soft-wrap-string-from-width" => self.soft_wrap_string_from_width = val,
            "hard-wrap-string-from-width" => self.hard_wrap_string_from_width = val,
            "reindent" => {
                return Err(format!("reformat setting {name:?} is not an int"))
            }
            _ => return Err(format!("unknown reformat setting {name:?}")),
        }
        Ok(())
    }
}

// One ## Case section: a name, directives, and its blocks.
pub struct Case {
    pub name: String,
    pub to_json: Directive,
    pub from_json: Directive,
    pub to_ast: Directive,
    pub from_ast: Directive,
    pub error: Data,
    pub vars: BTreeMap<String, Data>,
    pub reformat: Reformat,
    pub parse_ast: bool,
    pub cnf: Vec<String>,
    pub json: Vec<String>,
    pub ast: Vec<String>,
}

impl Case {
    pub fn new(name: String) -> Self {
        Case {
            name,
            to_json: Directive::default(),
            from_json: Directive::default(),
            to_ast: Directive::default(),
            from_ast: Directive::default(),
            error: Data::default(),
            vars: BTreeMap::new(),
            reformat: Reformat::default(),
            parse_ast: false,
            cnf: vec![],
            json: vec![],
            ast: vec![],
        }
    }

    pub fn directive(&self, direction: Direction) -> &Directive {
        match direction {
            Direction::ToJson => &self.to_json,
            Direction::FromJson => &self.from_json,
            Direction::ToAst => &self.to_ast,
            Direction::FromAst => &self.from_ast,
            Direction::None => unreachable!(),
        }
    }
}

// Read every *.md file under dir, skipping the README.
pub fn read_all(dir: &str) -> Result<Vec<Topic>, String> {
    let mut files = vec![];
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot read {dir}: {e}"))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|e| e == "md") {
            files.push(path);
        }
    }
    if files.is_empty() {
        return Err(format!("no case files found in {dir}"));
    }
    files.sort();
    let mut topics = vec![];
    for path in files {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        if name == "README" {
            continue // documentation, not a case
        }
        let topic = read_topic(&path, &name)?;
        if topic.cases.is_empty() {
            return Err(format!("{name}: no cases found"));
        }
        for case in &topic.cases {
            validate_blocks(case)?;
        }
        topics.push(topic);
    }
    Ok(topics)
}

// Check the machine-read blocks of a case: each ast/json block is
// exactly one json value, and a directive reads every block.
fn validate_blocks(case: &Case) -> Result<(), String> {
    if !case.parse_ast {
        for block in &case.ast {
            one_json_value("ast", block)?;
        }
    }
    for block in &case.json {
        one_json_value("json", block)?;
    }
    let reads_cnf = case.to_json.kind != Kind::None
        || case.to_ast.kind != Kind::None
        || (case.from_json.kind != Kind::None && !case.from_json.is_error())
        || (case.from_ast.kind != Kind::None && !case.from_ast.is_error());
    let reads_json = case.from_json.kind != Kind::None
        || (case.to_json.kind != Kind::None && !case.to_json.is_error());
    let reads_ast = case.from_ast.kind != Kind::None
        || (case.to_ast.kind != Kind::None && !case.to_ast.is_error());
    if !case.cnf.is_empty() && !reads_cnf {
        return Err("cnf block without a directive that reads it".into());
    }
    if !case.json.is_empty() && !reads_json {
        return Err("json block without a directive that reads it".into());
    }
    if !case.ast.is_empty() && !reads_ast {
        return Err("ast block without a directive that reads it".into());
    }
    Ok(())
}

// text holds exactly one json value, nothing but blanks from it.
fn one_json_value(kind: &str, text: &str) -> Result<(), String> {
    let mut de = serde_json::Deserializer::from_str(text).into_iter::<serde_json::Value>();
    match de.next() {
        None => return Err(format!("{kind} block is not JSON")),
        Some(Err(err)) => return Err(format!("{kind} block is not JSON: {err}")),
        Some(Ok(_)) => {}
    }
    if de.next().is_some() {
        return Err(format!("{kind} block has trailing content after the value"));
    }
    Ok(())
}

// Parse one case file: the header, the prose, the case sections.
fn read_topic(path: &Path, name: &str) -> Result<Topic, String> {
    let data = fs::read_to_string(path).map_err(|e| format!("{}", e))?;
    let mut topic = Topic { name: name.to_string(), cases: vec![] };
    let lines: Vec<&str> = data.split('\n').collect();
    if lines.is_empty() || !lines[0].starts_with("# ") {
        return Err("cannot find opening # header".into());
    }
    let mut section = String::new();
    let mut block: Vec<String> = vec![];
    let mut in_fence = false;
    let mut vars_mode = false;
    let mut pretty_mode = false;
    let mut pretty_seen: HashSet<String> = HashSet::new();

    for ln in lines[1..].iter().copied() {
        if ln.starts_with("## Case") {
            let case_name = case_name(ln)?;
            if topic.cases.iter().any(|c| c.name == case_name) {
                return Err(format!("duplicate case name {case_name:?}"));
            }
            // No flush here: every content fence flushes at its close.
            topic.cases.push(Case::new(case_name));
            pretty_seen.clear();
            section.clear();
            block.clear();
            in_fence = false;
            vars_mode = false;
            pretty_mode = false;
            continue;
        }
        if topic.cases.is_empty() {
            continue // prose before the first case
        }
        let case = topic.cases.last_mut().unwrap();
        if in_fence {
            if ln.starts_with("```") {
                flush(case, &mut block, &section);
                in_fence = false;
                vars_mode = false;
                pretty_mode = false;
            } else if !section.is_empty() {
                block.push(ln.to_string());
            } else if ln.trim().is_empty() {
                continue;
            } else if vars_mode && (ln.starts_with(' ') || ln.starts_with('\t')) {
                let (var_name, var_data) = parse_header_value(ln)?;
                if !is_var_name(&var_name) {
                    return Err(format!("var name {var_name:?} is not [a-z]+"));
                }
                if case.vars.contains_key(&var_name) {
                    return Err(format!("duplicate var {var_name:?}"));
                }
                case.vars.insert(var_name, var_data);
            } else if pretty_mode && (ln.starts_with(' ') || ln.starts_with('\t')) {
                let colon = ln.find(':').ok_or("cannot find key-value separator")?;
                let setting = ln[..colon].trim();
                if !pretty_seen.insert(setting.to_string()) {
                    return Err(format!("duplicate reformat setting {setting:?}"));
                }
                if setting == "reindent" {
                    let (_, val) = parse_header_pair(ln)?;
                    case.reformat.reindent = val;
                } else if setting == "ignore-comments" {
                    let val = ln[colon + 1..].trim();
                    if val != "true" && val != "false" {
                        return Err(format!(
                            "{ln}: reformat setting {setting:?}: cannot parse {val:?}"
                        ));
                    }
                    case.reformat.ignore_comments = val == "true";
                } else {
                    let val = ln[colon + 1..].trim();
                    let num: i64 = val.parse().map_err(|_| {
                        format!("{ln}: reformat setting {setting:?}: cannot parse {val:?}")
                    })?;
                    case.reformat.set(setting, num).map_err(|e| format!("{ln}: {e}"))?;
                }
            } else if ln.trim() == "vars:" {
                vars_mode = true;
                pretty_mode = false;
            } else if ln.trim() == "reformat:" {
                pretty_mode = true;
                vars_mode = false;
            } else if let Some(val) = ln.strip_prefix("parse-ast:") {
                let val = val.trim();
                if val != "true" {
                    return Err(format!("{ln}: parse-ast expects true, got {val:?}"));
                }
                case.parse_ast = true;
            } else {
                let (key, header_data) = parse_header_value(ln)?;
                if header_data.regexp && key != "error" {
                    return Err(format!("{ln}: a regexp map is not allowed here"));
                }
                // Only error: and the vars take the regexp map.
                if key == "error" {
                    case.error = header_data;
                    continue;
                }
                let kind = match header_data.text.as_str() {
                    "advised-text" => Kind::AdvisedText,
                    "exact-value" => Kind::ExactValue,
                    "advised-value" => Kind::AdvisedValue,
                    "error" => Kind::Error,
                    "" => Kind::None,
                    other => return Err(format!("unknown directive kind {other:?}")),
                };
                match key.as_str() {
                    "to-json" => case.to_json = Directive { kind },
                    "from-json" => case.from_json = Directive { kind },
                    "to-ast" => case.to_ast = Directive { kind },
                    "from-ast" => case.from_ast = Directive { kind },
                    other => return Err(format!("unknown header key {other:?}")),
                }
            }
        } else if let Some(sec) = ln.strip_prefix("### ") {
            // No flush here either: the closing fence already flushed.
            section = sec.trim().to_string();
        } else if ln.starts_with("```") {
            in_fence = true;
        }
    }
    // The final flush catches a file that ends inside a fence.
    if in_fence {
        let case = topic.cases.last_mut().unwrap();
        flush(case, &mut block, &section);
    }
    Ok(topic)
}

// Append the fence's lines to its section block.
fn flush(case: &mut Case, block: &mut Vec<String>, section: &str) {
    if section.is_empty() {
        return;
    }
    // An empty cnf block is meaningful: the empty file.
    if block.is_empty() && section != "cnf" {
        return;
    }
    let text = if section == "cnf" && block.is_empty() {
        String::new()
    } else {
        let mut text = block.join("\n");
        text.push('\n');
        text
    };
    match section {
        "cnf" => case.cnf.push(text),
        "json" => case.json.push(text),
        "ast" => case.ast.push(text),
        _ => {}
    }
    block.clear();
}

// Read the name from a "## Case: <name>" header.
fn case_name(ln: &str) -> Result<String, String> {
    let name = ln["## Case:".len()..].trim();
    if name.is_empty() {
        return Err("case header without a name".into());
    }
    if !name.chars().all(is_bare_rune) {
        return Err(format!("case name {name:?} is not a bare word"));
    }
    Ok(name.to_string())
}

fn is_var_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|ch| ('a'..='z').contains(&ch))
}

fn is_bare_rune(ch: char) -> bool {
    ch == '-' || ch == '_' || ch.is_ascii_alphanumeric()
}

// Split one `key: value` header line: the value is a cnf
// double-quoted string, or a {regexp: "..."} map.
fn parse_header_value(ln: &str) -> Result<(String, Data), String> {
    let colon = ln.find(':').ok_or("cannot find key-value separator")?;
    let key = ln[..colon].trim().to_string();
    let rest = ln[colon + 1..].trim();
    if rest.starts_with('{') {
        // The map form: {regexp: "..."} exactly - one key, nothing else.
        let val = rest
            .strip_prefix("{regexp:")
            .ok_or(format!("cannot parse value {rest:?}: the map key must be regexp"))?;
        let val = val
            .strip_suffix('}')
            .ok_or(format!("cannot parse value {rest:?}: the map does not close"))?
            .trim();
        if val.is_empty() {
            return Err(format!("cannot parse value {rest:?}: the map value is empty"));
        }
        let text = unquote(val)?;
        // The reference harnesses compile here; the value may still
        // hold ${...} var refs, and this regexp engine has no
        // literal fallback for a bad brace, so those compile at
        // match time, once expanded.
        if !text.contains("${") {
            Regex::new(&text).map_err(|err| format!("bad regexp {text:?}: {err}"))?;
        }
        return Ok((key, Data { text, regexp: true }));
    }
    Ok((key, Data { text: unquote(rest)?, regexp: false }))
}

// Split one `key: "value"` header line: the plain-string form.
fn parse_header_pair(ln: &str) -> Result<(String, String), String> {
    let (key, data) = parse_header_value(ln)?;
    if data.regexp {
        return Err(format!(
            "cannot parse value {:?}: a regexp map is not allowed here",
            data.text
        ));
    }
    Ok((key, data.text))
}

// Read one cnf double-quoted string, its escapes decoded.
fn unquote(rest: &str) -> Result<String, String> {
    let chars: Vec<char> = rest.chars().collect();
    let n = chars.len();
    if n < 2 || chars[0] != '"' || chars[n - 1] != '"' {
        return Err(format!("cannot parse value {rest:?}"));
    }
    let mut out = String::new();
    let mut i = 1;
    while i < n - 1 {
        let ch = chars[i];
        if ch == '\\' && i + 1 < n - 1 {
            i += 1;
            match chars[i] {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars[i + 1..(i + 5).min(n - 1)].iter().collect();
                    let code = u32::from_str_radix(&hex, 16)
                        .ok()
                        .filter(|_| hex.len() == 4 && is_hex_run(&hex))
                        .ok_or(format!(
                            "cannot parse escape \\u{hex}: wants four hex digits"
                        ))?;
                    out.push(char::from_u32(code).ok_or("cannot parse escape: bad code")?);
                    i += 4;
                }
                'U' => {
                    let hex: String = chars[i + 1..(i + 9).min(n - 1)].iter().collect();
                    let code = u32::from_str_radix(&hex, 16)
                        .ok()
                        .filter(|_| hex.len() == 8 && is_hex_run(&hex))
                        .ok_or(format!(
                            "cannot parse escape \\U{hex}: wants eight hex digits"
                        ))?;
                    if code > 0x10FFFF {
                        return Err(format!(
                            "cannot parse escape \\U{hex}: out of range"
                        ));
                    }
                    out.push(char::from_u32(code).ok_or("cannot parse escape: bad code")?);
                    i += 8;
                }
                '"' | '\\' => out.push(chars[i]),
                other => return Err(format!("cannot parse escape \\{other}")),
            }
        } else {
            out.push(ch);
        }
        i += 1;
    }
    Ok(out)
}

fn is_hex_run(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}
