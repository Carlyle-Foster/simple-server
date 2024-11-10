pub mod library;
pub mod helpers;

use std::path::Path;

use library::{Method, Request, Response, Server, Service};
use helpers::{IntoResponse, VirtualFile, get_domain_certs, get_private_key};

fn main() {
    let mut server = Server::new("127.0.0.1:8783");
    server.add_service(Service::new("/", Method::GET, &serve_client_directory));
    server.add_404_page(Path::new("client/missing.html"));

    let domain_cert = get_domain_certs("https_certificates/domain.cert.pem");
    let private_key = get_private_key("https_certificates/private.key.pem");

    server.serve_with_tls(domain_cert, private_key).unwrap();
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
