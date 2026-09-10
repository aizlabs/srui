mod reconnect;
mod report;
mod serialization;
mod terminal;

use reconnect::reconnect;
use report::{Artifacts, Output};
use serialization::{
    build_transactions, encode_transactions, frame_encoded_transactions, serialization, Fixture,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use terminal::terminal;

fn argument(name: &str) -> Result<String, String> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == name {
            return args
                .next()
                .ok_or_else(|| format!("{name} requires a value"));
        }
    }
    Err(format!("missing {name}"))
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), String> {
    let fixture_path = PathBuf::from(argument("--fixture")?);
    let output_path = PathBuf::from(argument("--output")?);
    let profile = argument("--profile")?;
    let iterations = if profile == "full" { 500 } else { 25 };
    let fixture: Fixture = serde_json::from_slice(
        &fs::read(&fixture_path).map_err(|error| format!("{}: {error}", fixture_path.display()))?,
    )
    .map_err(|error| error.to_string())?;

    let canonical_transactions = build_transactions(&fixture)?;
    let canonical_encoded = encode_transactions(&canonical_transactions);
    let canonical_bytes = frame_encoded_transactions(&canonical_encoded)?;
    let canonical_digest = Sha256::digest(&canonical_bytes);
    let output = Output {
        artifacts: Artifacts {
            canonical_transaction_sha256: format!("{canonical_digest:x}"),
            canonical_transaction_bytes: canonical_bytes.len(),
        },
        sections: vec![
            serialization(&fixture, iterations)?,
            reconnect(iterations).await?,
            terminal(iterations).await?,
        ],
    };
    let json = serde_json::to_vec_pretty(&output).map_err(|error| error.to_string())?;
    fs::write(Path::new(&output_path), json).map_err(|error| error.to_string())
}
