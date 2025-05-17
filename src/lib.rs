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
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use helpers::reader_to_writer;
use http::HttpServer;
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Token};

use camino::{Utf8Path, Utf8PathBuf};
use rustls::ServerConfig;
use smithy::{HttpSmith, ParseError};
use TLS::TLStream;
use TLS2::ClientID;

const SERVER: Token = Token((!0));

pub struct Server {
    pub clients: HashMap<ClientID, Client>,
    pub http: HttpServer,

    pub config: Arc<ServerConfig>,
    pub listener: TcpListener,
    pub poll: Poll,
    pub last_refresh: Instant,
    pub last_push: SystemTime,
}

impl Server {
    pub fn new(address: SocketAddr, config: ServerConfig) -> Self {
        let poll = Poll::new().unwrap();
        let registry = poll.registry();
        let mut listener = TcpListener::bind(address).unwrap();

        registry.register(&mut listener, SERVER, Interest::READABLE | Interest::WRITABLE).unwrap();

        println!("HTTPSERVER: initializing server on (https://{}:{})", address.ip(), address.port());

        Self { 
            clients: HashMap::with_capacity(1028),
            http: HttpServer::new(),

            config: Arc::new(config),
            listener,
            poll,
            last_refresh: Instant::now(),
            last_push: SystemTime::now(),
        }
    }
    pub fn serve(&mut self) {
        let mut events = Events::with_capacity(64);
        loop {
            match self.poll.poll(&mut events, Some(Duration::from_millis(400))) {
                Ok(_) => {},
                Err(ref e) if e.kind() == ErrorKind::Interrupted => {},
                Err(e) => panic!("{e}"),
            }
            for event in events.iter() {
                match event.token() {
                    SERVER => {
                        match self.listener.accept() {
                            Ok((client, _)) => {
                                self.register(client).unwrap();
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                            Err(e) => {
                                println!("TLServer: connection refused due to error accepting: {:?}", e);
                            },
                        }
                    }
                    client => {
                        let id = client.0 as ClientID;
                        let client = self.clients.get_mut(&id).unwrap();
                        let stream = &mut client.stream;
                        if stream.tls.is_handshaking() {
                            match client.stream.handshake() {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) if e.kind() == ErrorKind::Interrupted => {},
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when handshaking: {e}");
                                    self.drop_client(id);
                                },
                            };
                            continue
                        }
                        if event.is_writable() {
                            match stream.flush() {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) if e.kind() == ErrorKind::ConnectionAborted => {
                                    self.drop_client(id);
                                    break
                                },
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when flushing: {e}");
                                    self.drop_client(id);
                                    break
                                },
                            } 
                            match reader_to_writer(&mut client.delivery, stream) {
                                Ok(()) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) => {
                                    println!("HTTP_SERVER: dropped client on account of error when writing: {e}");
                                    self.drop_client(id);
                                    break
                                }
                            };
                        }
                        if event.is_readable() {
                            match reader_to_writer(stream, &mut client.buf) {
                                Ok(()) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) => {
                                    println!("HTTP_SERVER: dropped client on account of error when reading: {e}");
                                    self.drop_client(id);
                                    continue
                                }
                            };
                            if !client.buf.has_read() { continue }
                            let story = client.buf.the_story_so_far();
                            match self.http.smith.deserialize(story) {
                                Ok((request, rest)) => {
                                    client.buf.data.copy_within(rest.., 0);
                                    client.buf.data.resize(client.buf.data.len() - rest, 0);
                                    client.buf.read = client.buf.data.len();
                                    client.buf.prev_read = client.buf.read;

                                    let response = self.http.handle_request(request);
                                    let (header, body_path) = self.http.smith.serialize(&response);
                                    //TODO: this panics if u haven't set a 404 page
                                    println!("body_path = {body_path}");
                                    client.delivery = Package {
                                        head: header,
                                        body: self.http.file_system.get(&body_path).unwrap().clone(),
                                        writ: 0,
                                    };
                                    match reader_to_writer(&mut client.delivery, stream) {
                                        Ok(()) => {},
                                        Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                        Err(e) => {
                                            println!("HTTP_SERVER: dropped client on account of error when writing: {e}");
                                            self.drop_client(id);
                                            break
                                        }
                                    };
                                },
                                Err(ParseError::Incomplete) => println!("HTTP_SERVER: request incomplete at size {}", story.len()),
                                Err(e) => {
                                    println!("HTTP_SERVER: dropped client on account of error when parsing request: {e}");
                                    self.drop_client(id);
                                    break
                                }

                            }
                        }
                    }
                }
            }
            if self.last_refresh.elapsed() > Duration::from_millis(800) {
                let changelog = self.http.file_system.client_dir.join(".changelog");
                match fs::metadata(&changelog) {
                    Ok(md) => {
                        let last_mod = md.modified().unwrap();
                        if last_mod > self.last_push {
                            let changes = fs::read_to_string(&changelog).unwrap();
                            for line in changes.split("\n\n") {
                                if let Some(path) = line.strip_prefix("DELETE\t") {
                                    self.http.file_system.remove(path.into()).unwrap();
                                }
                                else {
                                    let path: &Utf8Path = line.into();
                                    self.http.file_system.sync_with_file_system(path).unwrap();
                                }
                            } 
                            println!("got ya");
                        }
                        println!("attempting to remove {changelog}");
                        fs::remove_file(changelog).unwrap();
                    }
                    Err(e) if e.kind() == ErrorKind::NotFound => { println!("no changelog ")},
                    Err(e) => println!("HEARTBEAT: failed to open '.changlelog' because of Error: {e}"),
                }
                self.last_refresh = Instant::now();
            }
        }
    }
    pub fn drop_client(&mut self, id: ClientID) {
        let client = self.clients.get_mut(&id).unwrap();
        
        let registry = self.poll.registry();
        registry.deregister(&mut client.stream).unwrap();

        self.clients.remove(&id).unwrap();
    }
    fn register(&mut self, client: TcpStream) -> io::Result<ClientID> {
        let registry = self.poll.registry();
        let mut stream = TLStream::new(client, self.config.clone());
        let mut id = fastrand::usize(..);
        while self.clients.contains_key(&id) {
            id = fastrand::usize(..);
        }
        let token = Token(id as usize);
        let interests = Interest::READABLE | Interest::WRITABLE;
        registry.register(&mut stream, token, interests)?;

        let client = Client::new(id, Protocol::HTTP, stream);
        self.clients.insert(id, client);

        return Ok(id)
    }
}


