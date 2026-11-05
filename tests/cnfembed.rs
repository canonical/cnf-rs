// The conformance corpus against cnfembed's public surface.
//
// to-json parses each cnf block and renders it as json. The other
// directions skip: the crate translates, it does not parse json or
// write cnf.

mod cnftest;

use cnftest::corpus::{read_all, Direction, Kind, Reformat};
use cnftest::runner::{run_case, Outcome};

const SPEC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/spec/cases");

fn convert(direction: Direction, source: &str, _reformat: &Reformat) -> Outcome {
    match direction {
        Direction::ToJson => match cnf_rs::to_json(source, None) {
            Ok(text) => Outcome::Text(text),
            Err(err) => Outcome::Error(err.to_string()),
        },
        _ => Outcome::Skip,
    }
}

// Compare after trimming the whitespace between tokens: the case
// files write their json pretty-printed, to_json emits compact json.
fn check(fmt: &str, _kind: Kind, want: &str, got: &str) -> Result<(), String> {
    let (want, got) = if fmt == "json" {
        (trim_json(want), trim_json(got))
    } else {
        (want.to_string(), got.to_string())
    };
    fn strip(t: &str) -> &str {
        t.strip_suffix('\n').unwrap_or(t)
    }
    if strip(&got) != strip(&want) {
        return Err(format!("mismatch:\nwant:\n{want}\ngot:\n{got}"));
    }
    Ok(())
}

// Trim each line's whitespace and join the rest into one compact
// string; a json string value holds no newline, so the trim
// touches layout only.
fn trim_json(text: &str) -> String {
    let mut out = String::new();
    for line in text.split('\n') {
        let line = line.trim();
        if !line.is_empty() {
            out.push_str(line);
        }
    }
    out
}

fn run_topic(name: &str, failures: &mut Vec<String>, skips: &mut usize) {
    let topics = read_all(SPEC).expect("cannot read spec/cases");
    let topic = topics
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("no topic {name} in {SPEC}"));
    for case in &topic.cases {
        let case_name = format!("{}/{}", topic.name, case.name);
        let report = run_case(&case_name, case, &convert, &check);
        if let Some(note) = report.skipped {
            println!("{note}");
            *skips += 1;
        }
        failures.extend(report.failures);
    }
}

macro_rules! topic {
    ($name:ident) => {
        #[test]
        fn $name() {
            let mut failures = vec![];
            let mut skips = 0;
            run_topic(stringify!($name), &mut failures, &mut skips);
            assert!(
                failures.is_empty(),
                "\n{}",
                failures.join("\n")
            );
        }
    };
}

topic!(comments);
topic!(lists);
topic!(maps);
topic!(numbers);
topic!(reformat);
topic!(strings);
topic!(top);
