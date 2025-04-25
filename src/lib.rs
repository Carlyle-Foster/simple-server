#![allow(nonstandard_style)]
#![allow(unused_braces)]
#![allow(unused_parens)]

pub mod http;
pub mod smithy;
pub mod helpers;
// pub mod websocket;
pub mod TLS;
pub mod TLS2;

use std::collections::HashMap;
use std::io::{self};
use core::str;
use std::io::{Write, ErrorKind};
use std::net::SocketAddr;
use std::io::Read;
use std::fs::{self};
use std::sync::Arc;

use helpers::{Read2, Write2};
use mio::event::Event;
use TLS::TLStream;

use mio::net::TcpListener;
use mio::{Events, Interest, Poll, Token};

use rustls::ServerConfig;

use camino::{Utf8Path, Utf8PathBuf};
use TLS2::TLServer2;

const SERVER: Token = Token((!0));

#[derive(Clone)]
pub struct Vfs(HashMap<Utf8PathBuf, V_file>);

impl Vfs {
    pub fn get(&mut self, path: &Utf8Path) -> Option<&V_file> {
        self.check_cache(path);
        self.0.get(path)
    }

    pub fn get_mut(&mut self, path: &Utf8Path) -> Option<&mut V_file> {
        self.check_cache(path);
        self.0.get_mut(path)
    }

    fn check_cache(&mut self, path: &Utf8Path) {
        if !self.0.contains_key(path) {
            if let Ok(data) = fs::read(path) {
                self.0.insert(path.into(), V_file { data });
            }
        }
    }

    pub fn get_size(&mut self, path: &Utf8Path) -> Option<usize> {
        self.get(path).map(|file| file.data.len())
    }

