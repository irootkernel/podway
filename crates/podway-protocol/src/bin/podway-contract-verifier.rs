#![forbid(unsafe_code)]

use std::{env, path::PathBuf, process};

use podway_protocol::{ReleaseContractVerifierConfigV1, verify_release_contract_v1};

fn main() {
    match run() {
        Ok(()) => {}
        Err(error) => {
            eprintln!("{error}");
            process::exit(1);
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut contract_root = None;
    let mut podway = None;
    let mut podwayd = None;
    let mut expected_target = None;
    let mut expected_source_commit = None;
    let mut arguments = env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        let destination = match argument.to_str() {
            Some("--contract-root") => &mut contract_root,
            Some("--podway") => &mut podway,
            Some("--podwayd") => &mut podwayd,
            Some("--expected-target") => &mut expected_target,
            Some("--expected-source-commit") => &mut expected_source_commit,
            _ => return Err("unknown or non-UTF-8 contract verifier argument".into()),
        };
        if destination.is_some() {
            return Err("contract verifier argument was repeated".into());
        }
        *destination = Some(
            arguments
                .next()
                .ok_or("contract verifier argument has no value")?,
        );
    }
    let config = ReleaseContractVerifierConfigV1::new(
        PathBuf::from(contract_root.ok_or("--contract-root is required")?),
        PathBuf::from(podway.ok_or("--podway is required")?),
        PathBuf::from(podwayd.ok_or("--podwayd is required")?),
        expected_target
            .ok_or("--expected-target is required")?
            .into_string()
            .map_err(|_| "--expected-target must be UTF-8")?,
        expected_source_commit
            .ok_or("--expected-source-commit is required")?
            .into_string()
            .map_err(|_| "--expected-source-commit must be UTF-8")?,
    );
    let receipt = verify_release_contract_v1(&config)?;
    println!("{}", serde_json::to_string(&receipt)?);
    Ok(())
}
