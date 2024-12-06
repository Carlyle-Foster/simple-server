use std::net::SocketAddr;

use camino::Utf8PathBuf;
use simple_server::http::HttpServer;
use simple_server::http::{Method, Request};
use simple_server::helpers::{get_domain_certs, get_private_key};

fn main() {
    let mut server = HttpServer::new();

    server.set_client_directory("client");
    server.add_service("/", Method::GET, serve_client_directory());
    server.add_homepage("index.html");
    server.add_404_page("client/missing.html");

    let domain_cert = get_domain_certs("https_certificates/domain.cert.pem");
    let private_key = get_private_key("https_certificates/private.key.pem");

    server.serve(SocketAddr::from(([127, 0, 0, 1], 8783)), domain_cert, private_key);
}

fn serve_client_directory() -> impl FnMut(Request) -> Utf8PathBuf {
    move |request: Request| -> Utf8PathBuf {
        request.path
    }
}

// fn serve_static_string() -> impl FnMut(Request) -> String {
//     let mut upper = false;
//     let str = "hello world";
//     move |_: Request| -> String {
//         upper = !upper;
//         if upper {
//             str.to_uppercase()
//         }
//         else {
//             str.to_lowercase()
//         }
//     }
// }