    fn write2client(&mut self, client: &mut Client, server: &mut TLServer2) -> io::Result<()> {
        let payload = match self.get(&client.delivery) {
            Some(file) => &file.data,
            None => &Vec::new(),
        };
        let mut buffer = &client.envelope;
        let mut bytes_writ = buffer.len() + payload.len() - client.bytes_needed;
        while client.bytes_needed > 0 {
            if (buffer == &client.envelope) && (bytes_writ >= buffer.len()) {
                bytes_writ -= buffer.len();
                buffer = payload;
            };
            let (bytes, error) = server.write_to_client(client.id, &buffer[bytes_writ..]);
            bytes_writ += bytes as usize;
            client.bytes_needed -= bytes as usize;
            match error {
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
                Ok(()) => {},
            }
        };
        //TODO: this is very hacky, i'm using Unsupported to signal that the websocket handshake is complete
        if client.protocol == Protocol::WEBSOCKET && client.bytes_needed == 0 {
            return Err(ErrorKind::Unsupported.into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct V_file {
    data: Vec<u8>,
}
            
// pub struct TLServer {
//     config: Arc<ServerConfig>,
//     listener: TcpListener,
//     poll: Poll,
// }

// impl TLServer {
//     pub fn new(address: SocketAddr, config: ServerConfig) -> Self {
//         let poll = Poll::new().unwrap();
//         let registry = poll.registry();
//         let mut listener = TcpListener::bind(address).unwrap();

//         registry.register(&mut listener, SERVER, Interest::READABLE | Interest::WRITABLE).unwrap();

//         Self {
//             config: Arc::new(config),
//             listener,
//             poll,
//         }
//     }

//     fn poll(&mut self, events: &mut Events) {
//         match self.poll.poll(events, None) {
//             Ok(_) => {},
//             Err(ref e) if e.kind() == ErrorKind::Interrupted => {},
//             Err(e) => panic!("{e}"),
//         }
//     }

//     fn serve<'buf>(&mut self, file_system: &mut Vfs, connections: &'buf mut ClientManifest) -> io::Result<(&'buf [u8], Token)> {
//         let mut events: Events = Events::with_capacity(128);
//         loop {
//             self.poll(&mut events);
//             for event in events.iter() {
//                 match event.token() {
//                     SERVER => {
//                         match self.listener.accept() {
//                             Ok((connection, _)) => {
//                                 let stream = TLStream::new(connection, self.config.clone());
//                                 connections.insert(stream, Protocol::HTTP, self.poll.registry())?;
//                             }
//                             Err(e) if e.kind() == ErrorKind::WouldBlock => {},
//                             Err(e) => {
//                                 println!("TCPSERVER: connection refused due to error accepting: {:?}", e);
//                             },
//                         }
//                     }
//                     token => {
//                         match serve_client(file_system, event, connections) {
//                             Ok(0) => {},
//                             Ok(_) => {
//                                 let client = connections.get(event.token()).unwrap();
//                                 return Ok((&client.buffer[..], token))
//                             },
//                             Err(e) if e.kind() == ErrorKind::Unsupported => {
//                                 let client = connections.get(event.token()).unwrap();
//                                 return Ok((&client.buffer[..0], token))
//                             },
//                             Err(e) => {
//                                 println!("TCP: dropped client {} on account of error {e}", token.0);
//                                 connections.remove(token);
//                             }
//                         };
//                     }
//                 }
//             }
//         }
//     }

//     fn dispatch_delivery(&mut self, head: Vec<u8>, body: Utf8PathBuf, file_system: &mut Vfs, connection: &mut Client) -> io::Result<()> {
//         connection.envelope = head;
//         connection.delivery = body;
//         connection.bytes_needed = connection.envelope.len();
//         if let Some(body_size) = file_system.get_size(&connection.delivery) {
//             connection.bytes_needed += body_size;
//         }

//         println!("transmitting {:#?} of size {} to client {}", connection.delivery, connection.bytes_needed, connection.token.0);

//         file_system.write2client(connection)
//     }
// }

// fn serve_client(file_system: &mut Vfs, event: &Event, connections: &mut ClientManifest) -> io::Result<usize> {
//     let token = event.token();
//     if let Some(client) = connections.get(token) {
//         client.check_handshake();
//         let buffer = &mut client.buffer;
//         if event.is_readable() {
//             println!("buffer.len() = {}", buffer.len());
//             let mut bytes_read = 0;
//             loop {
//                 let (bytes, error) = client.stream.read2(&mut buffer[bytes_read..]);
//                 bytes_read += bytes;
//                 match error {
//                     Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
//                     Err(e) => return Err(e),
//                     _ => {},
//                 }
//                 if bytes_read >= buffer.len() { buffer.resize(buffer.len() + 4096, 0); }
//             }
//             if bytes_read > 0 {
//                 client.check_handshake();
//                 return Ok(bytes_read);
//             }
//         }
//         if event.is_writable() {
//             if client.bytes_needed > 0 {
//                 match file_system.write2client(client) {
//                     Ok(()) => {},
//                     Err(e) => return Err(e)
//                 }
//             }
//             else {
//                 match client.stream.flush2() {
//                     Ok(()) => {},
//                     Err(ref e) if e.kind() == ErrorKind::WouldBlock => {},
//                     Err(e) => return Err(e),
//                 } 
//             }
//         }
//         client.check_handshake();
//     }
//     else {
//         println!("TCPSERVER: received invalid ID token {}", token.0);
//     };

//     Ok(0)
// }

#[derive(Debug)]
#[derive(Clone, Copy)]
#[derive(PartialEq)]
pub enum Protocol {
    HTTP,
    WEBSOCKET,
}

pub struct Client {
    pub id: u64,
    pub envelope: Vec<u8>,
    pub delivery: Utf8PathBuf,
    pub bytes_needed: usize,
    pub buffer: Vec<u8>,
    pub buffer_len: usize,
    pub protocol: Protocol,
}

impl Client {
    fn new(id: u64, protocol: Protocol) -> Self {
        Self {
            id,
            envelope: Vec::new(),
            delivery: "".into(),
            bytes_needed: 0,
            buffer: vec![0; 4096], //TODO: maybe this should be less aligned?
            buffer_len: 0,
            protocol,
        }
    }
}
