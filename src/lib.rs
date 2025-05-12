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
pub mod test_env;
pub mod tls;

use crate::cluster::Cluster;
use std::sync::Mutex;

/// Buffer is an object that can be shared between writers to be written to and readers
/// to be read from.
#[derive(Debug)]
pub struct Buffer {
    data: Mutex<Vec<u8>>,
    read_idx: Mutex<usize>,
    new_data_n: Mutex<usize>,
    is_new_data: std::sync::Condvar,
}

impl Buffer {
    pub fn new() -> Self {
        Buffer {
            data: Mutex::new(Vec::new()),
            read_idx: Mutex::new(0),
            new_data_n: Mutex::new(0),
            is_new_data: std::sync::Condvar::new(),
        }
    }

    /// Write data into this buffer.
    ///
    /// Note that the given buffer must already be big enough to hold write data, or this function
    /// will panic.
    pub fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
        use std::io::Write;
        let mut new_data_n = self
            .new_data_n
            .lock()
            .expect("could not acquire lock on Buffer data");
        let n = match self.data.lock().unwrap().write(buf) {
            Ok(n) => n,
            Err(e) => {
                return std::io::Result::Err(e);
            }
        };
        *new_data_n += n;
        self.is_new_data.notify_one();
        std::io::Result::Ok(n)
    }

    pub fn writeln(&self, buf: &[u8]) -> std::io::Result<usize> {
        self.write(&[buf, "\n".as_bytes()].concat())
    }

    /// Read data into given buffer.
    ///
    /// If the end of the given buffer is reached while reading, only the amount of data that can
    /// fill the given buffer will be read, possibly leaving data in the source buffer.
    pub fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.len() == 0 {
            panic!("given buffer too small to read full line into");
        }
        let mut new_data_n = self
            .new_data_n
            .lock()
            .expect("could not acquire lock on Buffer data");
        while *new_data_n == 0 {
            new_data_n = self
                .is_new_data
                .wait(new_data_n)
                .expect("could not wait on Buffer condvar");
        }
        let data = self.data.lock().unwrap();
        let mut read_idx = self.read_idx.lock().unwrap();
        let leftover_read = {
            let diff: isize = *new_data_n as isize - buf.len() as isize;
            if diff < 0 {
                0 as usize
            } else {
                diff as usize
            }
        };
        let read_slice = &data[*read_idx..(*read_idx + *new_data_n - leftover_read)];
        let mut nread = 0;
        for datum in read_slice.iter() {
            buf[nread] = *datum;
            nread += 1;
        }
        *read_idx += nread;
        *new_data_n = leftover_read;
        Ok(nread)
    }

    /// Reads bytes until a newline (0xA) is reached, appending these bytes including the newline
    /// to the provided buffer.
    ///
    /// Note that the given buffer must already be big enough to hold read data, or this function
    /// will panic.
    pub fn readln(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut charbuf = vec![0u8; 1];
        let outlen = buf.len();
        if outlen == 0 {
            panic!("given buffer too small to read full line into");
        }
        let mut out_idx = 0;
        while charbuf[0] != b'\n' {
            let n = self.read(&mut charbuf).expect("failed to read into buffer");
            buf[out_idx] = charbuf[0];
            out_idx += n;
            if out_idx >= outlen {
                panic!("given buffer too small to read full line into");
            }
        }
        Ok(out_idx)
    }
}

/// LogStream is an abstract object representing a writeable (and in some cases readable) stream.
///
/// Each enum variant represents a concrete type that has its own way of writing (and perhaps
/// reading).
#[derive(Debug)]
pub enum LogStream {
    Stdout(std::io::Stdout),
    Stderr(std::io::Stderr),
    Buffer(Buffer),
}

impl LogStream {
    pub fn new_stdout() -> Self {
        LogStream::Stdout(std::io::stdout())
    }

    pub fn new_stderr() -> Self {
        LogStream::Stderr(std::io::stderr())
    }

    pub fn new_buffer() -> Self {
        LogStream::Buffer(Buffer::new())
    }

    pub fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
        use std::io::Write;
        match self {
            LogStream::Stdout(s) => s.lock().write(buf),
            LogStream::Stderr(s) => s.lock().write(buf),
            LogStream::Buffer(b) => b.write(buf),
        }
    }

    pub fn writeln(&self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            LogStream::Stdout(_) => self.write(&[buf, b"\n"].concat()),
            LogStream::Stderr(_) => self.write(&[buf, b"\n"].concat()),
            LogStream::Buffer(b) => b.writeln(buf),
        }
    }

    pub fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            LogStream::Buffer(b) => b.readln(buf),
            _ => unimplemented!("cannot read from Stdio"),
        }
    }

    pub fn readln(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            LogStream::Buffer(b) => b.readln(buf),
            _ => unimplemented!("cannot read from Stdio"),
        }
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
