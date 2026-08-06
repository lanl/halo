// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::collections::{HashMap, HashSet};

use clap::Args;

use crate::{
    commands::*,
    manager::http::{ClusterJson, EventJson, ResourceJson},
    Handle, HandledResult,
};

#[derive(Args, Debug, Clone)]
pub struct StatusArgs {
    /// Only print abnormal results (resources that are stopped / failed over, etc.)
    #[arg(short = 'x')]
    exclude_normal: bool,

    /// Maximum number of event entries to output.
    #[arg(short = 'e', long, default_value_t = 10)]
    event_count: usize,
}

/// A representation of a Resource for the topological ordering. The depth is tracked in order to
/// indent resource groups according to their tree structure.
#[derive(Debug)]
struct ResourceNode {
    id: String,
    depth: usize,
}

/// Returns a list of ResourceNodes in a topologically sorted order with the following properties:
///
///   - Resource group roots appear in lexicographic order.
///
///   - Child resources appear after their parent resources, but with no intervening resources from
///     another group.
fn sorted_resource_list(
    cluster: &ClusterJson,
    map: &HashMap<String, ResourceJson>,
) -> Vec<ResourceNode> {
    /// Returns a list of the resource roots, in reverse sorted order. Reversed because the full
    /// topographical sorted list is built up backwards, and then reversed at the end to create
    /// the final list.
    fn get_roots(cluster: &ClusterJson) -> Vec<&ResourceJson> {
        let mut non_roots: HashSet<String> = HashSet::new();

        for res in &cluster.resources {
            for child in &res.dependents {
                non_roots.insert(child.clone());
            }
        }

        let mut roots: Vec<&ResourceJson> = cluster
            .resources
            .iter()
            .filter(|res| !non_roots.contains(&res.id))
            .collect();

        roots.sort_by(|a, b| b.id.cmp(&a.id));

        roots
    }

    /// Visit a node in the DAG, in a depth-first search, building up the topographical order in
    /// the parameter `list` as we go.
    fn visit(
        node: &ResourceJson,
        visited: &mut HashSet<String>,
        list: &mut Vec<ResourceNode>,
        map: &HashMap<String, ResourceJson>,
        depth: usize,
    ) {
        if visited.contains(&node.id) {
            return;
        }

        for child in &node.dependents {
            let child = map.get(child).unwrap();
            // We trust that the data sent to us by the manager is legit, i.e., it doesn't contain
            // cycles. So there is no risk of an infinite loop here.
            visit(child, visited, list, map, depth + 1);
        }

        visited.insert(node.id.clone());

        // This pushes the parent resource onto the list *after* its children...
        list.push(ResourceNode {
            id: node.id.clone(),
            depth,
        });
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut list: Vec<ResourceNode> = Vec::new();

    for node in get_roots(cluster) {
        visit(node, &mut visited, &mut list, map, 0);
    }

    // ...the list was built up in backwards order. Need to reverse it.
    list.into_iter().rev().collect()
}

fn status_and_comment(res: &ResourceJson) -> (String, Option<String>) {
    if !res.exclusive {
        return res.shared_host_status();
    }

    let (status, maybe_comment) = res.single_host_status();

    let location = match status {
        "Running" => format!(" on {}", res.home_host),
        "Running (Failed Over)" => format!(
            " on {}",
            res.failover_host
                .as_ref()
                .expect("Failover host must be set here.")
        ),
        _ => "".to_owned(),
    };

    let status = format!("{}{}", status, location);

    (status, maybe_comment)
}

/// Get the resource parameters as a string.
fn parameters(res: &ResourceJson) -> String {
    let mut s = " [".to_owned();

    let mut first_one = true;
    let mut params: Vec<_> = res.parameters.iter().collect();
    params.sort();
    for (key, value) in params {
        if first_one {
            first_one = false;
        } else {
            s += "; ";
        }
        s += &format!("{key}: {value}");
    }
    s += "]";

    s
}

pub fn status(cli: &Cli, args: &StatusArgs) -> HandledResult<()> {
    let cluster = get_status(cli.socket.as_deref())?;

    let resource_map: HashMap<String, ResourceJson> = cluster
        .resources
        .iter()
        .map(|r| (r.id.clone(), r.clone()))
        .collect();

    for ResourceNode { id, depth } in sorted_resource_list(&cluster, &resource_map) {
        let res = resource_map.get(&id).unwrap();

        if args.exclude_normal && res.single_host_status().0 == "Running" {
            continue;
        }

        print!("{:<20}\t", " ".repeat(depth) + &res.id);
        print!("({})\t", res.kind);

        let (status, maybe_comment) = status_and_comment(res);
        print!("{status}");

        if cli.verbose {
            print!("{}", parameters(res));
        }

        if let Some(comment) = maybe_comment {
            print!(" {comment} ");
        }

        if !res.managed {
            print!(" (Unmanaged)");
        }

        println!();
    }

    println!();

    let mut abnormal = false;
    let mut connected_activated = String::new();
    let mut connected_deactivated = String::new();
    let mut disconnected_activated = String::new();
    let mut disconnected_deactivated = String::new();
    for host in cluster.hosts {
        let node = format!("{},", host.id);
        if host.active {
            if host.connected {
                connected_activated.push_str(&node);
            } else {
                abnormal = true;
                disconnected_activated.push_str(&node);
            }
        } else if host.connected {
            connected_deactivated.push_str(&node);
        } else {
            abnormal = true;
            disconnected_deactivated.push_str(&node);
        }
    }

    let ca: nodeset::NodeSet = connected_activated
        .parse()
        .expect("Unable to parse nodeset from hostnames.");
    let cd: nodeset::NodeSet = connected_deactivated
        .parse()
        .expect("Unable to parse nodeset from hostnames.");
    let da: nodeset::NodeSet = disconnected_activated
        .parse()
        .expect("Unable to parse nodeset from hostnames.");
    let dd: nodeset::NodeSet = disconnected_deactivated
        .parse()
        .expect("Unable to parse nodeset from hostnames.");

    if !args.exclude_normal {
        print!("Connected hosts:\t{}", ca);
        if connected_deactivated.is_empty() {
            println!();
        } else {
            println!(", {} (deactivated)", cd);
        }
    }

    if !args.exclude_normal || abnormal {
        print!("Disconnected hosts:\t{}", da);
        if disconnected_deactivated.is_empty() {
            println!();
        } else {
            println!(", {} (deactivated)", dd);
        }
    }

    if !args.exclude_normal && !cluster.events.is_empty() {
        print!("Events: ");
        println!();

        for e in tail(&cluster.events, args.event_count) {
            println!("{}", e.syslog_print());
        }
    }

    Ok(())
}

fn tail(events: &[EventJson], n: usize) -> &[EventJson] {
    for i in 0..events.len() {
        // Start checking from the end of the events list (newest event checked first):
        let ind = events.len() - i - 1;

        // Return at most n events:
        if i == n {
            return &events[ind + 1..];
        }

        // A "clear" event means the admin doesn't want to see anything older than this, so just
        // return what we've got so far (even if it's fewer events than `n`):
        if events[ind].event == "clear" {
            return &events[ind + 1..];
        }
    }

    events
}

pub fn get_status(socket: Option<&str>) -> HandledResult<ClusterJson> {
    let client = get_http_client(socket)?;

    let response = client
        .get("http://halo_manager/status")
        .send()
        .handle_err(|e| eprintln!("Error making HTTP request: {e:?}"))?;

    response
        .json()
        .handle_err(|e| eprintln!("Error decoding JSON: {e}"))
}
