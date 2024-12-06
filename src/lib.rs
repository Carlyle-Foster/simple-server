#![allow(nonstandard_style)]
#![allow(unused_braces)]
#![allow(unused_parens)]

pub mod http;
pub mod smithy;
pub mod helpers;
pub mod websocket;
pub mod TLS;

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
use mio::{Events, Interest, Poll, Registry, Token};
#[cfg(target_family = "unix")]
use mio::unix::SourceFd;

use rustls::ServerConfig;

use camino::{Utf8Path, Utf8PathBuf};

const STDIN: i32 = 0;

const SERVER: Token = Token((!0));
const ADMIN: Token = Token((!0) - 1);

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
        match self.get(path) {
            Some(file) => Some(file.data.len()),
            _ => None,
        }
    }

    fn write2client(&mut self, connection: &mut Client) -> io::Result<()> {
        let payload = match self.get(&connection.delivery) {
            Some(file) => &file.data,
            None => &Vec::new(),
        };
        let mut buffer = &connection.envelope;
        let mut bytes_writ = buffer.len() + payload.len() - connection.bytes_needed;
        while connection.bytes_needed > 0 {
            if (buffer == &connection.envelope) && (bytes_writ >= buffer.len()) {
                bytes_writ -= buffer.len();
                buffer = payload;
            };
            let (bytes, error) = connection.stream.write2(&buffer[bytes_writ..]);
            bytes_writ += bytes;
            connection.bytes_needed -= bytes;
            match error {
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
                Ok(()) => {},
            }
        };
        Ok(())
    }
}

#[derive(Clone)]
pub struct V_file {
    data: Vec<u8>,
}
            
pub struct TLServer {
    config: Arc<ServerConfig>,
    listener: TcpListener,
    poll: Poll,
}

impl TLServer {
    pub fn new(address: SocketAddr, config: ServerConfig) -> Self {
        let poll = Poll::new().unwrap();
        let registry = poll.registry();
        let mut listener = TcpListener::bind(address).unwrap();

        registry.register(&mut listener, SERVER, Interest::READABLE | Interest::WRITABLE).unwrap();
        registry.register(&mut SourceFd(&STDIN), ADMIN, Interest::READABLE).unwrap();

        Self {
            config: Arc::new(config),
            listener,
            poll,
        }
    }

    fn poll(&mut self, events: &mut Events) {
        match self.poll.poll(events, None) {
            Ok(_) => {},
            Err(ref e) if e.kind() == ErrorKind::Interrupted => {},
            Err(e) => Err(e).unwrap(),
        }
    }

    fn serve<'buf>(&mut self, buffer: &'buf mut Vec<u8>, file_system: &mut Vfs, connections: &mut ClientManifest) -> io::Result<(&'buf [u8], Token)> {
        let mut events: Events = Events::with_capacity(128);
        loop {
            self.poll(&mut events);
            for event in events.iter() {
                match event.token() {
                    ADMIN => return Ok((&buffer[..0], ADMIN)),
                    SERVER => {
                        match self.listener.accept() {
                            Ok((connection, _)) => {
                                let stream = TLStream::new(connection, self.config.clone());
                                connections.insert(stream, self.poll.registry())?;
                            }
                            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {},
                            e => {
                                println!("TCPSERVER: connection refused due to error accepting: {:?}", e);
                            },
                        }
                    }
                    token => {
                        match self.serve_client(buffer, file_system, event, connections) {
                            Ok(0) => {},
                            Ok(bytes_read) => return Ok((&buffer[..bytes_read], token)),
                            Err(e) => {
                                println!("TCP: dropped client {} on account of error {e}", token.0);
                                connections.remove(token);
                            }
                        };
                    }
                }
            }

        }
    }
    fn serve_client(&mut self, buffer: &mut Vec<u8>, file_system: &mut Vfs, event: &Event, connections: &mut ClientManifest) -> io::Result<usize> {
        let token = event.token();
        if let Some(client) = connections.get(token) {
            client.check_handshake();
            if event.is_readable() {
                let mut bytes_read = 0;
                loop {
                    let (bytes, error) = client.stream.read2(&mut buffer[bytes_read..]);
                    bytes_read += bytes;
                    match error {
                        Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) => return Err(e),
                        _ => {},
                    }
                    if bytes_read >= buffer.len() { buffer.resize(buffer.len() + 4096, 0); }
                }
                if bytes_read > 0 {
                    client.check_handshake();
                    return Ok(bytes_read);
                }
            }
            if event.is_writable() {
                if client.bytes_needed > 0 {
                    match file_system.write2client(client) {
                        Ok(()) => {},
                        Err(e) => return Err(e)
                    }
                }
                else {
                    match client.stream.flush2() {
                        Ok(()) => {},
                        Err(ref e) if e.kind() == ErrorKind::WouldBlock => {},
                        Err(e) => return Err(e),
                    } 
                }
            }
            client.check_handshake();
        }
        else {
            println!("TCPSERVER: received invalid ID token {}", token.0);
        };

        Ok(0)
    }

    fn dispatch_delivery(&mut self, head: Vec<u8>, body: Utf8PathBuf, file_system: &mut Vfs, connection: &mut Client) -> io::Result<()> {
        connection.envelope = head;
        connection.delivery = body;
        connection.bytes_needed = connection.envelope.len();
        if let Some(body_size) = file_system.get_size(&connection.delivery) {
            connection.bytes_needed += body_size;
        }

        println!("transmitting {:#?} to client {}", connection.delivery, connection.token.0);

        file_system.write2client(connection)
    }
}

