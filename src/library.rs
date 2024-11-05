#![allow(nonstandard_style)]
#![allow(unused_braces)]

pub mod implementation;
use implementation::Client;

use core::str;
use std::collections::HashMap;
use std::io::{Write, ErrorKind};
use std::net::SocketAddr;
use std::io::Read;
use std::fs::{self};
use std::path::Path;
use std::path::PathBuf;
use std::error::Error;
use std::sync::Arc;

use mio::net::{TcpListener};
use mio::{Poll, Events, Interest, Token};

//use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig};

type Handler = &'static dyn Fn(Request) -> Response;

pub struct Service {
    path: PathBuf,
    method: Method,
    handler: Handler,
}

impl Service {
    pub fn new(path_str: &str, method: Method, handler: Handler ) -> Self {
        Service {
            path: path_str.parse().unwrap(),
            method,
            handler,
        }
    }
}

pub struct Server {
    address: SocketAddr,
    services: Vec<Service>,
    not_found: Vec<u8>,
    tls_config: Option<Arc<rustls::ServerConfig>>,
}

impl Server {
    pub fn new(address_str: &str) -> Server {
        Server {
            address: address_str.parse().unwrap(),
            services: Vec::with_capacity(4),
            not_found: Vec::new(),
            tls_config: None,
        }
    }
    pub fn add_service(&mut self, service: Service) {
        self.services.push(service);
    }
    pub fn add_404_page(&mut self, path: &Path) {
        self.not_found = fs::read(path).expect("custom 404 page should exist at the specified path");
    }
    pub fn add_certs(&mut self, domain_cert: Vec<CertificateDer<'static>>, private_key: PrivateKeyDer<'static>) {
        self.tls_config = 
            Some(
                Arc::new(
                ServerConfig::builder()
                    .with_no_client_auth()
                    .with_single_cert(domain_cert, private_key)
                    .unwrap()
                    )
                )
    }
    pub fn serve(&self) -> Result<(), Box<dyn Error>>  {
        const SERVER: Token = Token(!0);
        let mut poll = Poll::new()?;
        let mut events = Events::with_capacity(128);
        let mut connections = clientManifest::new(128);
        let mut server = TcpListener::bind(self.address).expect("this port should be available");
        let mut read_buffer = [0; 4096];

        poll.registry()
            .register(&mut server, SERVER, Interest::READABLE)?;
        println!("listening at address {} on port {}", self.address.ip(), self.address.port());
        loop {
            poll.poll(&mut events, None)?;
            for event in events.iter() {
                match event.token() {
                    SERVER => {
                        loop {
                            match server.accept() {
                                Ok((connection, _)) => {
                                    let (connection, token) = connections.insert(Client::new(connection));
                                    poll.registry().register(&mut connection.tcp, token, Interest::READABLE | Interest::WRITABLE)?;
                                }
                                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                                    println!("would block");
                                    break
                                },
                                e => {
                                    println!("ERROR: {:?}", e);
                                    break
                                },
                            }
                        }
                    },
                    token => {
                        loop {
                            let connection = connections.get(token).unwrap();
                            match connection.tcp.read(&mut read_buffer) {
                                Ok(bytes_read) => {
                                    let request_string = str::from_utf8(&read_buffer[0..bytes_read])?;
                                    //print!("{}\r\nRequest Length: {} bytes", request_string, request_string.len());
                                    let mut request = match Request::from_data(request_string) {
                                        Some(request) => request,
                                        None => break,
                                    };
                                    println!("{:#?}", request);
                                    for service in &self.services {
                                        println!("service path: {:?}, request path: {:?}", service.path, request.path);
                                        if request.path.starts_with(&service.path) && (request.method == service.method || service.method == Method::ANY) {
                                            request.path = request.path.strip_prefix(&service.path).unwrap().to_path_buf();
                                            let mut response = (service.handler)(request);
                                            if response.status == Status::NotFound {
                                                response.payload = self.not_found.clone();
                                            }
                                            connection.send(&response.deserialize());
                                            break
                                        }
                                    }
                                    break
                                },
                                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                                    break
                                },
                                e => {
                                    println!("ERROR: {:?}", e);
                                    break
                                },
                            }
                        }
                    },
                }
            }

        }
    }
    pub fn serve_with_tls(&self) -> Result<(), Box<dyn Error>>  {
        const SERVER: Token = Token(!0);
        let mut poll = Poll::new()?;
        let mut events = Events::with_capacity(128);
        let mut connections = clientManifest::new(128);
        let mut server = TcpListener::bind(self.address).expect("this port should be available");
        let mut read_buffer = [0; 4096];

        poll.registry()
            .register(&mut server, SERVER, Interest::READABLE)?;
        println!("listening at address {} on port {}", self.address.ip(), self.address.port());
        loop {
            poll.poll(&mut events, None)?;
            for event in events.iter() {
                //println!("{:#?}", event);
                match event.token() {
                    SERVER => {
                        loop {
                            match server.accept() {
                                Ok((connection, _)) => {
                                    let (connection, token) = connections.insert(Client::new_with_tls(self, connection));
                                    poll.registry().register(&mut connection.tcp, token, Interest::READABLE | Interest::WRITABLE)?;
                                }
                                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                                    println!("would block");
                                    break
                                },
                                e => {
                                    println!("ERROR: {:?}", e);
                                    break
                                },
                            }
                        }
                    },
                    token => {
                        let connection = connections.get(token).unwrap();
                        let tls = match &mut connection.tls {
                            Some(tls) => tls,
                            None => panic!("expected TLS connection"),
                        };
                        match tls.complete_io(&mut connection.tcp) {
                            Ok((_bytes_read, _bytes_written)) => {},
                            Err(ref e) if e.kind() == ErrorKind::InvalidData => {
                                panic!("replaces this panic with a graceful shutdown of the connection: {:?}! 1", e);
                            }
                            Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue,
                            Err(e) => {
                                println!("ERROR: {:?}", e);
                                panic!("replaces this panic with a graceful shutdown of the connection: {:?}! 2", e);
                            }
                        };
                        //println!("check 1");
                        match tls.reader().read(&mut read_buffer) {
                            Ok(bytes_read) => {
                                let request_string = str::from_utf8(&read_buffer[0..bytes_read])?;
                                //print!("{}\r\nRequest Length: {} bytes", request_string, request_string.len());
                                let mut request = match Request::from_data(request_string) {
                                    Some(request) => request,
                                    None => break,
                                };
                                println!("{:#?}", request);
                                for service in &self.services {
                                    //println!("service path: {:?}, request path: {:?}", service.path, request.path);
                                    if request.path.starts_with(&service.path) && (request.method == service.method || service.method == Method::ANY) {
                                        request.path = request.path.strip_prefix(&service.path).unwrap().to_path_buf();
                                        let mut response = (service.handler)(request);
                                        if response.status == Status::NotFound {
                                            response.payload = self.not_found.clone();
                                        }
                                        connection.send_with_tls(&response.deserialize());
                                        break
                                    }
                                }
                                break
                            },
                            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                                break
                            },
                            Err(e) => {
                                println!("ERROR: {:?}", e);
                                break
                            },
                        }
                    }
                }
            }

        }
    }
}

