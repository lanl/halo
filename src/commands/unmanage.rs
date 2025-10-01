// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

//use crate::{commands, resource};
use crate::commands;

#[derive(Args, Debug, Clone)]
pub struct UnmanageArgs {
    /// Resource to unmanage
    #[arg(long)]
    resource: String,
}

pub fn unmanage(_args: &UnmanageArgs) -> commands::Result{
    todo!("Implement logic");
}