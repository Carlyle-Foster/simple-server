use core::str;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::http::*;

pub trait HttpSmith {
    fn serialize(&self, response: &Response) -> Vec<u8>;
    fn deserialize<'a>(&self, request: &'a [u8]) -> Option<Request<'a>>;
}

pub struct HttpSmithText;

impl HttpSmith for HttpSmithText {
    fn serialize(&self, response: &Response) -> Vec<u8> {
        let mut data = Vec::with_capacity(response.payload.len() + 256);

        data.append(&mut response.version.to_str().to_owned().into_bytes());
        data.push(b' ');
        data.append(&mut response.status.to_status_line().to_owned().into_bytes());
        data.append(&mut "\r\n".to_owned().into_bytes());

        data.append(&mut format!("content-length: {}", response.payload.len()).to_owned().into_bytes());
        data.append(&mut "\r\n".to_owned().into_bytes());
        data.append(&mut "Expires: Wed, 21 Oct 2055 07:28:00 GMT".to_owned().into_bytes());
        data.append(&mut "\r\n\r\n".to_owned().into_bytes());

        data.extend_from_slice(&response.payload);

        data
    }
    fn deserialize<'a>(&self, request: &'a [u8]) -> Option<Request<'a>> {
        let text;
        match str::from_utf8(request) {
            Ok(t) => text = t,
            Err(_) => return None,
        }
        let (head, _body) = text.split_once("\r\n\r\n")?;
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