use super::*;

use std::sync::Arc;

use mio::net::{TcpListener, TcpStream};
use mio::{Poll, Events, Interest, Token};

//use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{AddrParseError, CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};


pub struct Client {
    pub tcp: TcpStream,
    pub tls: Option<ServerConnection>,
}

impl Client {
    pub fn new(tcp: TcpStream) -> Self {
        Client {
            tcp,
            tls: None,
        }
    }
    pub fn new_with_tls(server: &Server, tcp: TcpStream) -> Self {
        let config = match &server.tls_config {
            Some(config) => config,
            None => panic!("invalid TLS config"),
        };
        let mut tls = ServerConnection::new(config.clone()).unwrap();
        tls.set_buffer_limit(Some(64*1024*1024));
        Client {
            tcp,
            tls: Some(tls),
        }
    }
    pub fn send(&mut self, data: &Vec<u8>) {
        match str::from_utf8(data) {
            Ok(text) => println!("{text}"),
            Err(_) => {},
        };
        match self.tcp.write_all(data) {
            Ok(_) => println!("sent response"),
            Err(e) => {
                println!("failed to write to TCP stream due to error {}, closing connection", e);
            },
        };
    }
    pub fn send_with_tls(&mut self, data: &Vec<u8>) {
        // match str::from_utf8(data) {
        //     Ok(text) => println!("{text}"),
        //     Err(_) => {},
        // };
        let tls = match &mut self.tls {
            Some(tls) => tls,
            None => panic!("expected TLS, connection"),
        };
        match tls.writer().write_all(data) {
            Ok(_) => println!("sent response"),
            Err(e) => {
                println!("failed to write to TCP stream due to error: ({e}), closing connection");
            },
        };
        match tls.complete_io(&mut self.tcp) {
            Ok(_) => {},
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {},
            Err(e) => panic!("{e}"),
        };
    }
}