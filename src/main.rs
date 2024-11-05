pub mod library;
pub mod helpers;

//use std::collections::HashMap;
use std::path::Path;

use library::{Method, Request, Response, Server, Service};
use helpers::{IntoResponse, VirtualFile};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

fn main(){
    let domain_cert = CertificateDer::pem_file_iter("./https_certificates/domain.cert.pem")
        .unwrap()
        .map(|cert| cert.unwrap())
        .collect();

    let private_key = PrivateKeyDer::from_pem_file("./https_certificates/private.key.pem").unwrap();

    let mut server = Server::new("127.0.0.1:8783");
    server.add_service(Service::new("/", Method::GET, &serve_client_directory));
    server.add_404_page(Path::new("client/missing.html"));
    server.add_certs(domain_cert, private_key);
    //server.add_service(Service::new("/", Method::TRACE, &trace_handler));
    server.serve_with_tls().unwrap();
}

fn serve_client_directory(request: Request) -> Response {
    let file_path = match request.path == Path::new("") {
        true => Path::new("index.html"),
        false => Path::new(&request.path),
    };
    VirtualFile {
        root: Path::new("client"),
        path: file_path,
    }.into_response()
}

// fn trace_handler(request: Request) -> Response {

// }
