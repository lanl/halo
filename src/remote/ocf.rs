// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

//! ocf.rs
//!
//! This module implements OCF resource agent operations on nodes which
//! runs a resource.

use std::io;
use std::process::Command;

/// OCF Resource Agent operations that can be performed on a resource.
#[derive(Debug)]
pub enum Operation {
    Start,
    Stop,
    Monitor,
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Operation::Start => "start",
                Operation::Stop => "stop",
                Operation::Monitor => "monitor",
            }
        )
    }
}

/// OCF Resource Agent arguments are key-value pairs which are passed to the
/// resource agent script as environment variables.
pub struct Arguments {
    pub args: Vec<(String, String)>,
}

/// Prepare list of key, value pairs by prepending "OCF_RESKEY_" to each key name.
///
/// The OCF resource agents expect arguments to be in the form "OCF_RESKEY_key=value".
impl std::convert::From<&Vec<(&str, &str)>> for Arguments {
    fn from(args: &Vec<(&str, &str)>) -> Self {
        let args = args
            .iter()
            .map(|(k, v)| (format!("OCF_RESKEY_{k}"), v.to_string()))
            .collect();

        Arguments { args }
    }
}

/// OCF Resource Agent statuses are listed in /usr/lib/ocf/lib/heartbeat/ocf-returncodes
#[derive(Debug, PartialEq)]
pub enum Status {
    Success,
    ErrGeneric,
    ErrArgs,
    ErrUnimplemented,
    ErrPerm,
    ErrInstalled,
    ErrConfigured,
    ErrNotRunning,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Status::Success => "OCF_SUCCESS",
                Status::ErrGeneric => "OCF_ERR_GENERIC",
                Status::ErrArgs => "OCF_ERR_ARGS",
                Status::ErrUnimplemented => "OCF_ERR_UNIMPLEMENTED",
                Status::ErrPerm => "OCF_ERR_PERM",
                Status::ErrInstalled => "OCF_ERR_INSTALLED",
                Status::ErrConfigured => "OCF_ERR_CONFIGURED",
                Status::ErrNotRunning => "OCF_NOT_RUNNING",
            }
        )
    }
}

impl std::convert::From<i32> for Status {
    fn from(st: i32) -> Self {
        match st {
            0 => Status::Success,
            1 => Status::ErrGeneric,
            2 => Status::ErrArgs,
            3 => Status::ErrUnimplemented,
            4 => Status::ErrPerm,
            5 => Status::ErrInstalled,
            6 => Status::ErrConfigured,
            7 => Status::ErrNotRunning,
            _ => {
                eprintln!("Warning: unexpected return status for Resource Agent: {st}");
                Status::ErrUnimplemented
            }
        }
    }
}

/// Typical installation path for directory containing OCF Resource Agent scripts.
const OCF_ROOT: &str = "/usr/lib/ocf";

/// Perform an on operation on an OCF resource.
///
/// - resource: the name of the resource, which corresponds to its location under
///   `/usr/lib/ocf/resource.d/` (or `OCF_ROOT`, if that environment variable is defined).
/// - op: Operation to perform
/// - args: List of arguments to the operation.
/// - test_id: set the HALO_TEST_ID environment variable. Used in the testing environment to
///   distinguish multiple agents running on the same system.
pub fn do_operation(
    resource: &str,
    op: Operation,
    ocf_operation_args: &Arguments,
    cli_args: &crate::remote::Cli,
) -> io::Result<i32> {
    let test_id = match &cli_args.test_id {
        Some(id) => id.clone(),
        None => std::process::id().to_string(),
    };

    let ocf_root = cli_args
        .ocf_root
        .clone()
        .unwrap_or(std::env::var("OCF_ROOT").unwrap_or(OCF_ROOT.to_string()));
    let script = format!("{ocf_root}/resource.d/{resource}");

    let output = Command::new(script)
        .args([op.to_string()])
        .env("OCF_ROOT", ocf_root)
        .env("HALO_TEST_ID", test_id)
        .envs(ocf_operation_args.args.clone())
        .output()?;

    let exit_code = match output.status.code() {
        Some(code) => code,
        None => {
            eprintln!("Could not get exit status from Resource Agent");
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
    };

    if exit_code != 0 && cli_args.verbose {
        println!("Output: {:?}", output);
    }

    Ok(exit_code)
}