#[derive(Debug)]
#[derive(PartialEq)]
pub enum Method {
    UNINITIALIZED,
    CONNECT,
    DELETE,
    GET,
    HEAD,
    OPTIONS,
    PATCH,
    POST,
    PUT,
    TRACE,
    ANY,
}

impl Method {
    fn from_str(s: &str) -> Option<Method> {
        match s {
            "GET" => return Some(Method::GET),
            "POST" => return Some(Method::POST),
            _ => return None,
        }
    }
}

#[derive(Debug)]
pub enum Version {
    V_1_0,
    V_1_1,
    V_2_0,
    V_3_0,
}


impl Version {
    fn from_str(s: &str) -> Option<Version> {
        println!("{}", s);
        match s {
            "HTTP/1" => return Some(Version::V_1_0),
            "HTTP/1.1" => return Some(Version::V_1_1),
            "HTTP/2" => return Some(Version::V_2_0),
            "HTTP/3" => return Some(Version::V_3_0),
            _ => return None,
        }
    }
    fn to_str(&self) -> &str {
        match self {
            Version::V_1_0 => "HTTP/1",
            Version::V_1_1 => "HTTP/1.1",
            Version::V_2_0 => "HTTP/2",
            Version::V_3_0 => "HTTP/3",
        }
    }
}

#[derive(Debug)]
pub struct Header(String, String);

#[derive(Debug)]
pub struct Request<'event_loop> {
    pub method: Method,
    pub path: PathBuf,
    pub version: Version,
    pub headers: HashMap<&'event_loop str, &'event_loop str>,
}


