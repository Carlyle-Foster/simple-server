use core::str;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::{collections::HashMap, marker::PhantomData};
use std::path::PathBuf;
use std::time::SystemTime;
use std::sync::mpsc;

use chrono::{DateTime, Utc};

use mio::Token;

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use camino::Utf8PathBuf;

use crate::helpers::path_is_sane;
use crate::smithy::{HttpSmith, HttpSmithText, ParseError};
use crate::websocket::{compute_sec_websocket_accept, WebSocket};
use crate::TLS::TLStream;
use crate::{ClientManifest, Protocol, TLServer, Vfs};


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
    pub websocket: Option<mpsc::Sender<TLStream>>,
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
            websocket: None,
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
    pub fn set_homepage(&mut self, path: &str) {
        self.homepage = path.into();
    }
    pub fn set_404_page(&mut self, path: &str) {
        self.not_found = path.into();
    }
    pub fn set_websocket_handler(&mut self, mut handler: impl FnMut(WebSocket) + 'static + std::marker::Send) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let websocket = WebSocket::new(receiver);
        self.websocket = Some(sender);
        std::thread::spawn(move || { handler(websocket) });
    }
    pub fn set_client_directory(&mut self, path: &str) {
        self.client_directory = path.into();
    }
    pub fn serve(&mut self, address: SocketAddr, domain_cert: Vec<CertificateDer<'static>>, private_key: PrivateKeyDer<'static>) {
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(domain_cert, private_key)
            .unwrap();
        let mut tls_server = TLServer::new(address, config);

        println!("HTTPSERVER: serving on (https://{}:{})", address.ip(), address.port());

        loop {
            match tls_server.serve(&mut self.file_system, &mut self.connections) {
                Ok((request, token)) => {
                    //a zero-length buffer signals the completion of a websocket handshake
                    if request.is_empty() {
                        self.sendoff_websocket(token, &tls_server);
                    } else {
                        let client = self.connections.get(token).unwrap();
                        let rq = client.buffer.clone(); // this is stupid!!!
                        self.serve_http(token, &rq, &mut tls_server);
                    }
                },
                Err(e) => panic!("HTTPSERVER_CRASHED (on account of {e})"),
            }
        };
    }

    fn sendoff_websocket(&mut self, token: Token, tls_server: &TLServer) {
        if self.connections.get(token).unwrap().bytes_needed == 0 {
            let client = self.connections.take(token, tls_server.poll.registry()).unwrap();
            let _ = self.websocket.as_mut().unwrap().send(client);
            println!("sendoff_websocket: client {} sent off", token.0);
        }
    }

    fn serve_http(&mut self, token: Token, request: &[u8], tls_server: &mut TLServer) {
        match self.smith.deserialize(request) {
            Ok((request, bytes)) => {
                let response = self.handle_request(request);
                let (head, body) = self.smith.serialize(&response);

                let client = self.connections.get(token).unwrap();
                client.buffer.clear(); // TODO: make this a ring buffer?
                assert!(client.protocol != Protocol::WEBSOCKET);
                if response.status == Status::SwitchingProtocols {
                    client.protocol = Protocol::WEBSOCKET;
                }

                if !bytes.is_empty() {
                    println!("DEBUG: dropped {} bytes!!!", bytes.len());
                }

                match tls_server.dispatch_delivery(head, body, &mut self.file_system, client) {
                    Ok(_) => {},
                    //this is so stupid but it's not like Unsupported is otherwise returnable, so... 
                    Err(e) if e.kind() == ErrorKind::Unsupported => self.sendoff_websocket(token, tls_server),
                    Err(e) if e.kind() == ErrorKind::ConnectionAborted => {
                        println!("HTTPSERVER: client {} disconnected (discovered when we tried writing to them)", token.0);
                        self.connections.remove(token);
                    },
                    Err(e) => {
                        println!("HTTPSERVER: client {} dropped on account of IO error: {{{e}}}", token.0);
                        self.connections.remove(token);
                    },
                };
            },
            Err(ParseError::Incomplete) => {
                let client = self.connections.get(token).unwrap();
                client.buffer.extend_from_slice(request);
            }
            Err(e) => {
                println!("HTTPSERVER: client {} dropped on account of bad request, Message Parsing error: {{{e}}}", token.0);
                self.connections.remove(token);
            },
        }
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
            let mut response: Response = Status::SwitchingProtocols.into();
            response.add_header("connection", "upgrade");
            response.add_header("upgrade", "websocket");
            response.add_header("sec-websocket-version", "13");
            response.add_header("sec-websocket-accept", &compute_sec_websocket_accept(key));
            response.add_header("content-length", "0");
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
            body_size = self.file_system.get_size(&self.not_found).unwrap_or(0)
        }
        //format: Sun, 06 Nov 1994 08:49:37 GMT
        let time: DateTime<Utc> = SystemTime::now().into();
        let timestamp = time.to_rfc2822();
        response.add_header("server", "simple-server");
        response.add_header("date", &timestamp);
        response.add_header("content-length", &format!("{}", body_size));
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
    pub fn parse(s: &str) -> Option<Method> {
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
    pub fn parse(s: &str) -> Option<Version> {
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
pub struct Header(pub &'static str, pub String);

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

impl Response {
    pub fn add_header(&mut self, key: &'static str, value: &str) {
        self.headers.push(Header(key, value.to_owned()))
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