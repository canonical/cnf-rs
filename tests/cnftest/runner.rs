//! The harness to run the conformance corpus against an implementation.

use super::corpus::{Case, Data, Direction, Kind, Reformat};
use super::matcher::{expand_vars, match_error};

// The directions a case may declare, in the order they run.
pub const DIRECTIONS: [Direction; 4] = [
    Direction::ToJson,
    Direction::FromJson,
    Direction::ToAst,
    Direction::FromAst,
];

// What one convert call came to: its text, the implementation's
// error, or the refusal to run the direction.
pub enum Outcome {
    Text(String),
    Error(String),
    Skip,
}

// One case's run: its failures, and the skip note when nothing ran.
pub struct Report {
    pub failures: Vec<String>,
    pub skipped: Option<String>,
}

/// Run one case through every direction it declares, collecting
/// the failures. check decides the comparison: comparing the wanted
/// text against the got text in the direction's target format.
pub fn run_case(
    name: &str,
    case: &Case,
    convert: &dyn Fn(Direction, &str, &Reformat) -> Outcome,
    check: &dyn Fn(&str, Kind, &str, &str) -> Result<(), String>,
) -> Report {
    let mut report = Report { failures: vec![], skipped: None };
    let mut ran = false;
    for dir in DIRECTIONS {
        let directive = case.directive(dir);
        if directive.kind == Kind::None {
            continue;
        }
        if run_direction(name, case, dir, directive, convert, check, &mut report.failures) {
            ran = true;
        }
    }
    if !ran && report.failures.is_empty() {
        report.skipped = Some(format!(
            "{name}: skipping unsupported formats case: {}",
            formats(case)
        ));
    }
    report
}

// Run one direction of one case; ran reports whether it ran at all.
fn run_direction(
    name: &str,
    case: &Case,
    dir: Direction,
    directive: &super::corpus::Directive,
    convert: &dyn Fn(Direction, &str, &Reformat) -> Outcome,
    check: &dyn Fn(&str, Kind, &str, &str) -> Result<(), String>,
    failures: &mut Vec<String>,
) -> bool {
    let dirn = direction_name(dir);
    if !known_kind(directive.kind) {
        failures.push(format!("{name}: {dirn}: unknown directive kind {:?}", directive.kind));
        return true;
    }
    let (inputs, expectations) = match blocks(case, dir, convert) {
        Blocks::Skip => return false,
        Blocks::Broken(err) => {
            failures.push(format!("{name}: {dirn}: broken case: {err}"));
            return true;
        }
        Blocks::Ok(inputs, expectations) => (inputs, expectations),
    };
    for (i, block) in inputs.iter().enumerate() {
        match expand_vars(case, block, block) {
            Ok(_) => {}
            Err(err) => {
                failures.push(format!(
                    "{name}: {dirn}: {} block #{}: broken case: {err}",
                    source_name(dir),
                    i + 1
                ));
                return true;
            }
        }
    }
    // Probe every input for the refusal, before any verdict.
    if inputs
        .iter()
        .any(|block| matches!(convert(dir, block, &case.reformat), Outcome::Skip))
    {
        return false;
    }
    if directive.is_error() {
        run_errors(name, case, dir, &inputs, &case.error, convert, failures);
        return true;
    }
    if expectations.is_empty() {
        failures.push(format!("{name}: {dirn} needs a {} block", format_name(dir)));
        return true;
    }
    for (i, block) in inputs.iter().enumerate() {
        let want = match expand_vars(case, block, &expectations[0]) {
            Ok(want) => want,
            Err(err) => {
                failures.push(format!(
                    "{name}: {dirn}: {} block #{}: broken case: {err}",
                    source_name(dir),
                    i + 1
                ));
                continue;
            }
        };
        let got = match convert(dir, block, &case.reformat) {
            Outcome::Error(err) => {
                failures.push(format!(
                    "{name}: {dirn}: {} block #{}: {err}",
                    source_name(dir),
                    i + 1
                ));
                continue;
            }
            Outcome::Skip => return false,
            Outcome::Text(text) => text,
        };
        if let Err(report) = check(format_name(dir), directive.kind, &want, &got) {
            failures.push(format!(
                "{name}: {dirn}: {} block #{}: {report}",
                source_name(dir),
                i + 1
            ));
        }
    }
    true
}

