// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;
use std::collections::HashMap;
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
    let mut config = config::Config {
        hosts: Vec::new(),
        failover_pairs: None,
    };
    for hostname in args.hostnames.iter() {
        let host = discover_one_host(hostname, args.verbose).unwrap();
        config.hosts.push(host);
    }
    println!("{}", toml::to_string_pretty(&config).unwrap());
    Ok(())
}

/// Attempt to discover all of the resources (zpools and lustre targerts) running on `hostname`,
/// and construct them into a config::Host object that owns those resources.
fn discover_one_host(hostname: &str, verbose: bool) -> io::Result<config::Host> {
    let zpool_output = get_zpool_output(hostname, verbose)?;

    let mut resources = parse_zpool_output(zpool_output);

    let lustre_output = get_lustre_output(hostname, verbose)?;

    let lustre_resources = parse_lustre_output(lustre_output);

    resources.extend(lustre_resources);

    Ok(config::Host {
        hostname: hostname.to_string(),
        resources,
        fence_agent: None,
        fence_parameters: None,
    })
}

fn parse_lustre_output(output: String) -> HashMap<String, config::Resource> {
    let mut resources = HashMap::new();

    for line in output.lines() {
        let res = config::Resource::new_lustre(line);

        let target = res.parameters.get("target").unwrap();

        resources.insert(target.to_string(), res);
    }

    resources
}

fn get_lustre_output(hostname: &str, verbose: bool) -> io::Result<String> {
    // Get Targets and parse both Zpools and Lustre targets
    if verbose {
        eprintln!("Discovering lustre targets for host={hostname}");
        eprintln!("Running command on hosggt: 'mount -t lustre'");
    }
    let output = Command::new("ssh")
        .args([hostname, "mount", "-t", "lustre"])
        .output()?;
    if verbose {
        eprintln!(
            "stdout: {}",
            String::from_utf8(output.stdout.clone()).unwrap()
        );
        eprintln!("stderr: {}", String::from_utf8(output.stderr).unwrap());
    }

    Ok(String::from_utf8(output.stdout).unwrap())
}

fn parse_zpool_output(output: String) -> HashMap<String, config::Resource> {
    HashMap::from_iter(output.lines().map(|line| {
        (
            line.to_string(),
            config::Resource::new_zpool(line.to_string()),
        )
    }))
}

fn get_zpool_output(hostname: &str, verbose: bool) -> io::Result<String> {
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

    Ok(String::from_utf8(output.stdout).unwrap())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{parse_lustre_output, parse_zpool_output};
    use crate::config::*;

    #[test]
    fn parse_zpools() {
        let output = String::from("zpool_1\nzpool_2");
        let resources = parse_zpool_output(output);
        assert_eq!(resources.len(), 2);

        let goal = HashMap::from([
            (
                "zpool_1".to_string(),
                Resource::new_zpool("zpool_1".to_string()),
            ),
            (
                "zpool_2".to_string(),
                Resource::new_zpool("zpool_2".to_string()),
            ),
        ]);

        assert_eq!(resources, goal);
    }

    #[test]
    fn parse_lustre() {
        let output = concat!("oss01e0/ost2 on /mnt/ost2 type lustre (ro,svname=test-OST0002,mgsnode=10.0.0.1@tcp:10.0.0.2@tcp,osd=osd-zfs)\n",
                             "oss01e1/ost3 on /mnt/ost3 type lustre (ro,svname=test-OST0003,mgsnode=10.0.0.1@tcp:10.0.0.2@tcp,osd=osd-zfs)");

        let resources = parse_lustre_output(output.to_string());
        assert_eq!(resources.len(), 2);

        let goal_1 = Resource {
            kind: "lustre/Lustre".to_string(),
            parameters: HashMap::from([
                ("mountpoint".to_string(), "/mnt/ost2".to_string()),
                ("target".to_string(), "oss01e0/ost2".to_string()),
                ("kind".to_string(), "ost".to_string()),
            ]),
            requires: Some("oss01e0".to_string()),
        };
        let goal_2 = Resource {
            kind: "lustre/Lustre".to_string(),
            parameters: HashMap::from([
                ("mountpoint".to_string(), "/mnt/ost3".to_string()),
                ("target".to_string(), "oss01e1/ost3".to_string()),
                ("kind".to_string(), "ost".to_string()),
            ]),
            requires: Some("oss01e1".to_string()),
        };
        let goal = HashMap::from([
            ("oss01e0/ost2".to_string(), goal_1),
            ("oss01e1/ost3".to_string(), goal_2),
        ]);

        assert_eq!(resources, goal);
    }
}
