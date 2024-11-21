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

impl From<Status> for Response {
    fn from(status: Status) -> Response {
        Response {
            version: Version::V_1_1,
            status,
            headers: vec![],
            payload: vec![],
        }
    }
}

impl From<Vec<u8>> for Response {
    fn from(payload: Vec<u8>) -> Response {
        Response {
            version: Version::V_1_1,
            status: Status::Ok,
            headers: vec![],
            payload,
        }
    }
}

impl From<String> for Response {
    fn from(payload: String) -> Response {
        payload.into_bytes().into()
    }
}

impl From<PathBuf> for Response {
    //TODO: proper error handling via HTTP status line
    fn from(path: PathBuf) -> Response {
        match fs::read(&path){
            Ok(data) => {
                let mut r: Response = data.into();
                let timestamp: DateTime<Utc> = fs::metadata(&path).unwrap().modified().unwrap().into();
                r.headers.push(Header("last-modified-since".to_owned(), timestamp.to_rfc2822()));
                r
            },
            Err(_) => ().into(),
        }
    }
}

pub struct VirtualFile {
    pub root: PathBuf,
    pub path: PathBuf,
    pub if_modified_since: Option<DateTime<Utc>>,
}

impl VirtualFile {
    pub fn new(root: PathBuf, default: &str, request: Request) -> Self {
        Self {
            root,
            path: 
                match request.path == Path::new("") {
                    true => default.into(),
                    false => request.path,
                },
            if_modified_since: 
                match request.headers.get("if-modified-since") {
                    Some(date) => {
                        let date: DateTime<Utc> = DateTime::parse_from_rfc2822(date)
                            .unwrap()
                            .into();
                        Some(date)
                    },
                    None => None,
            },
        }
    }
}

impl From<VirtualFile> for Response {
    fn from(file: VirtualFile) -> Response {
        if path_is_sane(&file.path) {
            let virtual_path = PathBuf::from(file.root).join(file.path);
            println!("{:#?}", virtual_path);
            match file.if_modified_since {
                Some(date) => {
                    let last_modified: DateTime<Utc> = fs::metadata(&virtual_path).unwrap().modified().unwrap().into();
                    match date.cmp(&last_modified) {
                        Ordering::Less => virtual_path.into(),
                        Ordering::Equal => Status::NotModified.into(),
                        Ordering::Greater => Status::NotModified.into(),
                    }
                },
                None => virtual_path.into(),
            }
        }
        else {
            ().into()
        }
    }
}

impl From<()> for Response {
    fn from(_: ()) -> Response {
        Response {
            version: Version::V_1_1,
            status: Status::NotFound,
            headers: vec![],
            payload: Vec::new(),
        }
    }
}

impl From<Request> for PathBuf {
    fn from(r: Request) -> PathBuf {
        r.path
    }
}