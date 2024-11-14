use std::path::Path;

use simple_server::Server;
use simple_server::http::{Method, Request, Response};
use simple_server::helpers::{IntoResponse, VirtualFile, get_domain_certs, get_private_key};

fn main() {
    let mut server = Server::new("127.0.0.1:8783");
    let mut service = serve_client_directory();

    server.add_service("/", Method::GET, &mut service);
    server.add_404_page("client/missing.html");

    let domain_cert = get_domain_certs("https_certificates/domain.cert.pem");
    let private_key = get_private_key("https_certificates/private.key.pem");

    server.serve_with_tls(domain_cert, private_key).unwrap();
}

fn serve_client_directory() -> impl FnMut(Request) -> Response {
    move |request: Request| -> Response {
        let file_path = match request.path == Path::new("") {
            true => Path::new("index.html"),
            false => Path::new(&request.path),
        };
        VirtualFile {
            root: Path::new("client"),
            path: file_path,
            request: &request,
        }.into_response()
    }
}

