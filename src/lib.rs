#![allow(nonstandard_style)]
#![allow(unused_braces)]

pub mod http;
pub mod smithy;
pub mod helpers;

use std::io;
use core::str;
use std::io::{Write, ErrorKind};
use std::net::SocketAddr;
use std::io::Read;
use std::fs::{self};
use std::error::Error;
use std::sync::Arc;
use std::time::Instant;

use http::*;
use smithy::{HttpSmith, HttpSmithText};

use mio::net::{TcpListener, TcpStream};
use mio::{Poll, Events, Interest, Token};

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};

type Handler = dyn FnMut(Request) -> Response;

pub struct EncryptedStream {
    pub stream: TcpStream,
    pub tls_layer: ServerConnection,
}

impl Write for EncryptedStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut writer = self.tls_layer.writer();
        writer.write(buf)?;
        match self.tls_layer.complete_io(&mut self.stream) {
            Ok((bytes_written, _)) => {
                Ok(bytes_written)
            },
            Err(e) => Err(e),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        let mut writer = self.tls_layer.writer();
        writer.flush()?;
        self.tls_layer.complete_io(&mut self.stream)?;
        Ok(())
    }
}

impl Read for EncryptedStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.tls_layer.complete_io(&mut self.stream)?;
        let mut reader = self.tls_layer.reader();
        reader.read(buf)
    }
}

pub struct Server<'a> {
    address: SocketAddr,
    http: HttpServer<'a>,
}

impl<'a> Server<'a> {
    pub fn new(address_str: &str) -> Server {
        Server {
            address: address_str.parse().unwrap(),
            http: HttpServer {
                services: Vec::with_capacity(4),
                not_found: Vec::new(),
            }
        }
    }
    pub fn add_service(&mut self, path: &str, method: Method, handler: &'a mut Handler ) {
        let service = Service::new(path, method, handler);
        self.http.services.push(service);
    }
    pub fn add_404_page(&mut self, path: &str) {
        self.http.not_found = fs::read(path).expect("custom 404 page should exist at the specified path");
    }
    pub fn serve(&mut self) -> Result<(), Box<dyn Error>> {
        println!("serving HTTP on (http://{}:{})", self.address.ip(), self.address.port());
        self.generic_serve(&|stream: TcpStream| stream)
    }
    pub fn serve_with_tls(&mut self, domain_cert: Vec<CertificateDer<'static>>, private_key: PrivateKeyDer<'static>) -> Result<(), Box<dyn Error>> {
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(domain_cert, private_key)
            .unwrap();
        let config = Arc::new(config);
        
        let factory = |stream: TcpStream| -> EncryptedStream {
            let mut tls_layer =  ServerConnection::new(config.clone()).unwrap();
            tls_layer.set_buffer_limit(Some(64 * 1024 * 1024));
            EncryptedStream {
                stream,
                tls_layer,
            }
        };
        println!("serving HTTPS on (https://{}:{})", self.address.ip(), self.address.port());
        self.generic_serve(&factory)
    }
    pub fn generic_serve<Client: Read + Write>(&mut self, factory: &dyn Fn(TcpStream) -> Client) -> Result<(), Box<dyn Error>>  {
        const SERVER: Token = Token(!0);
        let mut poll = Poll::new()?;
        let mut events = Events::with_capacity(128);
        let mut connections = clientManifest::<Client>::new(128);
        let mut server = TcpListener::bind(self.address).expect("this port should be available");
        let mut read_buffer = [0; 4096];
        let smith = HttpSmithText{};

        poll.registry()
            .register(&mut server, SERVER, Interest::READABLE)?;
        loop {
            if let Err(err) = poll.poll(&mut events, None) {
                if err.kind() == ErrorKind::Interrupted {
                    continue;
                }
                else { panic!("ERROR: {err}") }
            }
            for event in events.iter() {
                //println!("{:#?}", event);
                match event.token() {
                    SERVER => {
                        match server.accept() {
                            Ok((mut connection, _)) => {
                                let token = Token(connections.reservation_for_1());
                                poll.registry().register(&mut connection, token, Interest::READABLE | Interest::WRITABLE)?;
                                connections.insert(factory(connection));
                            }
                            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {},
                            e => {
                                println!("connection refused due to error accepting: {:?}", e);
                            },
                        }
                    }
                    token => {
                        let start_time = Instant::now();
                        let connection = match connections.get(token) {
                            Some(token) => token,
                            None => continue,
                        };
                        //println!("check 1");
                        match connection.read(&mut read_buffer) {
                            Ok(bytes_read) => {
                                let data = &read_buffer[..bytes_read];
                                //print!("{}\r\nRequest Length: {} bytes", request_string, request_string.len());
                                let request = match smith.deserialize(data) {
                                    Some(request) => request,
                                    None => break,
                                };
                                println!("{:#?}", request);
                                let response = self.http.handle_request(request);
                                match connection.write(&smith.serialize(&response)) {
                                    Ok(_) => {},
                                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => {},
                                    Err(e) => Err(e).unwrap()
                                }
                                println!("time: {:?}", start_time.elapsed());
                            },
                            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {},
                            Err(e) => {
                                connections.remove(token);
                                println!("client dropped due to error: {:?}", e);
                            },
                        }
                    }
                }
            }

        }
    }
}

pub struct clientManifest<Client: Read + Write> {
    contents: Vec<Option<Client>>,
    vacancies: Vec<usize>,
}

//TODO: push panics if the vector grows too large, is this a problem?
impl<Client: Read + Write> clientManifest<Client> {
    pub fn new(capacity: usize) -> Self {
        Self {
            contents: Vec::<Option<Client>>::with_capacity(capacity),
            vacancies: Vec::<usize>::with_capacity(capacity),
        }
    }
    pub fn reservation_for_1(&mut self) -> usize {
        match self.vacancies.pop() {
            Some(vacancy) => vacancy,
            None => {
                self.contents.len()
            },
        }
    }
    pub fn insert(&mut self, connection: Client) {
        let c = Some(connection);
        let vacancy = self.reservation_for_1();
        if vacancy == self.contents.len() {
            self.contents.push(c);
        }
        else {
            self.contents[vacancy] = c;
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
        self.contents[t.0] = None;
        self.vacancies.push(t.0);
    }
}

