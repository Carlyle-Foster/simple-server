use core::str;
use std::collections::HashMap;
use std::fmt::Debug;
use std::error::Error;

use camino::Utf8PathBuf;

use crate::helpers::Parser;
use crate::http::*;

pub trait HttpSmith {
    fn serialize(&self, response: &Response) -> (Vec<u8>, Utf8PathBuf);
    fn deserialize<'b>(&self, buf: &'b [u8]) -> Result<(Request, &'b [u8]), ParseError>;
}

#[derive(Debug, Clone, Copy, Default)]
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
    fn deserialize<'b>(&self, buf: &'b [u8]) -> Result<(Request, &'b [u8]), ParseError> {
        use ParseError::*;
        
        let (header, mut rest) = header_from_bytes(buf)?;
        //println!("{header}");
        let lines: Vec<&str> = header.split("\r\n").collect();
        let (request_line, headers) = lines.split_at(1);
        let request_line: Vec<&str> = request_line[0].split(' ').collect();

        //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2-7
        if request_line.len() != 3 { return Err(BadStatusLine); }

        let method = Method::parse(request_line[0]).ok_or(BadMethod)?;
        let (path, query_params) = match request_line[1].split_once('?') {
            Some((path, query)) => (path.into(), parse_query_parameters(query)?),
            None => (request_line[1].into(), HashMap::new()),
        };

        //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2-8
        if request_line[2].ends_with(|c: char| c.is_whitespace()) { return  Err(WhiteSpaceAfterStartLine) }

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
            let (key, value) = header.split_once(":").ok_or(MissingValueInField)?;
            let key = key.to_ascii_lowercase();
            let value = value.trim().to_owned();

            //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-5.1-2
            if key.ends_with(|c: char| c.is_whitespace()) { return Err(WhitespaceBeforeColon) }
            //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-5.2-4
            if key.starts_with(|c: char| c.is_whitespace()) { return Err(DeprecatedHeaderFolding) }
            
            if key == "content-length" {
                let content_length = value.parse::<usize>().map_err(|_| InvalidContentLength )?;
                let body;
                (body, rest) = rest.split_at_checked(content_length).ok_or(Incomplete)?;
                request.body = body.to_owned()
            }
            request.headers.insert(key, value);
        }
        return Ok((request, rest));
    }
}

impl Parser<Request, ParseError> for HttpSmithText {
    fn parse<'b>(&mut self, buf: &'b [u8]) -> Result<(Option<Request>, &'b [u8]), ParseError> {
        match self.deserialize(buf) {
            Ok((req, rest)) => Ok((Some(req), rest)),
            Err(ParseError::Incomplete) => Ok((None, buf)),
            Err(e) => Err(e),
        }
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
fn header_from_bytes(mut bytes: &[u8]) -> Result<(&str, &[u8]), ParseError> {
    use ParseError::*;

    //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2-6
    if bytes.starts_with(b"\r\n\r\n") { bytes = &bytes[4..] };
    let length = bytes.len();
    for index in 0..length {
        match bytes[index] {
            b'\r' => {
                if let Some(b'\n') = bytes.get(index+1) {
                    if let Some(b"\r\n\r\n") = bytes.get(index..index+4) {
                        return Ok(( unsafe{str::from_utf8_unchecked(&bytes[..index]) }, &bytes[index+4..] ));
                    }
                } else {
                    //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2-4
                    return Err(BareCarriageReturn)
                }
            }
            //REF: https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2-2
            128.. => return Err(InvalidCharacter),
            _ => {},
        };
    };
    Err(Incomplete)
}

#[derive(Debug)]
#[derive(PartialEq)]
pub enum ParseError {
    Incomplete,
    InvalidCharacter,
    TerminatorNotFound,
    BareCarriageReturn,
    BadStatusLine,
    BadMethod,
    UnknownVersion,
    BadQuery,
    MissingValueInField,
    WhitespaceBeforeColon,
    DeprecatedHeaderFolding,
    InvalidContentLength,
    WhiteSpaceAfterStartLine,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for ParseError {}