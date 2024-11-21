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
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use http::*;
use smithy::{HttpSmith, HttpSmithText};

use mio::net::{TcpListener, TcpStream};
use mio::{Poll, Events, Interest, Token};

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};

const SERVER: Token = Token(!0);

pub struct EncryptedStream {
    pub stream: TcpStream,
    pub tls_layer: ServerConnection,
}

impl EncryptedStream {
    fn new(stream: TcpStream, config: &Arc<ServerConfig>) -> EncryptedStream {
        let mut tls_layer =  ServerConnection::new(config.clone()).unwrap();
        tls_layer.set_buffer_limit(Some(64 * 1024 * 1024));
        EncryptedStream {
            stream,
            tls_layer,
        }
    }
}

impl Write for EncryptedStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        //self.tls_layer.complete_io(&mut self.stream)?;
        let mut writer = self.tls_layer.writer();
        let bytes = writer.write(buf)?;
        println!("bytes: {}", bytes);
        match self.tls_layer.complete_io(&mut self.stream) {
            Ok(_) => {
                Ok(bytes)
            },
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => Ok(bytes),
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
        match self.tls_layer.complete_io(&mut self.stream) {
            Ok(_) => {
                let mut reader = self.tls_layer.reader();
                reader.read(buf)
            }
            Err(e) => Err(e),
        }
    }
}

impl HttpServer {
    pub fn new() -> Self {
        Self {
            services: Vec::with_capacity(4),
            not_found: Vec::new(),
        }
    }
    pub fn add_service<I, O>(&mut self, path: &str, method: Method, function: impl FnMut(I) -> O + 'static)
    where
        I: From<Request> + 'static,
        O: Into<Response> + 'static,
    {
        let service = Service::new(path, method, function);
        self.services.push(service);
    }
    pub fn add_404_page(&mut self, path: &str) {
        self.not_found = fs::read(path).expect("custom 404 page should exist at the specified path");
    }
    pub fn serve(&mut self, address: SocketAddr, domain_cert: Vec<CertificateDer<'static>>, private_key: PrivateKeyDer<'static>) {
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(domain_cert, private_key)
            .unwrap();
        
        println!("serving HTTPS on (https://{}:{})", address.ip(), address.port());
        self.generic_serve(address, config);
    }
    pub fn generic_serve(&mut self, address: SocketAddr, config: ServerConfig) {
        let mut read_buffer = [0; 4096];
        let mut io_server = TcpServer::new(address, config);
        let smith = HttpSmithText{};

        loop {
            println!("reading now...");
            match io_server.read(&mut read_buffer) {
                Ok(0) => { continue },
                Ok(bytes_read) => {
                    let start_time = Instant::now();
                    let data = &read_buffer[..bytes_read];
                    //print!("{}\r\nRequest Length: {} bytes", request_string, request_string.len());
                    let request = match smith.deserialize(data) {
                        Some(request) => request,
                        None => continue,
                    };
                    println!("{:#?}", request);
                    let response = self.handle_request(request);
                    println!("writing now...");
                    match io_server.write(&smith.serialize(&response)) {
                        Ok(_) => {},
                        Err(ref e) if e.kind() == ErrorKind::ConnectionAborted => {},
                        Err(e) => Err(e).unwrap()
                    }
                    println!("time: {:?}", start_time.elapsed());
                },
                Err(e) => {
                    Err(e).unwrap()
                },
            }
        }
    }
}
            
pub struct TcpServer {
    poll: Poll,
    events: Events,
    connections: clientManifest,
    config: Arc<ServerConfig>,
    listener: TcpListener,
    active_client: Token,
}

impl TcpServer {
    pub fn new(address: SocketAddr, config: ServerConfig) -> Self {
        let poll = Poll::new().unwrap();
        let mut listener = TcpListener::bind(address).unwrap();

        poll.registry()
            .register(&mut listener, SERVER, Interest::READABLE).unwrap();

        Self {
            poll,
            events: Events::with_capacity(128),
            connections: clientManifest::new(128),
            config: Arc::new(config),
            listener,
            active_client: SERVER,
        }
    }
    fn poll(&mut self) {
        match self.poll.poll(&mut self.events, None) {
            Ok(_) => {},
            Err(ref e) if e.kind() == ErrorKind::Interrupted => {},
            Err(e) => Err(e).unwrap(),
        }
    }
} 

impl Read for TcpServer {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize>  {
        loop {
            //println!("new poll");
            self.poll();
            for event in self.events.iter() {
                //println!("{:#?}", event);
                match event.token() {
                    SERVER => {
                        match self.listener.accept() {
                            Ok((mut connection, _)) => {
                                let token = Token(self.connections.reservation_for_1());
                                self.poll.registry().register(&mut connection, token, Interest::READABLE | Interest::WRITABLE)?;
                                self.connections.insert(EncryptedStream::new(connection, &self.config));
                            }
                            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {},
                            e => {
                                println!("connection refused due to error accepting: {:?}", e);
                            },
                        }
                    }
                    token => {
                        let connection = match self.connections.get(token) {
                            Some(client) => client,
                            None => continue,
                        };
                        self.active_client = token;
                        match connection.read(buffer) {
                            // Ok(0) => {
                            //     self.connections.remove(token);
                            //     println!("read pipe closed");
                            // }
                            Ok(bytes_read) => {
                                println!("read {} bytes", bytes_read);
                                return Ok(bytes_read)
                            },
                            Err(ref e) if e.kind() == ErrorKind::WouldBlock => { println!("{e}"); },
                            Err(e) => {
                                self.connections.remove(token);
                                println!("client dropped due to error: {:?}", e);
                            },
                        }
                    }
                }
            }

        }
    }
}

impl Write for TcpServer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let connection = match self.connections.get(self.active_client) {
            Some(client) => client,
            None => panic!("142658"),
        };
        println!("bytes to be written: {}", buffer.len());
        let mut bytes_written = 0;
        loop {
            match connection.write(buffer) {
                Ok(0) => {
                    println!("connection closed by client");
                    self.connections.remove(self.active_client);
                    return Err(ErrorKind::ConnectionAborted.into());
                }
                Ok(bytes) => {
                    println!("wrote {bytes} bytes");
                    bytes_written += bytes;
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => { println!("sleeping now"); sleep(Duration::from_millis(1)) },
                Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    println!("client dropped due to error when writing: {e}");

                    self.connections.remove(self.active_client);
                    return Err(ErrorKind::ConnectionAborted.into());
                },
            }
            if bytes_written >= buffer.len() {
                //println!("wrote {bytes_written} bytes");
                return Ok(bytes_written)
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        let connection = match self.connections.get(self.active_client) {
            Some(token) => token,
            None => panic!("253790"),
        };
        connection.flush()
    }
}

pub struct clientManifest {
    contents: Vec<Option<EncryptedStream>>,
    vacancies: Vec<usize>,
}

//TODO: push panics if the vector grows too large, is this a problem?
impl clientManifest {
    pub fn new(capacity: usize) -> Self {
        Self {
            contents: Vec::<Option<EncryptedStream>>::with_capacity(capacity),
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
    pub fn insert(&mut self, connection: EncryptedStream) {
        let c = Some(connection);
        let vacancy = self.reservation_for_1();
        if vacancy == self.contents.len() {
            self.contents.push(c);
        }
        else {
            self.contents[vacancy] = c;
        }
    }
    pub fn get(&mut self, t: Token) -> Option<&mut EncryptedStream> {
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