impl<'event_loop> Request<'event_loop> {
    fn from_data(data: &'event_loop str) -> Option<Self> {
        let (head, _body) = data.split_once("\r\n\r\n")?;
        let lines: Vec<&str> = head.split("\r\n").collect();
        if lines.len() < 1 { return None }
        let (status_line, headers) = lines.split_at(1);
        let status_line: Vec<_> = status_line[0].split(" ").collect();
        
        if lines.len() >= 3 && status_line.len() >= 3 {
            let mut request = Request{
                method: Method::from_str(status_line[0])?,
                path: PathBuf::from(status_line[1]),
                version: Version::from_str(status_line[2])?,
                headers: HashMap::new(),
            };
            for header in headers {
                let (key, value) = header.split_once(":")?;
                //println!("header key: {}, header value: {}", key, value);
                request.headers.insert(key, value.trim());
            }
            return Some(request);
        }
        else {
            return None
        }
    }
}

pub struct Response {
    pub version: Version,
    pub status: Status,
    pub payload: Vec<u8>,
}

impl Response {
    pub fn deserialize(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(self.payload.len() + 256);

        data.append(&mut self.version.to_str().to_owned().into_bytes());
        data.push(b' ');
        data.append(&mut self.status.to_status_line().to_owned().into_bytes());
        data.append(&mut "\r\n".to_owned().into_bytes());

        data.append(&mut format!("content-length: {}", self.payload.len()).to_owned().into_bytes());
        data.append(&mut "\r\n".to_owned().into_bytes());
        data.append(&mut "Expires: Wed, 21 Oct 2055 07:28:00 GMT".to_owned().into_bytes());
        data.append(&mut "\r\n\r\n".to_owned().into_bytes());

        data.append(&mut self.payload.to_owned());

        data
    }
}

#[repr(i16)]
#[derive(PartialEq)]
pub enum Status {
    //Informational Responses (1XX)
        Continue,
        SwitchingProtocols,
        //Processing, (deprecated)
        EarlyHints,

    //Successful Responses (2XX)
        Ok,
        Created,
        Accepted,
        NonAuthoritativeInformation,
        NoContent,
        ResetContent,
        PartialContent,
        MultiStatus,        //WebDAV
        AlreadyReported,    //WebDAV
        IMUsed,
    //Redirection Responses (3XX)
        MultipleChoices,
        MovedPermanently,
        Found,
        SeeOther,
        NotModified,
        //UseProxy, (deprecated)
        Unused,
        TemporaryRedirect,
        PermanentRedirect,
    //Client-Error Responses (4XX)
        BadRequest,
        Unauthorized,
        PaymentRequired,
        Forbidden,
        NotFound,
        NotAcceptable,
        ProxyAuthenticationRequired,
        RequestTimeout,
        Conflict,
        Gone,
        LengthRequired,
        PreconditionFailed,
        ContentTooLarge,
        URITooLong,
        UnsupportedMediaType,
        RangeNotSatisfiable,
        ExpectationFailed,
        ImaTeapot,
        MisdirectedRequest,
        UnprocessableContent,   //WebDav
        Locked,                 //WebDAV
        FailedDependency,       //WebDAV
        TooEarly,               //Experimental
        UpgradeRequired,
        PreconditionRequired,
        TooManyRequests,
        RequestHeaderFieldTooLarge,
        UnavailableForLegalReasons,
    //Server-Error Responses (5XX)
        InternalServerError,
        NotImplemented,
        BadGateway,
        ServiceUnavailable,
        GatewayTimeout,
        HTTPVersionNotSupported,
        VariantAlsoNegotiates,
        InsufficientStorage,    //WebDAV
        LoopDetected,           //WebDAV
        NotExtended,
        NetworkAuthenticationRequired,
}

impl Status {
    fn to_status_line(&self) -> &str {
        match self {
            Self::Ok => "200 OK",
            Self::Created => "201 Created",
            Self::NotModified => "304 Not Modified",
            Self::NotFound => "404 Not Found",
            _ => "STATUS CODE NOT IMPLEMENTED"
        }
    }
}

pub struct clientManifest {
    contents: Vec<Client>,
    vacancies: Vec<usize>,
}

impl clientManifest {
    pub fn new(capacity: usize) -> clientManifest {
        clientManifest {
            contents: Vec::<Client>::with_capacity(capacity),
            vacancies: Vec::<usize>::with_capacity(capacity),
        }
    }
    pub fn insert(&mut self, connection: Client) -> (&mut Client, Token) {
        match self.vacancies.pop() {
            Some(opening) => {
                self.contents[opening] = connection;
                return (&mut self.contents[opening], Token(opening));
            },
            None => {
                let index = self.contents.len();
                self.contents.push(connection);
                return (&mut self.contents[index], Token(index));
            },
        }
    }
    pub fn get(&mut self, t: Token) -> Option<&mut Client> {
        self.contents.get_mut(t.0)
    }
    pub fn remove(&mut self, t: Token) {
        self.vacancies.push(t.0);
    }
}

