// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;
use std::error::Error;
use std::io;
use std::process::Command;

use crate::config;

#[derive(Args, Debug)]
pub struct DiscoverArgs {
    #[arg(short, long)]
    verbose: bool,

    #[arg()]
    hostnames: Vec<String>,
}

pub fn discover(args: &DiscoverArgs) -> Result<(), Box<dyn Error>> {
    let mut cluster = config::Config::new();
    for hostname in args.hostnames.iter() {
        let node = discover_one(hostname, args.verbose).unwrap();
        cluster.add_node(node);
    }
    println!("{}", toml::to_string_pretty(&cluster).unwrap());
    Ok(())
}

fn discover_one(hostname: &str, verbose: bool) -> io::Result<config::Node> {
    // Get Zpools
    if verbose {
        eprintln!("\nDiscovering zpools for host={hostname}");
        eprintln!("Running command on host: 'zpool list -H -o name'");
    }
    let output = Command::new("ssh")
        .args([hostname, "zpool", "list", "-H", "-o", "name"])
        .output()?;
    if verbose {
        eprintln!(
            "stdout: {}",
            String::from_utf8(output.stdout.clone()).unwrap()
        );
        eprintln!("stderr: {}", String::from_utf8(output.stderr).unwrap());
    }

    // Get Targets and parse both Zpools and Lustre targets
    if verbose {
        eprintln!("Discovering lustre targets for host={hostname}");
        eprintln!("Running command on host: 'mount -t lustre'");
    }
    let targets_output = Command::new("ssh")
        .args([hostname, "mount", "-t", "lustre"])
        .output()?;
    if verbose {
        eprintln!(
            "stdout: {}",
            String::from_utf8(output.stdout.clone()).unwrap()
        );
        eprintln!(
            "stderr: {}",
            String::from_utf8(targets_output.stderr).unwrap()
        );
    }

    let zpools = parse_zpool_list_output(
        &String::from_utf8(output.stdout).unwrap(),
        &String::from_utf8(targets_output.stdout).unwrap(),
    );

    if verbose {
        eprintln!("zpools: {:?}", zpools);
    }

    Ok(config::Node {
        hostname: hostname.to_string(),
        zpools,
    })
}

fn parse_zpool_list_output(output: &str, targets_output: &str) -> Vec<config::Zpool> {
    output
        .lines()
        .map(|name| config::Zpool {
            name: name.to_string(),
            targets: parse_lustre_targets_output(name.to_string(), targets_output),
        })
        .collect()
}

fn parse_lustre_targets_output(zpool_name: String, output: &str) -> Vec<config::Target> {
    let mut targets: Vec<config::Target> = Vec::new();
    for line in output.lines() {
        let mut tokens = line.split_whitespace();
        let device = tokens.next().unwrap();
        // Check if the zpool we are in matches the device, otherwise go to the next output line
        if let Some(dev) = device.split('/').next() {
            if dev != zpool_name {
                continue;
            }
        }
        let mountpoint = tokens.nth(1).unwrap();
        let opts = tokens.nth(2).unwrap();
        let opts = opts.trim_matches(|c| c == '(' || c == ')').split(',');
        let mut kind: Option<String> = None;
        let mut fstype: Option<String> = None;
        for opt in opts {
            if opt.starts_with("svname=") {
                if opt.contains("MDT") {
                    kind = Some("mdt".to_string());
                } else if opt.contains("MGS") {
                    kind = Some("mgs".to_string());
                } else if opt.contains("OST") {
                    kind = Some("ost".to_string());
                }
            }
            if opt.starts_with("osd=") {
                fstype = match opt {
                    "osd=osd-zfs" => Some("zfs".to_string()),
                    "osd=osd-ldiskfs" => Some("ldiskfs".to_string()),
                    _ => None,
                };
            }
        }
        let kind = match kind {
            Some(k) => k,
            None => continue,
        };
        let fstype = match fstype {
            Some(f) => f,
            None => continue,
        };
        targets.push(config::Target {
            device: device.to_string(),
            kind: kind.to_string(),
            mountpoint: mountpoint.to_string(),
            fstype: fstype.to_string(),
        });
    }

    targets
}
