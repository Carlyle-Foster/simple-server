use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf, Component};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use chrono::*;

use crate::*;
use crate::http::*;

pub fn get_domain_certs(path: &str) -> Vec<CertificateDer> {
    CertificateDer::pem_file_iter(path)
        .unwrap()
        .map(|cert| cert.unwrap())
        .collect()
}

pub fn get_private_key(path: &str) -> PrivateKeyDer<'_> {
    PrivateKeyDer::from_pem_file(path).unwrap()
}

fn path_is_sane(path: &Path) -> bool {
    let mut sane = true;
    for segment in path.components() {
        match segment {
            Component::Prefix(_) => sane = false,
            Component::RootDir => sane = false,
            Component::CurDir => continue,
            Component::ParentDir => sane = false,
            Component::Normal(_) => continue,
        };
    }
    return sane;
}

pub trait IntoResponse {
    fn into_response(self) -> Response;
}

impl IntoResponse for Status {
    fn into_response(self) -> Response {
        Response {
            version: Version::V_1_1,
            status: self,
            headers: vec![],
            payload: vec![],
        }
    }
}

impl IntoResponse for Vec<u8> {
    fn into_response(self) -> Response {
        Response {
            version: Version::V_1_1,
            status: Status::Ok,
            headers: vec![],
            payload: self,
        }
    }
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        self.into_bytes().into_response()
    }
}

impl IntoResponse for PathBuf {
    //TODO: proper error handling via HTTP status line
    fn into_response(self) -> Response {
        match fs::read(&self){
            Ok(data) => data.into_response(),
            Err(_) => ().into_response(),
        }
    }
}

pub struct VirtualFile<'a, 'b, 'c> {
    pub root: &'a Path,
    pub path: &'b Path,
    pub request: &'c Request,
}

impl<'a, 'b, 'c> IntoResponse for VirtualFile<'a, 'b, 'c> {
    fn into_response(self) -> Response {
        if path_is_sane(self.path) {
            let virtual_path = PathBuf::from(self.root).join(self.path);
            println!("{:#?}", virtual_path);
            match self.request.headers.get("if-modified-since") {
                Some(date) => {
                    let time: DateTime<Utc>;
                    match DateTime::parse_from_rfc2822(date) {
                        Ok(t) => time = t.into(),
                        Err(_) => panic!("blah"),
                    };
                    let last_modified: DateTime<Utc> = fs::metadata(&virtual_path).unwrap().modified().unwrap().into();
                    match time.cmp(&last_modified) {
                        Ordering::Less => virtual_path.into_response(),
                        Ordering::Equal => Status::NotModified.into_response(),
                        Ordering::Greater => Status::NotModified.into_response(),
                    }
                },
                None => virtual_path.into_response(),
            }
        }
        else {
            ().into_response()
        }
    }
}

impl IntoResponse for () {
    fn into_response(self) -> Response {
        Response {
            version: Version::V_1_1,
            status: Status::NotFound,
            headers: vec![],
            payload: Vec::new(),
        }
    }
}
