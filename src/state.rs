// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{
    collections::HashMap,
    fmt,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader},
    sync::Mutex,
};

use chrono::{Local, NaiveDateTime};

use crate::commands::{handled_error, Handle, HandledError, HandledResult};

/// A delta of the state of all hosts and resources between the times before and after the statefile was read.
#[derive(Debug)]
pub struct Delta {
    /// Tracks which hosts have been fenced or reset.
    pub hosts_fenced: HashMap<String, bool>,
    /// Tracks which hosts have been activated or deactivated.
    pub hosts_activated: HashMap<String, bool>,
    /// Tracks which resources have been managed or unmanaged.
    pub resources_managed: HashMap<String, bool>,
}

impl Delta {
    pub fn new() -> Self {
        Delta {
            hosts_fenced: HashMap::new(),
            hosts_activated: HashMap::new(),
            resources_managed: HashMap::new(),
        }
    }

    /// Create a new Delta given a file containing lines representing Record entries.
    pub fn new_from_file(file: &File) -> HandledResult<Self> {
        let records = Record::get_all_from_file(file)?;

        let mut state_delta = Self::new();

        for record in records {
            match record.event {
                Event::Manage => {
                    Self::add_or_overwrite_entry(
                        &mut state_delta.resources_managed,
                        &record.obj_id,
                        true,
                    );
                }
                Event::Unmanage => {
                    Self::add_or_overwrite_entry(
                        &mut state_delta.resources_managed,
                        &record.obj_id,
                        false,
                    );
                }
                Event::Activate => {
                    Self::add_or_overwrite_entry(
                        &mut state_delta.hosts_activated,
                        &record.obj_id,
                        true,
                    );
                }
                Event::Deactivate => {
                    Self::add_or_overwrite_entry(
                        &mut state_delta.hosts_activated,
                        &record.obj_id,
                        false,
                    );
                }
                Event::Fence => {
                    Self::add_or_overwrite_entry(
                        &mut state_delta.hosts_fenced,
                        &record.obj_id,
                        true,
                    );
                }
                Event::FenceReset => {
                    Self::add_or_overwrite_entry(
                        &mut state_delta.hosts_fenced,
                        &record.obj_id,
                        false,
                    );
                }
            }
        }
        Ok(state_delta)
    }

    /// Get a Vec of deduplicated hosts that this Delta applies to.
    pub fn hosts(&self) -> Vec<String> {
        let mut merged: HashMap<String, Option<String>> = HashMap::new();
        for (key, _) in &self.hosts_fenced {
            merged.insert(key.clone(), None);
        }
        for (key, _) in &self.hosts_activated {
            merged.insert(key.clone(), None);
        }
        merged.into_keys().collect()
    }

    /// Get a Vec of deduplicated resources that this Delta applies to.
    pub fn resources(&self) -> Vec<String> {
        self.resources_managed
            .keys()
            .map(|s| s.to_string())
            .collect()
    }

    /// Helper function to either insert a new String/boolean entry into the given HashMap or
    /// replace the existing entry.
    fn add_or_overwrite_entry(tracker: &mut HashMap<String, bool>, key: &str, new_val: bool) {
        tracker.insert(key.to_string(), new_val);
    }
}

#[derive(Debug)]
pub struct State {
    /// File that stores state
    file: Mutex<File>,
    /// Delta of state from the statefile.
    pub delta: Delta,
}

impl State {
    pub fn new(path: &str) -> HandledResult<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .handle_err(|e| {
                eprintln!("could not open statefile path '{path}': '{e}'");
            })?;
        let delta = Delta::new_from_file(&file)?;
        Ok(Self {
            file: Mutex::new(file),
            delta,
        })
    }

    /// Writes a single record to the statefile.
    pub fn write_record(&self, record: Record) -> HandledResult<()> {
        use std::io::Write;
        self.file
            .lock()
            .unwrap()
            .write_all(&[record.as_string().as_bytes(), &[b'\n']].concat())
            .handle_err(|e| {
                eprintln!("failed to write to statefile '{:?}': '{e}'", self.file);
            })
    }
}

/// An single record of an event that is tracked in the statefile.
#[derive(Debug, PartialEq)]
pub struct Record {
    pub timestamp: NaiveDateTime,
    pub event: Event,
    pub obj_id: String,
    pub comment: Option<String>,
}

impl Record {
    pub fn new(event: Event, obj_id: String, comment: Option<String>) -> Self {
        Record {
            timestamp: Local::now().naive_local(),
            event,
            obj_id,
            comment,
        }
    }

    /// Attempt to get all Records from a File, sorted by timestamp in ascending order.
    pub fn get_all_from_file(file: &File) -> HandledResult<Vec<Record>> {
        let lines = BufReader::new(file).lines();
        let mut records: Vec<Record> = lines
            .map(|line| -> HandledResult<Record> {
                let line = line.handle_err(|e| {
                    eprintln!("unable to parse statefile line: '{e}'");
                })?;

                Ok(Record::from_string(&line).handle_err(|_| {
                    eprintln!("failed parsing record from '{line}'");
                })?)
            })
            .collect::<HandledResult<Vec<Record>>>()?;
        records.sort_by_key(|record| record.timestamp);

        Ok(records)
    }

    /// Create a String from a Record.
    pub fn as_string(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}",
            self.timestamp.format("%Y-%m-%dT%H:%M:%S.%f"),
            self.event,
            self.obj_id,
            match self.comment {
                Some(ref comment) => comment,
                None => "",
            },
        )
    }

    /// Create a Record from a &str.
    pub fn from_string(record: &str) -> HandledResult<Self> {
        let mut fields = record.split('\t');
        let Some(timestamp) = fields.next() else {
            eprintln!("missing timestamp field");
            return handled_error();
        };
        let Some(event) = fields.next() else {
            eprintln!("missing event field");
            return handled_error();
        };
        let Some(obj_id) = fields.next() else {
            eprintln!("missing obj_id field");
            return handled_error();
        };
        let mut comment = String::from(fields.next().unwrap_or(""));
        while let Some(remainder) = fields.next() {
            comment.push_str("\t");
            comment.push_str(remainder);
        }

        let timestamp = NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S.%f")
            .handle_err(|e| eprintln!("failed to parse timestamp: '{e}'"))?;

        Ok(Self {
            timestamp,
            event: Event::try_from(event)?,
            obj_id: obj_id.to_string(),
            comment: Some(comment.to_string()),
        })
    }
}

/// All possible events that can be represented in the statefile.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// Resource is managed.
    Manage,
    /// Resource is not managed.
    Unmanage,
    /// Host was activated and can host resources.
    Activate,
    /// Host was deactivated and no longer can host resources.
    Deactivate,
    /// Host was fenced.
    Fence,
    /// Host had a reboot operation initiated.
    FenceReset,
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "{}",
            match self {
                Self::Manage => "manage",
                Self::Unmanage => "unmanage",
                Self::Activate => "activate",
                Self::Deactivate => "deactivate",
                Self::Fence => "fence",
                Self::FenceReset => "reset",
            }
        )
    }
}

impl TryFrom<&str> for Event {
    type Error = HandledError;
    fn try_from(val: &str) -> Result<Self, Self::Error> {
        Ok(match val {
            "manage" => Self::Manage,
            "unmanage" => Self::Unmanage,
            "activate" => Self::Activate,
            "deactivate" => Self::Deactivate,
            "fence" => Self::Fence,
            "reset" => Self::FenceReset,
            _ => {
                eprintln!("failed to parse '{val}' as Event");
                return handled_error();
            }
        })
    }
}