#[derive(Clone)]
pub struct Vfs {
    files: HashMap<Utf8PathBuf, Rc<V_file>>,
    client_dir: Utf8PathBuf,
}

impl Vfs {
    pub fn get(&self, path: &Utf8Path) -> Option<Rc<V_file>> {
        println!("attempted to get {path}, result = {}", self.files.get(path).is_some());
        self.files.get(path).cloned()
    }

    pub fn remove(&mut self, path: &Utf8Path) -> Option<Rc<V_file>> {
        self.files.remove(path)
    }
    
    fn sync_with_file_system(&mut self, path: &Utf8Path) -> io::Result<()> {
        let data= fs::read(self.client_dir.join(path))?;
        println!("FILE_SYSTEM: new pair with key = {path}, value from {}", self.client_dir.join(path));
        self.files.insert(path.into(), V_file { data }.into());

        Ok(())
    }

    pub fn get_size(&mut self, path: &Utf8Path) -> Option<usize> {
        self.get(path).map(|file| file.data.len())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Package {
    head: Vec<u8>,
    body: Rc<V_file>,
    writ: usize,
}

impl Read for Package {
    fn read(&mut self, mut buf: &mut [u8]) -> io::Result<usize> {
        let writ = self.writ;
        let source = 
            if writ < self.head.len() {
                &self.head[writ..]
            }
            else {
                &Rc::as_ref(&self.body).data[writ - self.head.len()..]
            };
        let read = buf.write(source)?;
        
        self.writ += read;
        println!("wrote {read} bytes");

        Ok(read)
    }
}

#[derive(Debug, Clone, Default)]
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

pub struct Buffer {
    pub data: Vec<u8>,
    pub read: usize,
    pub prev_read: usize,
}

impl Buffer {
    pub fn with_capacity(cap: usize) -> Self {
        Self { data: Vec::with_capacity(cap), read: 0, prev_read: 0 }
    }
    pub fn has_read(&self) -> bool { self.prev_read < self.read }
    pub fn the_story_so_far(&self) -> &[u8] {
        &self.data[..self.read]
    }
}

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.prev_read = self.read;

        let writ = self.data.write(&buf[self.read..])?;
        self.read += writ;
        //TODO: limit the growth of the buffer

        Ok(writ)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}


pub struct Client {
    pub id: ClientID,
    pub stream: TLStream,
    pub delivery: Package,
    pub buf: Buffer,
    pub protocol: Protocol,
}

impl Client {
    fn new(id: ClientID, protocol: Protocol, stream: TLStream) -> Self {
        Self {
            id,
            stream,
            delivery: Package::default(),
            buf: Buffer::with_capacity(4096), //TODO: maybe this should be less aligned?
            protocol,
        }
    }
}
