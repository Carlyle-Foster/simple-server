use core::str;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;
use chrono::*;

use crate::helpers::IntoResponse;

type Handler = dyn FnMut(Request) -> Response;

pub struct Service<'a> {
    path: PathBuf,
    method: Method,
    handler: &'a mut Handler,
}

impl<'a> Service<'a> {
    pub fn new(path_str: &str, method: Method, handler: &'a mut Handler ) -> Self {
        Service {
            path: path_str.parse().unwrap(),
            method,
            handler,
        }
    }
}

pub struct HttpServer<'a> {
    pub services: Vec<Service<'a>>,
    pub not_found: Vec<u8>,
}

impl<'a> HttpServer<'a> {
    pub fn handle_request(&mut self, mut request: Request) -> Response {
        let mut response = ().into_response();
        for service in &mut self.services {
            //println!("service path: {:?}, request path: {:?}", service.path, request.path);
            if request.path.starts_with(&service.path) && (request.method == service.method || service.method == Method::ANY) {
                request.path = request.path.strip_prefix(&service.path).unwrap().to_path_buf();
                response = (service.handler)(request);
                break
            }
        }
        if response.status == Status::NotFound {
            response.payload = self.not_found.clone();
        }
        //format: Sun, 06 Nov 1994 08:49:37 GMT
        let time: DateTime<Utc> = SystemTime::now().into();
        let timestamp = time.to_rfc2822();
        println!("{}", timestamp);
        response.headers.push(Header("server".to_string(), "simple-server".to_string()));
        response.headers.push(Header("date".to_string(), timestamp));
        response.headers.push(Header("content-length".to_string(), format!("{}", response.payload.len())));
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
    pub fn from_str(s: &str) -> Option<Version> {
        println!("{}", s);
        match s {
            "HTTP/1" => return Some(Version::V_1_0),
            "HTTP/1.1" => return Some(Version::V_1_1),
            "HTTP/2" => return Some(Version::V_2_0),
            "HTTP/3" => return Some(Version::V_3_0),
            _ => return None,
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
    pub path: PathBuf,
    pub version: Version,
    pub headers: HashMap<String, String>,
}


pub struct Response {
    pub version: Version,
    pub status: Status,
    pub headers: Vec<Header>,
    pub payload: Vec<u8>,
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
            _ => "STATUS CODE NOT IMPLEMENTED"
        }
    }
}