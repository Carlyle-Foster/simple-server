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
use std::io::Read;
use std::fs::{self};
use std::net::SocketAddr;

use http::HttpServer;
use mio::Token;

use camino::{Utf8Path, Utf8PathBuf};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use smithy::{HttpSmith, ParseError};
use TLS2::{ServerEvent, TLServer};

const SERVER: Token = Token((!0));

pub struct Server {
    pub clients: HashMap<u64, Client>,
    pub http: HttpServer,
}

impl Server {
    pub fn new() -> Self {
        Self { 
            clients: HashMap::with_capacity(1028),
            http: HttpServer::new(),
        }
    }
    pub fn serve(&mut self, address: SocketAddr, domain_cert: Vec<CertificateDer<'static>>, private_key: PrivateKeyDer<'static>) {
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(domain_cert, private_key)
            .unwrap();
        let mut tls_server = TLServer::new(address, config);

        println!("HTTPSERVER: serving on (https://{}:{})", address.ip(), address.port());

        loop {
            let (id, event) = tls_server.serve().unwrap_or_else(|e| panic!("HTTPSERVER_CRASHED (on account of {e})"));
            println!("{event:?}, id = {id}");
            match event {
                ServerEvent::CLIENT_JOINED => {
                    let newcomer = Client::new(id, Protocol::HTTP);
                    self.clients.insert(id, newcomer);
                },
                ServerEvent::CLIENT_READABLE => {
                    let client = self.clients.get_mut(&id).unwrap();
                    let (bytes_read, err) = tls_server.read_from_client(id, &mut client.buffer);
                    match err {
                        Ok(()) => {},
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                        Err(e) => {
                            tls_server.drop_client(id);
                            self.clients.remove(&id).unwrap();
                            println!("HTTP_SERVER: dropped client on account of error when reading: {e}");
                            continue
                        }
                    };
                    if bytes_read == 0 { continue }
                    match self.http.smith.deserialize(&client.buffer[..bytes_read as usize]) {
                        Ok((request, rest)) => {
                            client.buffer.copy_within(rest as usize.., 0);
                            client.buffer.resize(client.buffer.len() - rest as usize, 0);

                            let response = self.http.handle_request(request);
                            let (header, body_path) = self.http.smith.serialize(&response);
                            //TODO: this panics if u haven't set a 404 page
                            client.bytes_needed = header.len() + self.http.file_system.get_size(&body_path).unwrap();
                            client.envelope = header;
                            client.delivery = body_path;
                            match self.http.file_system.write2client(client, &mut tls_server) {
                                Ok(()) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) => {
                                    tls_server.drop_client(id);
                                    self.clients.remove(&id).unwrap();
                                    println!("HTTP_SERVER: dropped client on account of error when writing: {e}");
                                }
                            };
                        },
                        Err(ParseError::Incomplete) => println!("HTTP_SERVER: received request fragment of size {bytes_read}"),
                        Err(e) => {
                            tls_server.drop_client(id);
                            self.clients.remove(&id).unwrap();
                            println!("HTTP_SERVER: dropped client on account of error when parsing request: {e}");
                        }

                    }
                },
                ServerEvent::CLIENT_WRITABLE => {
                    let client = self.clients.get_mut(&id).unwrap();
                    match self.http.file_system.write2client(client, &mut tls_server) {
                        Ok(()) => {},
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                        Err(e) => {
                            tls_server.drop_client(id);
                            self.clients.remove(&id).unwrap();
                            println!("HTTP_SERVER: dropped client on account of error when writing: {e}");
                        }
                    };
                },
                ServerEvent::CLIENT_LEFT => {
                    self.clients.remove(&id).unwrap();
                },
            }
        };
    }
}

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

    fn write2client(&mut self, client: &mut Client, server: &mut TLServer) -> io::Result<()> {
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
