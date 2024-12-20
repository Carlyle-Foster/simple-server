use core::str;
use std::collections::HashMap;
use std::fmt::Debug;
use std::error::Error;

use camino::Utf8PathBuf;

use crate::http::*;

pub trait HttpSmith {
    fn serialize(&self, response: &Response) -> (Vec<u8>, Utf8PathBuf);
    fn deserialize(&self, request: & [u8]) -> Result<Request, ParseError>;
}

pub struct HttpSmithText;

impl HttpSmith for HttpSmithText {
    fn serialize(&self, response: &Response) -> (Vec<u8>, Utf8PathBuf) {
        let mut data = Vec::with_capacity(256);

        data.append(&mut response.version.to_str().to_owned().into_bytes());
        data.push(b' ');
        data.append(&mut response.status.to_status_line().to_owned().into_bytes());
        data.append(&mut "\r\n".to_owned().into_bytes());

        for header in &response.headers {
            data.extend_from_slice(header.0.as_bytes());
            data.push(b':');
            data.push(b' ');
            data.extend_from_slice(header.1.as_bytes());
            data.append(&mut "\r\n".to_owned().into_bytes());
        }
        data.append(&mut "\r\n".to_owned().into_bytes());

        (data, response.body.clone())
    }
    fn deserialize(&self, bytes: &[u8]) -> Result<Request, ParseError> {
        use ParseError::*;
        
        let header = header_from_bytes(bytes)?;
        //println!("{header}");
        let lines: Vec<&str> = header.split("\r\n").collect();
        let (request_line, headers) = lines.split_at_checked(1).ok_or(EmptyRequest)?;
        let request_line: Vec<&str> = request_line[0].split(' ').collect();

        //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2-7
        if request_line.len() != 3 { return Err(BadStatusLine); }

        let method = Method::parse(request_line[0]).ok_or(BadMethod)?;
        let (path, query_params) = match request_line[1].split_once('?') {
            Some((path, query)) => (path.into(), parse_query_parameters(query)?),
            None => (request_line[1].into(), HashMap::new()),
        };
        let version = Version::parse(request_line[2]).ok_or(UnknownVersion)?;

        let mut request = Request{
            method,
            path,
            version,
            headers: HashMap::new(),
            query_params,
            body: Vec::new(),
        };
        for header in headers {
            let (key, value) = header.split_once(":").ok_or(MissingColonInHeader)?;
            let key = key.to_ascii_lowercase();
            let value = value.trim().to_owned();

            //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-5.1-2
            if key.ends_with(|c: char| c.is_whitespace()) { return Err(WhitespaceBeforeColon) }
            //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-5.2-4
            if key.starts_with(|c: char| c.is_whitespace()) { return Err(DeprecatedHeaderFolding) }
            
            if key == "content-length" {
                let content_length = value.parse().map_err(|_| ContentLengthNotAnInteger )?;
                request.body = bytes[header.len()..content_length].to_owned()
            }
            request.headers.insert(key, value);
        }
        return Ok(request);
    }
}

fn parse_query_parameters(query: &str) -> Result<HashMap<String, String>, ParseError> {
    use ParseError::*;

    let mut map = HashMap::new();
    for param in query.split('&') {
        let (key, value) = param.split_once('=').ok_or(BadQuery)?;
        println!("QUERY: key = {key}, value = {value}");
        map.insert(key.to_owned(), value.to_owned());
    }
    return Ok(map)
}

//REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2-2
fn header_from_bytes(mut bytes: &[u8]) -> Result<&str, ParseError> {
    use ParseError::*;

    //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2-6
    if bytes.starts_with(b"\r\n\r\n") { bytes = &bytes[4..] };
    let length = bytes.len();
    for index in 0..length {
        match bytes[index] {
            b'\r' => {
                //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2-4
                if bytes[index+1] != b'\n' {
                    return Err(BareCarriageReturn);
                }
                if (length - index >= 4) && &bytes[index..index+4] == b"\r\n\r\n" {
                    return Ok( unsafe{str::from_utf8_unchecked(&bytes[..index]) } );
                }
            }
            //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2-2
            128.. => return Err(InvalidCharacter),
            _ => {},
        };
    };
    Err(TerminatorNotFound)
}

#[derive(Debug)]
pub enum ParseError {
    InvalidCharacter,
    TerminatorNotFound,
    BareCarriageReturn,
    BadStatusLine,
    EmptyRequest,
    BadMethod,
    UnknownVersion,
    BadQuery,
    MissingColonInHeader,
    WhitespaceBeforeColon,
    DeprecatedHeaderFolding,
    ContentLengthNotAnInteger,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for ParseError {}