#[derive(Debug)]
pub enum Protocol {
    HTTP,
    WEBSOCKET,
}

pub struct Client {
    pub token: Token,
    pub stream: TLStream,
    pub envelope: Vec<u8>,
    pub delivery: Utf8PathBuf,
    pub bytes_needed: usize,
    pub was_handshaking: bool,
    pub protocol: Protocol,
}

impl Client {
    fn new(stream: TLStream, token: Token) -> Self {
        Self {
            token,
            stream,
            envelope: Vec::new(),
            delivery: "".into(),
            bytes_needed: 0,
            was_handshaking: true,
            protocol: Protocol::HTTP,
        }
    }

    fn check_handshake(&mut self) {
        let is_handshaking = self.stream.tls.is_handshaking();
        if self.was_handshaking !=  is_handshaking {
            println!("CLIENT {}: handshake completed", self.token.0);
            self.was_handshaking = is_handshaking;
        };
    }
}

pub struct ClientManifest {
    contents: Vec<Option<Client>>,
    vacancies: Vec<usize>,
}

//TODO: push panics if the vector grows too large, is this a problem?
impl ClientManifest {
    pub fn new(capacity: usize) -> Self {
        Self {
            contents: Vec::<Option<Client>>::with_capacity(capacity),
            vacancies: Vec::<usize>::with_capacity(capacity),
        }
    }
    pub fn insert(&mut self, stream: TLStream, registry: &Registry) -> io::Result<()> {
        let vacancy = self.reservation_for_1();
        let token = Token(vacancy);
        let mut client = Client::new(stream, token);

        registry.register(&mut client.stream, token, Interest::READABLE | Interest::WRITABLE)?;
        self.contents[vacancy] = Some(client);

        println!("token = {}", token.0);

        Ok(())
    }
    fn reservation_for_1(&mut self) -> usize {
        match self.vacancies.pop() {
            Some(vacancy) => vacancy,
            None => {
                self.contents.push(None);
                self.contents.len()-1
            }
        }
    }
    pub fn get(&mut self, t: Token) -> Option<&mut Client> {
        match self.contents.get_mut(t.0) {
            Some(c) => match c {
                Some(c) => Some(c),
                None => None,
            },
            None => None,
        }
    }
    pub fn remove(&mut self, t: Token) {
        let client = &mut self.contents[t.0];
        if let Some(client) = client {
            client.stream.tcp.shutdown(std::net::Shutdown::Both).unwrap();
        }
        else { panic!("ERROR: attempted to free non-existant client {}", t.0) }
        *client = None;
        self.vacancies.push(t.0);
    }
}

