# Rust support for the cnf format

## Crates and modules

- `cnfembed` - Text-preserving cnf to json, a single file for embedding.
- `tests/cnftest` - Harness to run the conformance tests.

The crate is implemented with zero dependencies: adding it to a project
brings in no transitive crates at all. The json output is written directly,
and the tools the conformance corpus needs (notably regexp matching) live
in dev-dependencies, compiled only for the tests.


## Specification

Consult the [full specification](https://github.com/canonical/cnf) for examples and details of the format.


## Running tests

The `cnftest` harness runs the full conformance corpus, from a local
copy of the spec kept for the tests. To run the tests:

```shell
cargo test
```


## Performance

The following table offers a vague data point on the performance of the
implementation. It's as fair as I could make it: the two documents spell
the same data in the same indentation shape. Both documents decode to the
identical tree: 6721 maps, 11833 lists, 24605 scalars.

| Test                                     | Parser                | Best (ms) | MB/s |
|------------------------------------------|-----------------------|----------:|-----:|
| yaml => data (`YamlLoader`)              | yaml-rust2, YAML 1.2  |      20.1 | 55.7 |
| cnf => json (`to_json`)                  | cnfembed, Rust        |      17.0 | 99.4 |
| cnf => data (`to_json` + `serde_json`)   | cnfembed + serde_json |      21.2 | 79.3 |


## Example

This simple example shows the conversion of cnf to json:

```cnf
key: "value"
nested.key: 0xff
string:
    """
    Hello
    world
    """
```

One call decodes it to JSON:

```rust
use cnf_rs::to_json;

let json = to_json(doc, None)?;
```

This results in:

```json
{"key":"value","nested":{"key":255},"string":"Hello\nworld\n"}
```

For the time being this crate converts one direction only: cnf to json.

A string that fails to parse returns `Error` with the message
including the line and column of the problem:

```rust
use cnf_rs::{to_json, Error};

if let Err(err) = to_json("a: bare\n", None) {
    println!("{err}"); // 1:4: cannot parse unquoted value `bare`
}
```

For actual embedding into your project, you may opt to copy the
`src/cnfembed.rs` file alone.
