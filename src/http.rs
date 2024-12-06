use core::str;
use std::io::{stdin, ErrorKind, Read};
use std::net::SocketAddr;
use std::{collections::HashMap, marker::PhantomData};
use std::path::PathBuf;
use std::time::SystemTime;

use chrono::*;

use mio::Token;

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use camino::Utf8PathBuf;

use crate::helpers::path_is_sane;
use crate::smithy::{HttpSmith, HttpSmithText};
use crate::websocket::compute_sec_websocket_accept;
use crate::{ClientManifest, Protocol, TLServer, Vfs, ADMIN};


pub struct Service {
    path: PathBuf,
    method: Method,
    handler: Box<dyn Handle>,
}

impl Service {
    pub fn new<I, O>(path_str: &str, method: Method, function: impl FnMut(I) -> O + 'static) -> Self 
    where
        I: From<Request> + 'static,
        O: Into<Response> + 'static,
    {
        Service {
            path: path_str.parse().unwrap(),
            method,
            handler: Box::new(Handler::new(function)),
        }
    }
}


pub struct HttpServer {
    pub services: Vec<Service>,
    pub connections: ClientManifest,
    pub client_directory: Utf8PathBuf,
    pub homepage: Utf8PathBuf,
    pub not_found: Utf8PathBuf,
    pub file_system: Vfs,
    pub smith: HttpSmithText,
}

impl HttpServer {
    pub fn new() -> Self {
        Self {
            services: Vec::with_capacity(4),
            connections: ClientManifest::new(128),
            client_directory: Utf8PathBuf::new(),
            homepage: Utf8PathBuf::new(),
            not_found: Utf8PathBuf::new(),
            file_system: Vfs(HashMap::new()),
            smith: HttpSmithText{},
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
    pub fn add_homepage(&mut self, path: &str) {
        self.homepage = path.into();
    }
    pub fn add_404_page(&mut self, path: &str) {
        self.not_found = path.into();
    }
    pub fn set_client_directory(&mut self, path: &str) {
        self.client_directory = path.into();
    }
    pub fn serve(&mut self, address: SocketAddr, domain_cert: Vec<CertificateDer<'static>>, private_key: PrivateKeyDer<'static>) {
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(domain_cert, private_key)
            .unwrap();
        
        println!("HTTPSERVER: serving on (https://{}:{})", address.ip(), address.port());
        self.generic_serve(address, config);
    }
    pub fn generic_serve(&mut self, address: SocketAddr, config: ServerConfig) {
        let mut buffer = Vec::with_capacity(4096);
        let mut tls_server = TLServer::new(address, config);
        let mut command_buffer = [0; 4096];
        
        loop {
            match tls_server.serve(&mut buffer, &mut self.file_system, &mut self.connections) {
                Ok((request, token)) => {
                    if token == ADMIN {
                        let bytes_read = stdin().read(&mut command_buffer).unwrap();
                        let command = String::from_utf8_lossy(&command_buffer[..bytes_read]);
                        
                        self.COMMAND(&command);
                        continue
                    }
                    match self.connections.get(token).unwrap().protocol {
                        Protocol::HTTP => {
                            self.serve_http(token, request, &mut tls_server);
                        },
                        Protocol::WEBSOCKET => {
                            
                        }
                    }
                },
                Err(e) => panic!("HTTPSERVER_CRASHED (on account of {e})"),
            }
        };
    }

    fn serve_http(&mut self, token: Token, request: &[u8], tls_server: &mut TLServer) {
        match self.smith.deserialize(request) {
            Ok(request) => {
                let response = self.handle_request(request);
                let (head, body) = self.smith.serialize(&response);
                let client = self.connections.get(token).unwrap();
                if response.status == Status::SwitchingProtocols {
                    client.protocol = Protocol::WEBSOCKET;
                }
                match tls_server.dispatch_delivery(head, body, &mut self.file_system, client) {
                    Ok(_) => {},
                    Err(ref e) if e.kind() == ErrorKind::ConnectionAborted => {
                        println!("HTTPSERVER: client {} disconnected (discovered when we tried writing to them)", token.0);
                        self.connections.remove(token);
                    },
                    Err(e) => {
                        println!("HTTPSERVER: client {} dropped on account of IO error: {{{e}}}", token.0);
                        self.connections.remove(token);
                    },
                };
            },
            Err(e) => {
                println!("HTTPSERVER: client {} dropped on account of bad request, Message Parsing error: {{{e}}}", token.0);
                self.connections.remove(token);
            },
        }
    }

    fn COMMAND(&mut self, command: &str) -> Option<()> {
        let parts: Vec<&str> = command.split(|c: char| c.is_whitespace()).filter(|s| !s.is_empty()).collect();
        let command_word = parts.get(0)?.trim().to_lowercase();

        println!("");
    
        match command_word.as_ref() {
            "clients" => {
                println!("Clients:");
                for entry in &self.connections.contents {
                    if let Some(client) = entry {
                        println!("    {}: bytes_needed = {}, delivery = {}, protocol = {:?}", client.token.0, client.bytes_needed, client.delivery, client.protocol);
                    }
                }
            }
            "kick" => {
                if let Some(client) = parts.get(1) {
                    if let Ok(id) = client.parse::<usize>() {
                        let token = Token(id);
                        if self.connections.get(token).is_none() { println!("client {id} does not exist"); }
                        else {
                            self.connections.remove(Token(id));
                            println!("SUCCESS: curbstomped client {id}");
                        }
                    }
                    else {
                        println!("kick couldn't parse that client ID (the ID in question: {client})");
                    }
                }
                else {
                    println!("kick requires a client ID, EXAMPLE: kick 3");
                }
            }
            "files" => {
                println!("Files:");
                for (path, _file) in self.file_system.0.iter() {
                    println!("    {}", path);
                }
            }
            _ => println!("{command_word} is not a COMMAND, maybe you spelled it wrong?"),
        };
        Some(())
    }
}

impl Default for HttpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpServer {
    pub fn handle_request(&mut self, mut request: Request) -> Response {
        if let Some(key) = request.headers.get("sec-websocket-key") {
            println!("websockets!!, {:#?}", key);
            let mut response: Response = Status::SwitchingProtocols.into();
            response.headers.push(Header("connection".to_string(), "upgrade".to_string()));
            response.headers.push(Header("upgrade".to_string(), "websocket".to_string()));
            //response.headers.push(Header("sec-websocket-version".to_string(), "13".to_string()));
            //response.headers.push(Header("sec-websocket-protocol".to_string(), "chat".to_string()));
            response.headers.push(Header("sec-websocket-accept".to_string(), compute_sec_websocket_accept(key)));
            //response.headers.push(Header("content-length".to_string(), "0".to_string()));
            return response;
        }
        let mut response = ().into();
        for service in &mut self.services {
            //println!("service path: {:?}, request path: {:?}", service.path, request.path);
            if request.path.starts_with(&service.path) && (request.method == service.method || service.method == Method::ANY) {
                request.path = request.path.strip_prefix(&service.path).unwrap().to_path_buf();
                response = service.handler.handle(request);
                break
            }
        }
        let mut dropped = false;
        if response.body == PathBuf::new() {
            response.body = self.homepage.clone();
        }
        if !path_is_sane(&response.body) { dropped = true }
        response.body = self.client_directory.clone().join(&response.body);
        let mut body_size = 0;
        match self.file_system.get(&response.body) {
            Some(file) => body_size = file.data.len(),
            None => dropped = true,
        };
        if dropped {
            response.status = Status::NotFound;
            response.body = self.not_found.clone();
            body_size = self.file_system.get_size(&self.not_found).unwrap()
        }
        //format: Sun, 06 Nov 1994 08:49:37 GMT
        let time: DateTime<Utc> = SystemTime::now().into();
        let timestamp = time.to_rfc2822();
        response.headers.push(Header("server".to_string(), "simple-server".to_string()));
        response.headers.push(Header("date".to_string(), timestamp));
        response.headers.push(Header("content-length".to_string(), format!("{}", body_size)));
        response
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
    pub fn from_str(s: &str) -> Option<Method> {
        //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-3.1-1
        match s {
            "GET"   => Some(Method::GET),
            "POST"  => Some(Method::POST),
            _       => None,
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
    pub fn from_str(s: &str) -> Option<Version> {
        //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-2.3-2
        match s.to_ascii_uppercase().as_ref() {
            "HTTP/1"    => Some(Version::V_1_0),
            "HTTP/1.1"  => Some(Version::V_1_1),
            "HTTP/2"    => Some(Version::V_2_0),
            "HTTP/3"    => Some(Version::V_3_0),
            _ => None,
        }
    }
    pub fn to_str(&self) -> &str {
        match self {
            Version::V_1_0 => "HTTP/1",
            Version::V_1_1 => "HTTP/1.1",
            Version::V_2_0 => "HTTP/2",
            Version::V_3_0 => "HTTP/3",
        }
    }
}

#[derive(Debug)]
pub struct Header(pub String, pub String);

#[derive(Debug)]
pub struct Request {
    pub method: Method,
    pub path: Utf8PathBuf,
    pub version: Version,
    pub headers: HashMap<String, String>,
    pub query_params: HashMap<String, String>,
    pub body: Vec<u8>,
}


pub struct Response {
    pub version: Version,
    pub status: Status,
    pub headers: Vec<Header>,
    pub body: Utf8PathBuf,
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
    pub fn to_status_line(&self) -> &str {
        match self {
            Self::Ok => "200 OK",
            Self::Created => "201 Created",
            Self::NotModified => "304 Not Modified",
            Self::NotFound => "404 Not Found",
            Self::SwitchingProtocols => "101 Switching Protocols",
            _ => "STATUS CODE NOT IMPLEMENTED"
        }
    }
}

pub struct Handler<I, O, F>(pub F, PhantomData<I>, PhantomData<O>)
where
    I: From<Request>,
    O: Into<Response>,
    F: FnMut(I) -> O,
;

impl<I, O, F> Handler<I, O, F> 
where
    I: From<Request>,
    O: Into<Response>,
    F: FnMut(I) -> O,
{
    pub fn new(function: F) -> Self {
        Self(function, Default::default(), Default::default())
    }
}

pub trait Handle {
    fn handle(&mut self, r: Request) -> Response;
}

impl<I, O, F> Handle for Handler<I, O, F>
where
    I: From<Request>,
    O: Into<Response>,
    F: FnMut(I) -> O,
{
    fn handle(&mut self, r: Request) -> Response {
        (self.0)(I::from(r)).into()
    }
}