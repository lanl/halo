// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

pub mod cluster;
pub mod commands;
pub mod config;
pub mod halo_capnp;
pub mod host;
pub mod manager;
pub mod remote;
pub mod resource;
pub mod state;
pub mod test_env;
pub mod tls;

/// A `HandledError` represents an error that has already been handled. When you call a function
/// that returns a `HandledError` or `HandledResult`, you don't need to do anything with that error,
/// other than just be aware that it happened, and return it on to your caller.
///
/// `main()` has a special responsibility: since its "caller" is, in a certain sense, the operating
/// system, `main()` must return a nonzero exit status when it gets a `HandledError`.
///
/// The primary way to construct a `HandledError` is with the `handle_err()` function, which turns a
/// generic error into a `HandledError`, and also runs some caller-provided code to handle the
/// error. That provided code would normally do something like report the error to stderr.
///
/// A `HandledError` inentionally has no data about what the specific error was; the process of
/// handling the error "consumes" that information, and it is no longer needed as the error was
/// already appropriately handled.
#[derive(Debug, PartialEq)]
pub struct HandledError {}

pub type HandledResult<T> = std::result::Result<T, HandledError>;

pub fn handled_error<T>() -> HandledResult<T> {
    HandledResult::Err(HandledError {})
}

pub trait Handle<T, F> {
    fn handle_err(self, handler: F) -> HandledResult<T>;
}

impl<T, E, F: FnOnce(E)> Handle<T, F> for std::result::Result<T, E> {
    /// Handle an error by running the provided `handler` code, giving it the error.
    ///
    /// Then, return a `HandledResult`, so that transitive callers of this function know that they
    /// do not need to do anything further to handle the error.
    fn handle_err(self, handler: F) -> HandledResult<T> {
        self.map_err(|e| {
            handler(e);
            HandledError {}
        })
    }
}

/// Gets the port that the remote server should be listening on.
pub fn remote_port() -> u16 {
    match std::env::var("HALO_PORT") {
        Ok(port) => port
            .parse::<u16>()
            .expect("HALO_PORT must be a valid port number"),
        Err(_) => 8000,
    }
}

pub fn default_socket() -> String {
    match std::env::var("HALO_SOCKET") {
        Ok(sock) => sock,
        Err(_) => "/var/run/halo.socket".to_string(),
    }
}

pub fn default_config_path() -> String {
    match std::env::var("HALO_CONFIG") {
        Ok(conf) => conf,
        Err(_) => "/etc/halo/halo.conf".to_string(),
    }
}

pub fn default_statefile_path() -> String {
    match std::env::var("HALO_STATEFILE") {
        Ok(statefile) => statefile,
        Err(_) => "/etc/halo/halo.state".to_string(),
    }
}

pub fn default_server_cert() -> String {
    match std::env::var("HALO_SERVER_CERT") {
        Ok(cert) => cert,
        Err(_) => "/etc/halo/server.crt".to_string(),
    }
}

pub fn default_server_key() -> String {
    match std::env::var("HALO_SERVER_KEY") {
        Ok(key) => key,
        Err(_) => "/etc/halo/server.key".to_string(),
    }
}

pub fn default_client_cert() -> String {
    match std::env::var("HALO_CLIENT_CERT") {
        Ok(cert) => cert,
        Err(_) => "/etc/halo/client.crt".to_string(),
    }
}

pub fn default_client_key() -> String {
    match std::env::var("HALO_CLIENT_KEY") {
        Ok(key) => key,
        Err(_) => "/etc/halo/client.key".to_string(),
    }
}

pub fn default_ca_cert() -> String {
    match std::env::var("HALO_CA_CERT") {
        Ok(cert) => cert,
        Err(_) => "/etc/halo/ca.crt".to_string(),
    }
}

pub fn default_network() -> String {
    match std::env::var("HALO_NET") {
        Ok(net) => net,
        Err(_) => "192.168.1.0/24".to_string(),
    }
}
