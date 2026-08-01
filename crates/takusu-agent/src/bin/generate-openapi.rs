//! Generate the OpenAPI spec JSON file for the agent HTTP API.
//!
//! Usage: `cargo run -p takusu-agent --bin generate-openapi --features openapi -- -o agent-openapi.json`

use std::io::Write;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut output = "agent-openapi.json".to_string();
    while let Some(arg) = args.next() {
        if arg == "-o" || arg == "--output" {
            if let Some(path) = args.next() {
                output = path;
            }
        } else if arg == "-h" || arg == "--help" {
            eprintln!("Usage: generate-openapi [-o <path>]");
            return;
        }
    }

    let spec = takusu_agent::openapi::generate_openapi();
    let json = serde_json::to_string_pretty(&spec).expect("serialize openapi");
    let mut file = std::fs::File::create(&output).expect("create output file");
    file.write_all(json.as_bytes()).expect("write openapi spec");
    eprintln!("wrote {output}");
}