// The error kind compares each input's raised message against the
// case's error expectation, literal or regexp.
fn run_errors(
    name: &str,
    case: &Case,
    dir: Direction,
    inputs: &[String],
    error: &Data,
    convert: &dyn Fn(Direction, &str, &Reformat) -> Outcome,
    failures: &mut Vec<String>,
) {
    for (i, block) in inputs.iter().enumerate() {
        let msg = match convert(dir, block, &case.reformat) {
            Outcome::Error(msg) => msg,
            Outcome::Skip => return,
            Outcome::Text(_) => {
                failures.push(format!(
                    "{name}: {}: {} block #{}: no error, want {:?}",
                    direction_name(dir),
                    source_name(dir),
                    i + 1,
                    error.text
                ));
                continue;
            }
        };
        let want = match expand_vars(case, block, &error.text) {
            Ok(want) => want,
            Err(err) => {
                failures.push(format!(
                    "{name}: {}: {} block #{}: broken case: {err}",
                    direction_name(dir),
                    source_name(dir),
                    i + 1
                ));
                continue;
            }
        };
        let want = Data { text: want, regexp: error.regexp };
        match match_error(&want, &msg) {
            Ok(true) => {}
            Ok(false) => failures.push(format!(
                "{name}: {}: {} block #{}: error:\n  got  {msg:?}\n  want {:?}",
                direction_name(dir),
                source_name(dir),
                i + 1,
                want.text
            )),
            Err(err) => failures.push(format!(
                "{name}: {}: {} block #{}: broken case: {err}",
                direction_name(dir),
                source_name(dir),
                i + 1
            )),
        }
    }
}

// The blocks of one direction: its inputs and expectations. The
// from-ast inputs convert through the harness's own to-ast when the
// case's ast blocks are spelled as cnf; an entry the converter skips
// is dropped, and a case whose every entry refuses skips its whole
// direction.
enum Blocks {
    Ok(Vec<String>, Vec<String>),
    Skip,
    Broken(String),
}

fn blocks(
    case: &Case,
    dir: Direction,
    convert: &dyn Fn(Direction, &str, &Reformat) -> Outcome,
) -> Blocks {
    match dir {
        Direction::ToJson => Blocks::Ok(case.cnf.clone(), case.json.clone()),
        Direction::FromJson => Blocks::Ok(case.json.clone(), case.cnf.clone()),
        Direction::ToAst => Blocks::Ok(case.cnf.clone(), case.ast.clone()),
        Direction::FromAst if !case.parse_ast => {
            Blocks::Ok(case.ast.clone(), case.cnf.clone())
        }
        Direction::FromAst => {
            let mut ast_blocks = vec![];
            for block in &case.ast {
                match convert(Direction::ToAst, block, &case.reformat) {
                    Outcome::Skip => {}
                    Outcome::Error(err) => return Blocks::Broken(err),
                    Outcome::Text(text) => ast_blocks.push(text),
                }
            }
            if ast_blocks.is_empty() && !case.ast.is_empty() {
                return Blocks::Skip;
            }
            Blocks::Ok(ast_blocks, case.cnf.clone())
        }
        Direction::None => unreachable!(),
    }
}

fn known_kind(kind: Kind) -> bool {
    matches!(kind, Kind::AdvisedText | Kind::ExactValue | Kind::AdvisedValue | Kind::Error)
}

pub fn direction_name(dir: Direction) -> &'static str {
    match dir {
        Direction::ToJson => "to-json",
        Direction::FromJson => "from-json",
        Direction::ToAst => "to-ast",
        Direction::FromAst => "from-ast",
        Direction::None => "?",
    }
}

// The format of the direction's input blocks.
fn source_name(dir: Direction) -> &'static str {
    match dir {
        Direction::ToJson | Direction::ToAst => "cnf",
        Direction::FromJson => "json",
        Direction::FromAst => "ast",
        Direction::None => "source",
    }
}

// The format of the direction's expectation blocks.
fn format_name(dir: Direction) -> &'static str {
    match dir {
        Direction::ToJson => "json",
        Direction::FromJson | Direction::FromAst => "cnf",
        Direction::ToAst => "ast",
        Direction::None => "expectation",
    }
}

// The source formats of the case's declared directions, ", "-joined.
fn formats(case: &Case) -> String {
    DIRECTIONS
        .iter()
        .filter(|dir| case.directive(**dir).kind != Kind::None)
        .map(|dir| source_name(*dir))
        .collect::<Vec<_>>()
        .join(", ")
}
